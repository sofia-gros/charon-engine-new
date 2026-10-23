//! wgpu momentum/pressure compute. CPU f64 VOF remains the mass authority.
//! f32 pressure corrections are refined against the original f64 operator.
use crate::{Config, Error};
use std::{
    sync::{mpsc, Mutex},
    time::Duration,
};

struct Timer {
    queries: wgpu::QuerySet,
    resolved: wgpu::Buffer,
}
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Work {
    pub kernel_ms: Option<f64>,
    pub upload_bytes: u64,
    pub readback_bytes: u64,
    pub dispatches: u32,
}
impl Work {
    fn add(&mut self, other: Self) {
        self.kernel_ms = self.kernel_ms.zip(other.kernel_ms).map(|(a, b)| a + b);
        self.upload_bytes += other.upload_bytes;
        self.readback_bytes += other.readback_bytes;
        self.dispatches += other.dispatches;
    }
}
pub(super) struct Velocity {
    pub u: Vec<f64>,
    pub v: Vec<f64>,
    pub work: Work,
}
pub struct GpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    velocity: wgpu::ComputePipeline,
    pressure: wgpu::ComputePipeline,
    info: wgpu::AdapterInfo,
    scratch: Mutex<[Option<wgpu::Buffer>; 6]>,
    timer: Option<Timer>,
}
impl GpuContext {
    pub fn new() -> Result<Self, String> {
        Self::with_timing(true)
    }
    /// Unsupported timestamp queries leave kernel times unavailable; compute
    /// remains usable. Turning profiling off must not change physical state.
    pub fn with_timing(enabled: bool) -> Result<Self, String> {
        pollster::block_on(Self::create(enabled))
    }
    async fn create(timing: bool) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::DX12,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        let info = adapter.get_info();
        let timing_features = if timing {
            adapter.features() & wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("charon-fluid-compute"),
                required_features: timing_features,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = |name, source: &'static str| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let velocity = pipeline("MAC advection", include_str!("velocity.wgsl"));
        let pressure = pipeline("compact pressure CG", include_str!("pressure.wgsl"));
        if let Some(e) = scope.pop().await {
            return Err(e.to_string());
        }
        let timer = (!timing_features.is_empty()).then(|| Timer {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("fluid kernel timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            }),
            resolved: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fluid timestamp resolve"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
        });
        Ok(Self {
            timer,
            device,
            queue,
            velocity,
            pressure,
            info,
            scratch: Mutex::new(Default::default()),
        })
    }
    pub fn timing_available(&self) -> bool {
        self.timer.is_some()
    }
    pub(super) fn empty_work(&self) -> Work {
        Work {
            kernel_ms: self.timer.as_ref().map(|_| 0.),
            ..Work::default()
        }
    }
    fn readback_size(&self, size: u64) -> u64 {
        if self.timer.is_some() {
            size.next_multiple_of(8) + 16
        } else {
            size
        }
    }
    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.info
    }
    /// Trace identity: exact replay is scoped to the same backend/driver.
    pub fn identity(&self) -> String {
        let data = format!("{:?}", self.info);
        let hash = data.bytes().fold(0xcbf29ce484222325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
        });
        format!("gpu-{hash:016x}")
    }
    fn buffer(
        &self,
        cache: &mut [Option<wgpu::Buffer>; 6],
        slot: usize,
        label: &str,
        size: u64,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        if cache[slot].as_ref().is_none_or(|b| b.size() < size) {
            cache[slot] = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.next_power_of_two(),
                usage,
                mapped_at_creation: false,
            }));
        }
        cache[slot].as_ref().unwrap().clone()
    }
    fn run(
        &self,
        pipeline: &wgpu::ComputePipeline,
        buffers: &[(&wgpu::Buffer, u64)],
        output: &wgpu::Buffer,
        size: u64,
        readback: &wgpu::Buffer,
        groups: u32,
    ) -> Result<(Vec<u8>, Work), Error> {
        let layout = pipeline.get_bind_group_layout(0);
        let entries: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(i, b)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: b.0,
                    offset: 0,
                    size: std::num::NonZeroU64::new(b.1),
                }),
            })
            .collect();
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fluid compute"),
            layout: &layout,
            entries: &entries,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("fluid compute"),
                timestamp_writes: self.timer.as_ref().map(|timer| {
                    wgpu::ComputePassTimestampWrites {
                        query_set: &timer.queries,
                        beginning_of_pass_write_index: Some(0),
                        end_of_pass_write_index: Some(1),
                    }
                }),
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(groups, 1, 1);
        }
        encoder.copy_buffer_to_buffer(output, 0, readback, 0, size);
        let read_size = self.readback_size(size);
        if let Some(timer) = &self.timer {
            encoder.resolve_query_set(&timer.queries, 0..2, &timer.resolved, 0);
            encoder.copy_buffer_to_buffer(&timer.resolved, 0, readback, read_size - 16, 16);
        }
        let submission = self.queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::channel();
        readback
            .slice(..read_size)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = sender.send(r);
            });
        let result = (|| {
            self.device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(Duration::from_secs(30)),
                })
                .map_err(|_| Error::Backend)?;
            receiver
                .recv_timeout(Duration::from_secs(30))
                .map_err(|_| Error::Backend)?
                .map_err(|_| Error::Backend)?;
            let bytes = readback
                .slice(..read_size)
                .get_mapped_range()
                .map_err(|_| Error::Backend)?
                .to_vec();
            let kernel_ms = self.timer.as_ref().and_then(|_| {
                let times = &bytes[read_size as usize - 16..];
                let begin = u64::from_le_bytes(times[..8].try_into().unwrap());
                let end = u64::from_le_bytes(times[8..].try_into().unwrap());
                end.checked_sub(begin)
                    .map(|delta| delta as f64 * f64::from(self.queue.get_timestamp_period()) * 1e-6)
            });
            let work = Work {
                kernel_ms,
                readback_bytes: read_size,
                dispatches: 1,
                upload_bytes: 0,
            };
            Ok((bytes[..size as usize].to_vec(), work))
        })();
        // Cancel pending maps on failure as well; a later retry must not map an
        // already-pending staging buffer after a timeout or device loss.
        readback.unmap();
        result
    }
    pub(super) fn advect(
        &self,
        config: Config,
        u: &[f64],
        v: &[f64],
        bounds: Option<[usize; 4]>,
        dt: f64,
    ) -> Result<Velocity, Error> {
        let Some(b) = bounds else {
            return Ok(Velocity {
                u: vec![0.; u.len()],
                v: vec![0.; v.len()],
                work: self.empty_work(),
            });
        };
        let w = config.width;
        let h = config.height;
        let mut params = Vec::new();
        for x in [
            w,
            h,
            b[0].saturating_sub(3).max(1),
            b[1].saturating_sub(3).max(1),
            (b[2] + 3).min(w),
            (b[3] + 3).min(h),
        ] {
            params.extend_from_slice(&(x as u32).to_le_bytes());
        }
        params.extend_from_slice(&(dt as f32).to_le_bytes());
        params.extend_from_slice(&((dt * config.viscosity) as f32).to_le_bytes());
        let mut cache = self.scratch.lock().map_err(|_| Error::Backend)?;
        let params_buffer = self.buffer(
            &mut cache,
            0,
            "velocity parameters",
            params.len() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        self.queue.write_buffer(&params_buffer, 0, &params);
        let input_data: Vec<_> = u
            .iter()
            .chain(v)
            .flat_map(|x| (*x as f32).to_le_bytes())
            .collect();
        let size = input_data.len() as u64;
        let input = self.buffer(
            &mut cache,
            1,
            "old velocity",
            size,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        self.queue.write_buffer(&input, 0, &input_data);
        let output = self.buffer(
            &mut cache,
            2,
            "new velocity",
            size,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let readback = self.buffer(
            &mut cache,
            5,
            "fluid readback",
            self.readback_size(size),
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let (result, mut work) = self.run(
            &self.velocity,
            &[
                (&params_buffer, params.len() as u64),
                (&input, size),
                (&output, size),
            ],
            &output,
            size,
            &readback,
            ((u.len() + v.len()) as u32).div_ceil(128),
        )?;
        let values: Vec<_> = result
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f64::from(f32::from_le_bytes(*b)))
            .collect();
        if values.iter().any(|v| !v.is_finite()) {
            return Err(Error::Backend);
        }
        work.upload_bytes = params.len() as u64 + size;
        Ok(Velocity {
            u: values[..u.len()].to_vec(),
            v: values[u.len()..].to_vec(),
            work,
        })
    }
    pub(super) fn solve(
        &self,
        rows: &[([usize; 4], [f64; 4], f64)],
        rhs: &[f64],
        p: &mut [f64],
    ) -> Result<(usize, Work), Error> {
        let n = rows.len();
        let apply = |x: &[f64], r: &mut [f64]| {
            for (i, (adj, a, diag)) in rows.iter().enumerate() {
                r[i] = diag * x[i]
                    - (0..4)
                        .filter(|&k| adj[k] != usize::MAX)
                        .map(|k| a[k] * x[adj[k]])
                        .sum::<f64>();
            }
        };
        // Anchor only components without a free-surface pressure reference.
        // Keep the original operator for residual validation and physical faces.
        let mut anchors = vec![false; n];
        let mut visited = vec![false; n];
        for start in 0..n {
            if visited[start] {
                continue;
            }
            let mut stack = vec![start];
            visited[start] = true;
            let mut dirichlet = false;
            while let Some(i) = stack.pop() {
                let (adj, a, _) = &rows[i];
                for k in 0..4 {
                    if adj[k] == usize::MAX {
                        dirichlet |= a[k] > 0.;
                    } else if !visited[adj[k]] {
                        visited[adj[k]] = true;
                        stack.push(adj[k]);
                    }
                }
            }
            if !dirichlet {
                anchors[start] = true;
            }
        }
        let tolerance = (rhs.iter().map(|v| v * v).sum::<f64>().sqrt() * 1e-8).max(1e-9);
        let mut product = vec![0.; n];
        let mut iterations = 0;
        let mut total_work = self.empty_work();
        for _ in 0..5 {
            apply(p, &mut product);
            let residual: Vec<_> = rhs.iter().zip(&product).map(|(b, a)| b - a).collect();
            let norm = residual.iter().map(|v| v * v).sum::<f64>().sqrt();
            if norm <= tolerance {
                return Ok((iterations, total_work));
            }
            let scale = residual.iter().map(|v| v.abs()).fold(0., f64::max);
            if !scale.is_finite() || scale == 0. {
                return Err(Error::Pressure);
            }
            let mut data = Vec::with_capacity(n * 48);
            for (i, (adj, a, diag)) in rows.iter().enumerate() {
                for &j in adj {
                    let j = if anchors[i] || j == usize::MAX || anchors[j] {
                        u32::MAX
                    } else {
                        j as u32
                    };
                    data.extend_from_slice(&j.to_le_bytes());
                }
                for &x in a {
                    data.extend_from_slice(&(if anchors[i] { 0. } else { x as f32 }).to_le_bytes());
                }
                for x in [
                    if anchors[i] { 1. } else { *diag as f32 },
                    if anchors[i] {
                        0.
                    } else {
                        (residual[i] / scale) as f32
                    },
                    0.,
                    0.,
                ] {
                    data.extend_from_slice(&x.to_le_bytes());
                }
            }
            let mut cache = self.scratch.lock().map_err(|_| Error::Backend)?;
            let data_buffer = self.buffer(
                &mut cache,
                3,
                "pressure rows",
                data.len() as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            self.queue.write_buffer(&data_buffer, 0, &data);
            let size = (n * 20) as u64;
            // Kernel initializes every work entry. Only actual rows are bound,
            // so a growing buffer never exposes stale capacity to arrayLength.
            let work = self.buffer(
                &mut cache,
                4,
                "pressure work",
                size,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            let readback = self.buffer(
                &mut cache,
                5,
                "fluid readback",
                self.readback_size(size),
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            );
            let (result, mut work) = self.run(
                &self.pressure,
                &[(&data_buffer, data.len() as u64), (&work, size)],
                &work,
                size,
                &readback,
                1,
            )?;
            work.upload_bytes = data.len() as u64;
            total_work.add(work);
            for (i, bytes) in result.as_chunks::<20>().0.iter().enumerate() {
                let correction =
                    f64::from(f32::from_le_bytes(bytes[0..4].try_into().unwrap())) * scale;
                if !correction.is_finite() {
                    return Err(Error::Pressure);
                }
                p[i] += correction;
            }
            // Count actual iterations stored in the spare A*d slot of node 0.
            iterations += f32::from_le_bytes(result[16..20].try_into().unwrap()) as usize;
        }
        apply(p, &mut product);
        if rhs
            .iter()
            .zip(product)
            .map(|(b, a)| (b - a).powi(2))
            .sum::<f64>()
            .sqrt()
            <= tolerance
        {
            Ok((iterations, total_work))
        } else {
            Err(Error::Pressure)
        }
    }
}
