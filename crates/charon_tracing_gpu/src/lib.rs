//! Independent GPU tracing. The `rtx` feature provides experimental Vulkan ray
//! queries; portable path modes never request hardware ray tracing features.
use charon_tracing::charon_lighting::{Geometry, LightField, ambient_occlusion};
pub use charon_tracing::{Mode, Platform, Quality, TraceScene};
#[cfg(feature = "plugin")]
mod plugin;
#[cfg(feature = "rtx")]
mod rtx;
#[cfg(feature = "plugin")]
pub use plugin::{SwitchRequest, Tracing, TracingPlugin, TracingSlot};
mod device;
mod hdr;
pub use device::{DeviceCapabilities, LightingDevice};
pub use hdr::{TraceMetrics, hdr_shader};
pub use wgpu;
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
struct LegacyTrace {
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
    group: wgpu::BindGroup,
    samples: u32,
    last: Option<TraceScene>,
    #[cfg(feature = "rtx")]
    acceleration: Option<rtx::Acceleration>,
}
impl LegacyTrace {
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
        let emitters = buffer("trace emission indices", n * 4, storage);
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
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("2D diffuse path tracer"),
            layout: None,
            module: &module,
            entry_point: Some("trace"),
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
            group,
            samples: 0,
            last: None,
            #[cfg(feature = "rtx")]
            acceleration: None,
        })
    }
    pub fn device(&self) -> LightingDevice {
        LightingDevice::new(self.device.clone(), self.queue.clone())
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
        }
        let count = self
            .quality
            .samples_per_update
            .min(self.quality.max_samples - self.samples);
        if count == 0 {
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
            let ao = ambient_occlusion(
                self.geometry,
                &scene.input.optics,
                scene.input.settings.ao_strength,
            )?;
            let values: Vec<_> = scene
                .input
                .optics
                .iter()
                .enumerate()
                .map(|(i, o)| Optical {
                    transport: [
                        o.transmission[0],
                        o.transmission[1],
                        o.transmission[2],
                        o.opacity,
                    ],
                    source: [
                        scene.input.sources[i][0],
                        scene.input.sources[i][1],
                        scene.input.sources[i][2],
                        0.,
                    ],
                    surface: [
                        scene.reflectance[i][0],
                        scene.reflectance[i][1],
                        scene.reflectance[i][2],
                        ao[i],
                    ],
                })
                .collect();
            self.queue
                .write_buffer(&self.optics, 0, bytemuck::cast_slice(&values));
            if !emitters.is_empty() {
                self.queue
                    .write_buffer(&self.emitters, 0, bytemuck::cast_slice(&emitters));
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

/// Portable Lite keeps its original numerical and rendering contract. Path/RTX
/// own only HDR resources; switching never allocates a hidden legacy tracer.
pub struct GpuTrace {
    in_flight: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    geometry: Geometry,
    backend: TraceBackend,
}
enum TraceBackend {
    Legacy(LegacyTrace),
    Hdr(hdr::HdrGpu),
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
                required_features: features
                    | (adapter.features() & wgpu::Features::TIMESTAMP_QUERY),
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
            return Err("RTX requires a ray-query-enabled Vulkan device".into());
        }
        let backend = if matches!(mode, Mode::Path | Mode::Rtx) {
            TraceBackend::Hdr(hdr::HdrGpu::new(device, queue, geometry, mode, quality).await?)
        } else {
            TraceBackend::Legacy(
                LegacyTrace::new(device, queue, geometry, mode, platform, quality).await?,
            )
        };
        Ok(Self {
            geometry,
            backend,
            in_flight: Default::default(),
        })
    }
    pub fn device(&self) -> LightingDevice {
        match &self.backend {
            TraceBackend::Legacy(l) => l.device(),
            TraceBackend::Hdr(h) => h.device(),
        }
    }
    pub fn readback_geometry(&self) -> Geometry {
        self.geometry
    }
    /// Lite: illumination image. Path/RTX: four horizontal HDR component planes;
    /// bind with charon_lighting_render::HDR_SHADER, not the legacy compositor.
    pub fn texture(&self) -> &wgpu::Texture {
        match &self.backend {
            TraceBackend::Legacy(l) => l.texture(),
            TraceBackend::Hdr(h) => h.texture(),
        }
    }
    pub fn samples(&self) -> u32 {
        match &self.backend {
            TraceBackend::Legacy(l) => l.samples(),
            TraceBackend::Hdr(h) => h.samples(),
        }
    }
    pub fn mode(&self) -> Mode {
        match &self.backend {
            TraceBackend::Legacy(l) => l.mode(),
            TraceBackend::Hdr(h) => h.mode(),
        }
    }
    pub fn reset(&mut self) {
        match &mut self.backend {
            TraceBackend::Legacy(l) => l.reset(),
            TraceBackend::Hdr(h) => h.reset(),
        }
    }
    /// Nonblocking readiness for real-time callers. Explicit/offline render calls
    /// may still enqueue consecutive updates for reproducible sample counts.
    pub fn is_idle(&self) -> Result<bool, String> {
        self.device()
            .raw_device()
            .poll(wgpu::PollType::Poll)
            .map_err(|e| e.to_string())?;
        Ok(self.in_flight.load(std::sync::atomic::Ordering::Acquire) == 0)
    }
    pub fn render(&mut self, s: &TraceScene) -> Result<bool, String> {
        use std::sync::atomic::Ordering;
        let device = self.device();
        let submitted = match &mut self.backend {
            TraceBackend::Legacy(l) => l.render(s),
            TraceBackend::Hdr(h) => h.render(s),
        }?;
        if submitted {
            self.in_flight.fetch_add(1, Ordering::AcqRel);
            let pending = self.in_flight.clone();
            device.raw_queue().on_submitted_work_done(move || {
                pending.fetch_sub(1, Ordering::AcqRel);
            });
        }
        Ok(submitted)
    }
    pub fn readback(&self) -> Result<LightField, String> {
        match &self.backend {
            TraceBackend::Legacy(l) => l.readback(),
            TraceBackend::Hdr(h) => Ok(h.readback()?.light_field()),
        }
    }
    pub fn readback_hdr(&self) -> Result<charon_tracing::HdrField, String> {
        match &self.backend {
            TraceBackend::Hdr(h) => h.readback(),
            _ => Err("HDR is available only in Path/RTX modes".into()),
        }
    }
    pub fn metrics(&self) -> TraceMetrics {
        match &self.backend {
            TraceBackend::Hdr(h) => h.metrics(),
            _ => TraceMetrics::default(),
        }
    }
}
