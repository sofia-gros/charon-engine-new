//! Independent GPU tracing. The `rtx` feature provides experimental Vulkan ray
//! queries; portable path modes never request hardware ray tracing features.
use charon_tracing::charon_lighting::{Geometry, LightField};
pub use charon_tracing::{Mode, Platform, Quality, TraceScene};
#[cfg(feature = "plugin")]
mod plugin;
#[cfg(feature = "rtx")]
mod rtx;
#[cfg(feature = "plugin")]
pub use plugin::{SwitchRequest, Tracing, TracingPlugin, TracingSlot};
mod device;
mod prepare;
#[cfg(feature = "plugin")]
mod worker;
pub use device::{DeviceCapabilities, LightingDevice};
pub use wgpu;
#[cfg(feature = "plugin")]
pub use worker::TracingWorker;
pub const COMMON: &str = include_str!("trace.wgsl");
pub fn shader(mode: Mode) -> String {
    if mode == Mode::Rtx {
        format!(
            "{}\n{}",
            COMMON.replace("const USE_RTX:bool = false;", "const USE_RTX:bool = true;"),
            include_str!("rtx.wgsl")
        )
    } else {
        format!(
            "{COMMON}\nfn opaque_hit(p:vec2<f32>,d:vec2<f32>,range:f32)->Hit {{return empty_hit(p,range);}}"
        )
    }
}
pub fn rtx_features() -> wgpu::Features {
    wgpu::Features::EXPERIMENTAL_RAY_QUERY
        | wgpu::Features::EXPERIMENTAL_RAY_TRACING_ACCELERATION_STRUCTURE
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    extent: [u32; 4],
    budget: [u32; 4],
    geometry: [f32; 4],
    sky_falloff: [f32; 4],
    ambient: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Optical {
    transport: [f32; 4],
    source: [f32; 4],
    surface: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct EmitterGpu {
    position: [f32; 4],
    color: [f32; 4],
}
pub struct GpuTrace {
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
    output: wgpu::Texture,
    pipeline: wgpu::ComputePipeline,
    resolve: wgpu::ComputePipeline,
    _raw: wgpu::Buffer,
    group: wgpu::BindGroup,
    samples: u32,
    last: Option<TraceScene>,
    cpu_pool: Option<std::sync::Arc<charon_tasks::TaskPool>>,
    #[cfg(feature = "rtx")]
    acceleration: Option<rtx::Acceleration>,
}
impl GpuTrace {
    pub async fn headless(
        geometry: Geometry,
        mode: Mode,
        platform: Platform,
        quality: Quality,
    ) -> Result<(Self, String), String> {
        quality.validate(mode, platform)?;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: if mode == Mode::Rtx {
                wgpu::Backends::VULKAN
            } else {
                wgpu::Backends::all()
            },
            ..Default::default()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        if mode == Mode::Rtx
            && matches!(
                adapter.get_info().device_type,
                wgpu::DeviceType::Cpu | wgpu::DeviceType::Other
            )
        {
            return Err("hardware RTX mode cannot use a CPU/software adapter".into());
        }
        let features = if mode == Mode::Rtx {
            rtx_features()
        } else {
            wgpu::Features::empty()
        };
        if !adapter.features().contains(features) {
            return Err(
                "RTX mode unavailable: Vulkan hardware ray query/acceleration features missing"
                    .into(),
            );
        }
        let mut limits = wgpu::Limits::default();
        if mode == Mode::Rtx {
            limits = limits.using_minimum_supported_acceleration_structure_values();
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: features,
                required_limits: limits,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        let name = adapter.get_info().name;
        Ok((
            Self::new(device, queue, geometry, mode, platform, quality).await?,
            name,
        ))
    }
    pub async fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        geometry: Geometry,
        mode: Mode,
        platform: Platform,
        quality: Quality,
    ) -> Result<Self, String> {
        geometry.validate()?;
        quality.validate(mode, platform)?;
        if mode == Mode::Rtx
            && (!cfg!(feature = "rtx") || !device.features().contains(rtx_features()))
        {
            return Err(
                "RTX requires the rtx feature and a ray-query-enabled Vulkan device".into(),
            );
        }
        let n = geometry.len() as u64;
        let limits = device.limits();
        if n * 48 > limits.max_storage_buffer_binding_size as u64
            || geometry.width > limits.max_texture_dimension_2d
            || geometry.height > limits.max_texture_dimension_2d
        {
            return Err("trace geometry exceeds device limits".into());
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
        let optics = buffer("trace optical inputs", n * 48, storage);
        let emitters = buffer("deterministic trace emitters", 64 * 32, storage);
        let raw = buffer("separate direct and indirect illumination", n * 32, storage);
        let history = buffer("trace radiance accumulation", n * 16, storage);
        let result = buffer(
            "trace result",
            n * 16,
            storage | wgpu::BufferUsages::COPY_SRC,
        );
        let params = buffer(
            "trace parameters",
            80,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("traced illumination"),
            size: wgpu::Extent3d {
                width: geometry.width,
                height: geometry.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("2D path tracing"),
            source: wgpu::ShaderSource::Wgsl(shader(mode).into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let storage_type = |read_only| wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let data_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trace/resolve shared data"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(1, storage_type(true)),
                entry(2, storage_type(true)),
                entry(3, storage_type(false)),
                entry(4, storage_type(false)),
                entry(
                    5,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                ),
                entry(6, storage_type(false)),
            ],
        });
        let ray_layout = (mode == Mode::Rtx).then(|| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ray query acceleration"),
                entries: &[
                    entry(
                        0,
                        wgpu::BindingType::AccelerationStructure {
                            vertex_return: false,
                        },
                    ),
                    entry(1, storage_type(true)),
                ],
            })
        });
        let mut layouts = vec![&data_layout];
        if let Some(ray) = &ray_layout {
            layouts.push(ray);
        }
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trace/resolve layout"),
            bind_group_layouts: &layouts,
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("2D diffuse path tracer"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("trace"),
            compilation_options: Default::default(),
            cache: None,
        });
        let resolve = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("edge-aware indirect resolve"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("resolve"),
            compilation_options: Default::default(),
            cache: None,
        });
        let view = output.create_view(&Default::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trace data"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: optics.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: emitters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: history.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: result.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: raw.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        if let Some(e) = device.pop_error_scope().await {
            return Err(e.to_string());
        }
        Ok(Self {
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
            output,
            pipeline,
            resolve,
            _raw: raw,
            group,
            samples: 0,
            last: None,
            cpu_pool: None,
            #[cfg(feature = "rtx")]
            acceleration: None,
        })
    }
    pub fn set_cpu_pool(&mut self, pool: std::sync::Arc<charon_tasks::TaskPool>) {
        self.cpu_pool = Some(pool);
    }
    pub fn device(&self) -> LightingDevice {
        LightingDevice::new(self.device.clone(), self.queue.clone())
    }
    pub fn readback_geometry(&self) -> Geometry {
        self.geometry
    }
    pub fn texture(&self) -> &wgpu::Texture {
        &self.output
    }
    pub fn samples(&self) -> u32 {
        self.samples
    }
    pub fn mode(&self) -> Mode {
        self.mode
    }
    pub fn reset(&mut self) {
        self.last = None;
        self.samples = 0;
    }
    pub fn render(&mut self, scene: &TraceScene) -> Result<bool, String> {
        scene.validate()?;
        if scene.input.geometry != self.geometry {
            return Err("tracer geometry is immutable; recreate and resubmit inputs".into());
        }
        let changed = self.last.as_ref().is_none_or(|old| !old.same_input(scene));
        if changed {
            self.samples = 0;
        } else if let Some(last) = self.last.as_mut() {
            last.input.revision = scene.input.revision;
        }
        let count = self
            .quality
            .samples_per_update
            .min(self.quality.max_samples - self.samples);
        if count == 0 {
            if let Some(last) = self.last.as_mut() {
                last.input.revision = scene.input.revision;
            }
            return Ok(false);
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("tracing independent from drawing"),
            });
        #[cfg(feature = "rtx")]
        if self.mode == Mode::Rtx {
            let rebuild = self.last.as_ref().is_none_or(|old| {
                old.input
                    .optics
                    .iter()
                    .zip(&scene.input.optics)
                    .any(|(a, b)| (a.opacity >= 0.999) != (b.opacity >= 0.999))
            });
            if rebuild || self.acceleration.is_none() {
                self.acceleration = Some(rtx::Acceleration::build(
                    &self.device,
                    &mut encoder,
                    scene,
                    &self.pipeline,
                )?);
            }
        }
        let emitters = scene.emitters();
        if changed {
            let values = prepare::optics(scene, self.cpu_pool.as_ref())?;
            self.queue
                .write_buffer(&self.optics, 0, bytemuck::cast_slice(&values));
            if !emitters.is_empty() {
                let lights: Vec<_> = emitters
                    .iter()
                    .map(|e| EmitterGpu {
                        position: [e.position[0], e.position[1], 0., 0.],
                        color: [e.radiance[0], e.radiance[1], e.radiance[2], 0.],
                    })
                    .collect();
                self.queue
                    .write_buffer(&self.emitters, 0, bytemuck::cast_slice(&lights));
            }
        }
        let s = scene.input.settings;
        let g = self.geometry;
        let params =
            Params {
                extent: [g.width, g.height, count, self.quality.bounces],
                budget: [
                    self.samples,
                    u32::from(scene.input.optics.iter().any(|o| {
                        o.opacity < 0.999 && (o.opacity > 0. || o.transmission != [1.; 3])
                    })),
                    self.quality.max_steps,
                    emitters.len() as u32,
                ],
                geometry: [g.origin[0], g.origin[1], g.cell_size, s.max_distance],
                sky_falloff: [s.sky[0], s.sky[1], s.sky[2], s.falloff_per_unit],
                ambient: [s.ambient[0], s.ambient[1], s.ambient[2], 0.],
            };
        self.queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sample and accumulate paths"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.group, &[]);
            #[cfg(feature = "rtx")]
            if let Some(acceleration) = &self.acceleration {
                pass.set_bind_group(1, &acceleration.group, &[]);
            }
            pass.dispatch_workgroups(g.width.div_ceil(8), g.height.div_ceil(8), 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("resolve deterministic direct and filtered indirect light"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.resolve);
            pass.set_bind_group(0, &self.group, &[]);
            #[cfg(feature = "rtx")]
            if let Some(acceleration) = &self.acceleration {
                pass.set_bind_group(1, &acceleration.group, &[]);
            }
            pass.dispatch_workgroups(g.width.div_ceil(8), g.height.div_ceil(8), 1);
        }
        self.queue.submit([encoder.finish()]);
        self.samples += count;
        if changed {
            self.last = Some(scene.clone());
        }
        Ok(true)
    }
    pub fn readback(&self) -> Result<LightField, String> {
        if self.samples == 0 {
            return Err("no traced field available".into());
        }
        let n = self.geometry.len() as u64;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("explicit trace readback"),
            size: n * 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.result, 0, &staging, 0, n * 16);
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
        let rgba = bytemuck::cast_slice::<u8, [f32; 4]>(&mapped).to_vec();
        drop(mapped);
        staging.unmap();
        Ok(LightField {
            geometry: self.geometry,
            revision: self.last.as_ref().unwrap().input.revision,
            rgba,
        })
    }
}
