//! wgpu momentum/pressure compute. CPU f64 VOF remains the mass authority.
//! f32 pressure corrections are refined against the original f64 operator.
use crate::{Config, Error};
use std::{
    sync::{mpsc, Mutex},
    time::Duration,
};

mod preconditioner;
mod tiled;
const TILED_THRESHOLD: usize = 8192;
type Scratch = [Option<wgpu::Buffer>; 9];

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
    pub max_workgroups: u32,
}
impl Work {
    fn add(&mut self, other: Self) {
        self.kernel_ms = self.kernel_ms.zip(other.kernel_ms).map(|(a, b)| a + b);
        self.upload_bytes += other.upload_bytes;
        self.readback_bytes += other.readback_bytes;
        self.dispatches += other.dispatches;
        self.max_workgroups = self.max_workgroups.max(other.max_workgroups);
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
    tiled: tiled::TiledPressure,
    info: wgpu::AdapterInfo,
    scratch: Mutex<Scratch>,
    timer: Option<Timer>,
}
impl GpuContext {
    pub fn new() -> Result<Self, String> {
        Self::with_timing(true)
    }
    /// Unsupported timestamp queries leave kernel times unavailable; compute
    /// remains usable. Turning profiling off must not change physical state.
    pub fn with_timing(enabled: bool) -> Result<Self, String> {
        pollster::block_on(Self::create(enabled, None))
    }
    #[cfg(test)]
    pub(super) fn with_pressure_lanes(lanes: u32) -> Result<Self, String> {
        pollster::block_on(Self::create(true, Some(lanes)))
    }
    async fn create(timing: bool, pressure_lanes: Option<u32>) -> Result<Self, String> {
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
        let pipeline = |name, source: &'static str, constants: &[(&str, f64)]| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants,
                    ..Default::default()
                },
                cache: None,
            })
        };
        let velocity = pipeline("MAC advection", include_str!("velocity.wgsl"), &[]);
        // Software SIMD adapters pay heavily for cooperation across subgroups.
        // Keep the same GPU solver, but avoid scheduling 128 emulated lanes.
        // Hardware keeps a full workgroup; this is not a CPU solver fallback.
        let lanes = f64::from(pressure_lanes.unwrap_or(
            if info.device_type == wgpu::DeviceType::Cpu {
                4
            } else {
                128
            },
        ));
        let pressure = pipeline(
            "compact pressure CG",
            include_str!("pressure.wgsl"),
            &[("LANES", lanes)],
        );
        let tiled = tiled::TiledPressure::new(&device);
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
            tiled,
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
        cache: &mut Scratch,
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
        self.dispatch_read(output, size, readback, (1, groups), |pass| {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(groups, 1, 1);
        })
    }
    fn dispatch_read(
        &self,
        output: &wgpu::Buffer,
        size: u64,
        readback: &wgpu::Buffer,
        execution: (u32, u32),
        encode: impl FnOnce(&mut wgpu::ComputePass<'_>),
    ) -> Result<(Vec<u8>, Work), Error> {
        let (dispatches, max_workgroups) = execution;
        let mut encoder = self.device.create_command_encoder(&Default::default());
        if dispatches > 0 {
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
            encode(&mut pass);
        }
        encoder.copy_buffer_to_buffer(output, 0, readback, 0, size);
        let read_size = if dispatches > 0 {
            self.readback_size(size)
        } else {
            size
        };
        if let Some(timer) = self.timer.as_ref().filter(|_| dispatches > 0) {
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
                if dispatches == 0 {
                    return Some(0.);
                }
                let times = &bytes[read_size as usize - 16..];
                let begin = u64::from_le_bytes(times[..8].try_into().unwrap());
                let end = u64::from_le_bytes(times[8..].try_into().unwrap());
                end.checked_sub(begin)
                    .map(|delta| delta as f64 * f64::from(self.queue.get_timestamp_period()) * 1e-6)
            });
            let work = Work {
                kernel_ms,
                readback_bytes: read_size,
                dispatches,
                max_workgroups,
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
        // Identify closed components for gauge handling. Keep the original
        // operator for f64 residual validation and physical face corrections.
        let mut anchors = vec![false; n];
        let mut visited = vec![false; n];
        let mut closed_anchor = vec![usize::MAX; n];
        for start in 0..n {
            if visited[start] {
                continue;
            }
            let mut stack = vec![start];
            visited[start] = true;
            let mut dirichlet = false;
            let mut component = Vec::new();
            while let Some(i) = stack.pop() {
                component.push(i);
                let (adj, a, diagonal) = &rows[i];
                dirichlet |= *diagonal > a.iter().sum::<f64>() + 1e-12 * diagonal.abs().max(1e-20);
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
                let gauge = p[start];
                for i in component {
                    if n >= TILED_THRESHOLD {
                        p[i] -= gauge;
                    }
                    closed_anchor[i] = start;
                }
            }
        }
        let tolerance = (rhs.iter().map(|v| v * v).sum::<f64>().sqrt() * 1e-8).max(1e-9);
        let mut product = vec![0.; n];
        let mut iterations = 0;
        let mut total_work = self.empty_work();
        let mut preconditioner_data = None;
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
            // A point-pinned Neumann system becomes unnecessarily ill
            // conditioned at large sizes. Distributed CG keeps the compatible
            // semidefinite operator; remove each component's arbitrary constant
            // from the correction on readback. The f64 residual remains final.
            let pin = n < TILED_THRESHOLD;
            let mut right: Vec<f32> = residual.iter().map(|r| (r / scale) as f32).collect();
            if !pin {
                let mut sums = vec![0.; n];
                for (i, &anchor) in closed_anchor.iter().enumerate() {
                    if anchor != usize::MAX && i != anchor {
                        sums[anchor] += f64::from(right[i]);
                    }
                }
                for (i, &anchor) in anchors.iter().enumerate() {
                    if anchor {
                        right[i] = -sums[i] as f32;
                    }
                }
            }
            let mut data = Vec::with_capacity(n * 48);
            for (i, (adj, a, diag)) in rows.iter().enumerate() {
                for &j in adj {
                    let j = if pin && anchors[i] || j == usize::MAX || pin && anchors[j] {
                        u32::MAX
                    } else {
                        j as u32
                    };
                    data.extend_from_slice(&j.to_le_bytes());
                }
                for &x in a {
                    data.extend_from_slice(
                        &(if pin && anchors[i] { 0. } else { x as f32 }).to_le_bytes(),
                    );
                }
                for x in [
                    if pin && anchors[i] { 1. } else { *diag as f32 },
                    if pin && anchors[i] { 0. } else { right[i] },
                    if pin {
                        0.
                    } else {
                        (*diag - a.iter().sum::<f64>()) as f32
                    },
                    0.,
                ] {
                    data.extend_from_slice(&x.to_le_bytes());
                }
            }
            if n >= TILED_THRESHOLD {
                let (correction, count, work) = self.solve_tiled(&data, n)?;
                for (i, value) in p.iter_mut().enumerate() {
                    let anchor = closed_anchor[i];
                    let gauge = if anchor == usize::MAX {
                        0.
                    } else {
                        correction[anchor]
                    };
                    *value += (correction[i] - gauge) * scale;
                }
                iterations += count;
                total_work.add(work);
                continue;
            }
            let mut cache = self.scratch.lock().map_err(|_| Error::Backend)?;
            let first = preconditioner_data.is_none();
            if first {
                preconditioner_data = Some(preconditioner::build(rows, &anchors)?);
            }
            let preconditioner_data = preconditioner_data.as_ref().unwrap();
            let preconditioner = self.buffer(
                &mut cache,
                8,
                "two-level pressure preconditioner",
                preconditioner_data.len() as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            if first {
                self.queue
                    .write_buffer(&preconditioner, 0, preconditioner_data);
            }
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
                &[
                    (&data_buffer, data.len() as u64),
                    (&work, size),
                    (&preconditioner, preconditioner_data.len() as u64),
                ],
                &work,
                size,
                &readback,
                1,
            )?;
            work.upload_bytes = data.len() as u64
                + if first {
                    preconditioner_data.len() as u64
                } else {
                    0
                };
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
