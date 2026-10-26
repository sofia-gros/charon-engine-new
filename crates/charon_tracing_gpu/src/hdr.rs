use super::*;
use charon_tracing::{HdrField, hdr::invalidation};
use std::time::Instant;
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct HdrParams {
    extent: [u32; 4],
    budget: [u32; 4],
    geometry: [f32; 4],
    sky_falloff: [f32; 4],
    ambient: [f32; 4],
    quality: [u32; 4],
    work: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct HdrOptical {
    transport: [f32; 4],
    source: [f32; 4],
    diffuse: [f32; 4],
    material: [f32; 4],
    absorption: [f32; 4],
    normal: [f32; 4],
    emission: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Emitter {
    index: u32,
    cdf: f32,
    pdf: f32,
    padding: f32,
}
/// GPU times are populated only when timestamp queries are available. CPU submit
/// and mesh preparation time are explicitly separate from GPU execution time.
#[derive(Clone, Copy, Debug, Default)]
pub struct TraceMetrics {
    pub submit_ms: f64,
    pub mesh_ms: f64,
    pub gpu_ms: Option<f64>,
    pub estimated_bytes: u64,
    pub faces: u32,
}
pub fn hdr_shader(mode: Mode) -> String {
    let common = include_str!("hdr.wgsl");
    if mode == Mode::Rtx {
        format!(
            "{}\n{}",
            common.replace("const USE_RTX:bool = false;", "const USE_RTX:bool = true;"),
            include_str!("hdr_rtx.wgsl")
        )
    } else {
        format!(
            "{common}\nfn hardware_hit(p:vec3<f32>,d:vec3<f32>,range:f32)->Hit {{ return empty_hit(p,range); }}"
        )
    }
}
struct Timer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    state: std::sync::Arc<std::sync::atomic::AtomicU8>,
}
impl Timer {
    fn new(device: &wgpu::Device) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        Some(Self {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("HDR GPU timing"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("HDR timestamp resolve"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("HDR asynchronous timing readback"),
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            state: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0)),
        })
    }
    fn ready(&self) -> bool {
        self.state.load(std::sync::atomic::Ordering::Acquire) == 0
    }
    fn collect(&self, device: &wgpu::Device, period: f32) -> Option<f64> {
        let _ = device.poll(wgpu::PollType::Poll);
        if self.state.load(std::sync::atomic::Ordering::Acquire) != 2 {
            return None;
        }
        let data = self.readback.slice(..).get_mapped_range();
        let t = bytemuck::cast_slice::<u8, u64>(&data);
        let ms = t[1].saturating_sub(t[0]) as f64 * period as f64 / 1_000_000.;
        drop(data);
        self.readback.unmap();
        self.state.store(0, std::sync::atomic::Ordering::Release);
        Some(ms)
    }
    fn map(&self) {
        self.state.store(1, std::sync::atomic::Ordering::Release);
        let state = self.state.clone();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                state.store(
                    if r.is_ok() { 2 } else { 3 },
                    std::sync::atomic::Ordering::Release,
                )
            });
    }
}
pub(crate) struct HdrGpu {
    timer: Option<Timer>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    geometry: Geometry,
    mode: Mode,
    quality: Quality,
    params: wgpu::Buffer,
    optics: wgpu::Buffer,
    emitters: wgpu::Buffer,
    _history: wgpu::Buffer,
    result: wgpu::Buffer,
    dirty: wgpu::Buffer,
    output: wgpu::Texture,
    pipeline: wgpu::ComputePipeline,
    resolve: wgpu::ComputePipeline,
    group: wgpu::BindGroup,
    resolve_group: wgpu::BindGroup,
    last: Option<TraceScene>,
    samples: u32,
    frame: u32,
    metrics: TraceMetrics,
    #[cfg(feature = "rtx")]
    acceleration: Option<rtx::Acceleration>,
}
impl HdrGpu {
    pub async fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        geometry: Geometry,
        mode: Mode,
        quality: Quality,
    ) -> Result<Self, String> {
        let n = geometry.len() as u64;
        let limits = device.limits();
        if geometry
            .width
            .checked_mul(4)
            .is_none_or(|w| w > limits.max_texture_dimension_2d)
            || n * 112 > limits.max_storage_buffer_binding_size as u64
        {
            return Err("HDR trace geometry exceeds device limits".into());
        }
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let params = buffer(
            "HDR trace parameters",
            112,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let optics = buffer("HDR world materials", n * 112, storage);
        let emitters = buffer("HDR power-weighted area emitters", n * 16, storage);
        let history = buffer("HDR indirect history", n * 16, storage);
        let result = buffer(
            "HDR separated components",
            n * 64,
            storage | wgpu::BufferUsages::COPY_SRC,
        );
        let dirty = buffer("conservative world influence mask", n * 4, storage);
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("HDR diffuse/specular/emission/guide atlas"),
            size: wgpu::Extent3d {
                width: geometry.width * 4,
                height: geometry.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("HDR prism transport"),
            source: wgpu::ShaderSource::Wgsl(hdr_shader(mode).into()),
        });
        let make_pipeline = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pipeline = make_pipeline("trace");
        let resolve = make_pipeline("resolve");
        let view = output.create_view(&Default::default());
        fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            }
        }
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("HDR trace bindings"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                entry(0, &params),
                entry(1, &optics),
                entry(2, &emitters),
                entry(3, &history),
                entry(4, &result),
                entry(6, &dirty),
            ],
        });
        let resolve_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("HDR spatial resolve bindings"),
            layout: &resolve.get_bind_group_layout(0),
            entries: &[
                entry(0, &params),
                entry(1, &optics),
                entry(4, &result),
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        if let Some(e) = device.pop_error_scope().await {
            return Err(e.to_string());
        }
        let timer = Timer::new(&device);
        Ok(Self {
            timer,
            device,
            queue,
            geometry,
            mode,
            quality,
            params,
            optics,
            emitters,
            _history: history,
            result,
            dirty,
            output,
            pipeline,
            resolve,
            group,
            resolve_group,
            last: None,
            samples: 0,
            frame: 0,
            metrics: TraceMetrics {
                estimated_bytes: n * (112 + 16 + 16 + 64 + 4 + 32),
                ..Default::default()
            },
            #[cfg(feature = "rtx")]
            acceleration: None,
        })
    }
    pub fn device(&self) -> LightingDevice {
        LightingDevice::new(self.device.clone(), self.queue.clone())
    }
    pub fn mode(&self) -> Mode {
        self.mode
    }
    pub fn texture(&self) -> &wgpu::Texture {
        &self.output
    }
    pub fn samples(&self) -> u32 {
        self.samples
    }
    pub fn reset(&mut self) {
        self.last = None;
        self.samples = 0;
        self.frame = 0;
    }
    pub fn metrics(&self) -> TraceMetrics {
        self.metrics
    }
    pub fn render(&mut self, s: &TraceScene) -> Result<bool, String> {
        let start = Instant::now();
        s.validate()?;
        if let Some(timer) = &self.timer {
            if let Some(ms) = timer.collect(&self.device, self.queue.get_timestamp_period()) {
                self.metrics.gpu_ms = Some(ms);
            }
        }
        if s.input.geometry != self.geometry {
            return Err("HDR geometry mismatch".into());
        }
        let changed = self.last.as_ref().is_none_or(|old| !old.same_input(s));
        if !changed && self.samples >= self.quality.max_samples {
            return Ok(false);
        }
        let mask = invalidation(self.last.as_ref(), s);
        if changed {
            self.samples = 0;
        }
        self.queue
            .write_buffer(&self.dirty, 0, bytemuck::cast_slice(&mask));
        let emitters = s.emitters();
        if changed {
            let values: Vec<_> = s
                .input
                .optics
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    let m = s.materials[i];
                    let radiance = s.radiance(i);
                    HdrOptical {
                        transport: [
                            o.transmission[0],
                            o.transmission[1],
                            o.transmission[2],
                            o.opacity,
                        ],
                        source: [
                            radiance[0],
                            radiance[1],
                            radiance[2],
                            if o.emission_strength > 0. || m.emission.iter().any(|v| *v > 0.) {
                                self.geometry.cell_size * self.geometry.cell_size
                            } else {
                                64.
                            },
                        ],
                        diffuse: [
                            m.diffuse[0] * s.reflectance[i][0],
                            m.diffuse[1] * s.reflectance[i][1],
                            m.diffuse[2] * s.reflectance[i][2],
                            0.,
                        ],
                        material: [m.roughness, m.specular, m.ior, m.depth],
                        absorption: [
                            m.absorption[0],
                            m.absorption[1],
                            m.absorption[2],
                            f32::from(m.dielectric),
                        ],
                        normal: [m.normal[0], m.normal[1], m.normal[2], f32::from(m.dynamic)],
                        emission: [m.emission[0], m.emission[1], m.emission[2], 0.],
                    }
                })
                .collect();
            self.queue
                .write_buffer(&self.optics, 0, bytemuck::cast_slice(&values));
            if !emitters.is_empty() {
                self.queue.write_buffer(
                    &self.emitters,
                    0,
                    bytemuck::cast_slice(
                        &charon_tracing::hdr::emitter_distribution(s, &emitters)
                            .into_iter()
                            .map(|(index, cdf, pdf)| Emitter {
                                index,
                                cdf,
                                pdf,
                                padding: 0.,
                            })
                            .collect::<Vec<_>>(),
                    ),
                );
            }
        }
        let g = self.geometry;
        let settings = s.input.settings;
        let sq = s.surface_quality;
        let count = self.quality.samples_per_update;
        let mut params = HdrParams {
            extent: [g.width, g.height, count, self.quality.bounces],
            budget: [
                self.frame,
                self.quality.max_samples,
                self.quality.max_steps,
                emitters.len() as u32,
            ],
            geometry: [g.origin[0], g.origin[1], g.cell_size, settings.max_distance],
            sky_falloff: [
                settings.sky[0],
                settings.sky[1],
                settings.sky[2],
                settings.falloff_per_unit,
            ],
            ambient: [
                settings.ambient[0],
                settings.ambient[1],
                settings.ambient[2],
                sq.indirect_clamp,
            ],
            work: [
                0,
                s.materials
                    .iter()
                    .map(|m| m.depth)
                    .fold(0_f32, f32::max)
                    .to_bits(),
                0,
                0,
            ],
            quality: [
                sq.reflection_bounces,
                sq.transmission_bounces,
                sq.denoise_radius,
                sq.dynamic_history,
            ],
        };
        #[allow(unused_mut)] // Mutable only when the optional RTX mesh is built.
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("HDR world tracing and denoise"),
            });
        #[cfg(feature = "rtx")]
        if self.mode == Mode::Rtx {
            let rebuild = self.last.as_ref().is_none_or(|old| {
                old.input
                    .optics
                    .iter()
                    .zip(&s.input.optics)
                    .enumerate()
                    .any(|(i, (a, b))| {
                        (a.opacity >= 0.999) != (b.opacity >= 0.999)
                            || old.materials[i].dielectric != s.materials[i].dielectric
                            || old.materials[i].depth != s.materials[i].depth
                            || old.materials[i].ior != s.materials[i].ior
                            || old.materials[i].emission.iter().any(|v| *v > 0.)
                                != s.materials[i].emission.iter().any(|v| *v > 0.)
                    })
            });
            if rebuild || self.acceleration.is_none() {
                let start = Instant::now();
                let mesh = rtx::surface_mesh(s)?;
                self.metrics.faces = mesh.1.len() as u32;
                self.metrics.estimated_bytes = self.geometry.len() as u64
                    * (112 + 16 + 16 + 64 + 4 + 32)
                    + self.metrics.faces as u64 * 88;
                self.acceleration = Some(rtx::Acceleration::build_mesh(
                    &self.device,
                    &mut encoder,
                    &self.pipeline,
                    mesh,
                )?);
                self.metrics.mesh_ms = start.elapsed().as_secs_f64() * 1000.;
            } else {
                self.metrics.mesh_ms = 0.;
            }
        }
        let timed = self.timer.as_ref().is_some_and(|t| t.ready());
        self.queue.submit([encoder.finish()]);
        // Separate submissions bound each expensive dispatch, rather than one
        // full-world ray-query workload risking the Windows GPU watchdog.
        const ROWS: u32 = 8;
        for row in (0..g.height).step_by(ROWS as usize) {
            params.work[0] = row;
            self.queue
                .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("bounded HDR tracing stripe"),
                });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("HDR direct, indirect, reflection and refraction"),
                    timestamp_writes: if timed && row == 0 {
                        Some(wgpu::ComputePassTimestampWrites {
                            query_set: &self.timer.as_ref().unwrap().queries,
                            beginning_of_pass_write_index: Some(0),
                            end_of_pass_write_index: None,
                        })
                    } else {
                        None
                    },
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.group, &[]);
                #[cfg(feature = "rtx")]
                if let Some(acceleration) = &self.acceleration {
                    pass.set_bind_group(1, &acceleration.group, &[]);
                }
                pass.dispatch_workgroups(
                    g.width.div_ceil(8),
                    (g.height - row).min(ROWS).div_ceil(8),
                    1,
                );
            }
            self.queue.submit([encoder.finish()]);
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("HDR completed-world resolve"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("material-aware HDR reconstruction"),
                timestamp_writes: if timed {
                    Some(wgpu::ComputePassTimestampWrites {
                        query_set: &self.timer.as_ref().unwrap().queries,
                        beginning_of_pass_write_index: None,
                        end_of_pass_write_index: Some(1),
                    })
                } else {
                    None
                },
            });
            pass.set_pipeline(&self.resolve);
            pass.set_bind_group(0, &self.resolve_group, &[]);
            pass.dispatch_workgroups(g.width.div_ceil(8), g.height.div_ceil(8), 1);
        }
        if timed {
            let timer = self.timer.as_ref().unwrap();
            encoder.resolve_query_set(&timer.queries, 0..2, &timer.resolve, 0);
            encoder.copy_buffer_to_buffer(&timer.resolve, 0, &timer.readback, 0, 16);
        }
        self.queue.submit([encoder.finish()]);
        if timed {
            self.timer.as_ref().unwrap().map();
        }
        self.samples = (self.samples + count).min(self.quality.max_samples);
        self.frame = self.frame.wrapping_add(1);
        self.last = Some(s.clone());
        self.metrics.submit_ms = start.elapsed().as_secs_f64() * 1000.;
        Ok(true)
    }
    pub fn readback(&self) -> Result<HdrField, String> {
        let s = self.last.as_ref().ok_or("no HDR field available")?;
        let n = self.geometry.len() as u64;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("explicit HDR diagnostic readback"),
            size: n * 64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.result, 0, &staging, 0, n * 64);
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::Wait)
            .map_err(|e| e.to_string())?;
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let mapped = staging.slice(..).get_mapped_range();
        let values = bytemuck::cast_slice::<u8, [[f32; 4]; 4]>(&mapped);
        let component = |k: usize| values.iter().map(|v| [v[k][0], v[k][1], v[k][2]]).collect();
        let field = HdrField {
            geometry: self.geometry,
            revision: s.input.revision,
            diffuse: component(0),
            specular: component(1),
            emission: component(2),
        };
        drop(mapped);
        staging.unmap();
        Ok(field)
    }
}
