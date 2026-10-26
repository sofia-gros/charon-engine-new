#[path = "hdr.rs"]
mod hdr;
use super::*;
use charon_assets::{Image, ShaderSource};
pub use hdr::{HdrPost, HdrSettings};
use std::{collections::HashMap, sync::Arc};
use wgpu::util::DeviceExt;
use winit::window::Window;
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    tint: [f32; 4],
}
struct GpuTexture {
    _texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    size: [u32; 2],
    cpu_writable: bool,
}
struct Pass {
    id: PassId,
    pipeline: wgpu::RenderPipeline,
    hdr_pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    sampled: Option<TextureId>,
    parameters: [f32; 4],
}
pub struct GpuRenderer {
    hdr: Option<hdr::HdrPost>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    textures: HashMap<TextureId, GpuTexture>,
    passes: Vec<Pass>,
    texture_layout: wgpu::BindGroupLayout,
    params_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    next_resource_id: u64,
    pub adapter_name: String,
    pub adapter_features: wgpu::Features,
    pub adapter_info: wgpu::AdapterInfo,
}
impl GpuRenderer {
    pub async fn new(window: Arc<Window>) -> Result<Self, String> {
        Self::with_optional_features(window, wgpu::Backends::all(), wgpu::Features::empty()).await
    }
    /// Explicit optional device features for independent compute plugins. Defaults
    /// remain unchanged; unsupported optional feature sets are not enabled.
    pub async fn with_optional_features(
        window: Arc<Window>,
        backends: wgpu::Backends,
        optional: wgpu::Features,
    ) -> Result<Self, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let surface = instance.create_surface(window).map_err(|e| e.to_string())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        let adapter_info = adapter.get_info();
        let adapter_name = adapter_info.name.clone();
        let adapter_features = adapter.features();
        let optional = if adapter.features().contains(optional) {
            optional
        } else {
            wgpu::Features::empty()
        };
        let mut limits = wgpu::Limits::default();
        if optional.contains(wgpu::Features::EXPERIMENTAL_RAY_TRACING_ACCELERATION_STRUCTURE) {
            limits = limits.using_minimum_supported_acceleration_structure_values();
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: (adapter.features() & wgpu::Features::TIMESTAMP_QUERY)
                    | optional,
                required_limits: limits,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface has no compatible format")?;
        if let Some(format) = surface
            .get_capabilities(&adapter)
            .formats
            .into_iter()
            .find(|f| f.is_srgb())
        {
            config.format = format;
        }
        config.present_mode = wgpu::PresentMode::AutoNoVsync;
        surface.configure(&device, &config);
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shader parameters"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pixel nearest"),
            ..Default::default()
        });
        let mut renderer = Self {
            hdr: None,
            surface,
            device,
            queue,
            config,
            textures: HashMap::new(),
            passes: Vec::new(),
            texture_layout,
            params_layout,
            sampler,
            adapter_name,
            adapter_features,
            adapter_info,
            next_resource_id: 100,
        };
        renderer
            .register_pass(
                PassId(0),
                &ShaderSource(DEFAULT_SHADER.into()),
                [1., 0., 0., 0.],
            )
            .await?;
        renderer.upload(TextureId(0), &Image::new(1, 1, vec![255; 4])?)?;
        Ok(renderer)
    }
    fn allocate_id(&mut self) -> Result<u64, String> {
        loop {
            let id = self.next_resource_id;
            self.next_resource_id = id.checked_add(1).ok_or("render IDs exhausted")?;
            if !self.textures.contains_key(&TextureId(id))
                && !self.passes.iter().any(|p| p.id == PassId(id))
            {
                return Ok(id);
            }
        }
    }
    pub fn allocate_texture_id(&mut self) -> Result<TextureId, String> {
        self.allocate_id().map(TextureId)
    }
    pub fn allocate_pass_id(&mut self) -> Result<PassId, String> {
        self.allocate_id().map(PassId)
    }
    pub fn resource_counts(&self) -> (usize, usize) {
        (self.textures.len(), self.passes.len())
    }
    pub fn remove_pass(&mut self, id: PassId) -> bool {
        if id == PassId(0) {
            return false;
        }
        let before = self.passes.len();
        self.passes.retain(|p| p.id != id);
        self.passes.len() != before
    }
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
    /// Share an externally computed image without GPU-to-CPU readback. Caller
    /// uses this renderer's device/queue; the image remains an ordinary texture.
    pub fn bind_texture(
        &mut self,
        id: TextureId,
        texture: wgpu::Texture,
        linear: bool,
    ) -> Result<(), String> {
        if id == TextureId(0)
            || !texture
                .usage()
                .contains(wgpu::TextureUsages::TEXTURE_BINDING)
            || texture.dimension() != wgpu::TextureDimension::D2
            || texture.depth_or_array_layers() != 1
            || texture.sample_count() != 1
            || !matches!(
                texture.format(),
                wgpu::TextureFormat::Rgba8Unorm
                    | wgpu::TextureFormat::Rgba8UnormSrgb
                    | wgpu::TextureFormat::Bgra8Unorm
                    | wgpu::TextureFormat::Bgra8UnormSrgb
                    | wgpu::TextureFormat::Rgba16Float
            )
        {
            return Err(
                "external image must be a sampleable RGBA8/BGRA8/RGBA16F 2D texture, ID 0 is reserved"
                    .into(),
            );
        }
        let view = texture.create_view(&Default::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: if linear {
                wgpu::FilterMode::Linear
            } else {
                wgpu::FilterMode::Nearest
            },
            min_filter: if linear {
                wgpu::FilterMode::Linear
            } else {
                wgpu::FilterMode::Nearest
            },
            ..Default::default()
        });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("external sampled image"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        self.textures.insert(
            id,
            GpuTexture {
                size: [texture.width(), texture.height()],
                _texture: texture,
                bind,
                cpu_writable: false,
            },
        );
        Ok(())
    }
    /// Optional group 2 is a generic sampled image, independent of any world or
    /// lighting data. Parameters remain the Shader's existing vec4 contract.
    pub async fn register_sampled_pass(
        &mut self,
        id: PassId,
        source: &ShaderSource,
        parameters: [f32; 4],
        image: TextureId,
    ) -> Result<(), String> {
        if !self.textures.contains_key(&image) {
            return Err("sampled pass image is missing".into());
        }
        self.register_with_sampled(id, source, parameters, Some(image))
            .await
    }
    pub async fn register_pass(
        &mut self,
        id: PassId,
        source: &ShaderSource,
        parameters: [f32; 4],
    ) -> Result<(), String> {
        self.register_with_sampled(id, source, parameters, None)
            .await
    }
    /// Registration is transactional: failed shaders leave the old pipeline alive.
    async fn register_with_sampled(
        &mut self,
        id: PassId,
        source: &ShaderSource,
        parameters: [f32; 4],
        sampled: Option<TextureId>,
    ) -> Result<(), String> {
        if parameters.iter().any(|v| !v.is_finite()) {
            return Err("non-finite shader parameter".into());
        }
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("game shader"),
                source: wgpu::ShaderSource::Wgsl(source.0.clone().into()),
            });
        let mut layouts = vec![&self.texture_layout, &self.params_layout];
        if sampled.is_some() {
            layouts.push(&self.texture_layout);
        }
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("quad layout"),
                bind_group_layouts: &layouts,
                push_constant_ranges: &[],
            });
        let pipeline=self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor{
   label:Some("quad pass"),layout:Some(&layout),vertex:wgpu::VertexState{module:&shader,entry_point:Some("vs_main"),compilation_options:Default::default(),buffers:&[wgpu::VertexBufferLayout{array_stride:32,step_mode:wgpu::VertexStepMode::Vertex,attributes:&wgpu::vertex_attr_array![0=>Float32x2,1=>Float32x2,2=>Float32x4]}]},
   fragment:Some(wgpu::FragmentState{module:&shader,entry_point:Some("fs_main"),compilation_options:Default::default(),targets:&[Some(wgpu::ColorTargetState{format:self.config.format,blend:Some(wgpu::BlendState::ALPHA_BLENDING),write_mask:wgpu::ColorWrites::ALL})]}),
   primitive:Default::default(),depth_stencil:None,multisample:Default::default(),multiview:None,cache:None,
  });
        let hdr_pipeline=self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor{
   label:Some("quad pass"),layout:Some(&layout),vertex:wgpu::VertexState{module:&shader,entry_point:Some("vs_main"),compilation_options:Default::default(),buffers:&[wgpu::VertexBufferLayout{array_stride:32,step_mode:wgpu::VertexStepMode::Vertex,attributes:&wgpu::vertex_attr_array![0=>Float32x2,1=>Float32x2,2=>Float32x4]}]},
   fragment:Some(wgpu::FragmentState{module:&shader,entry_point:Some("fs_main"),compilation_options:Default::default(),targets:&[Some(wgpu::ColorTargetState{format:wgpu::TextureFormat::Rgba16Float,blend:Some(wgpu::BlendState::ALPHA_BLENDING),write_mask:wgpu::ColorWrites::ALL})]}),
   primitive:Default::default(),depth_stencil:None,multisample:Default::default(),multiview:None,cache:None,
  });
        if let Some(error) = self.device.pop_error_scope().await {
            return Err(format!("shader pass {}: {error}", id.0));
        }
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("pass parameters"),
                contents: bytemuck::cast_slice(&parameters),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pass parameters"),
            layout: &self.params_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let pass = Pass {
            id,
            pipeline,
            hdr_pipeline,
            uniform,
            bind,
            sampled,
            parameters,
        };
        if let Some(old) = self.passes.iter_mut().find(|p| p.id == id) {
            *old = pass;
        } else {
            self.passes.push(pass)
        }
        Ok(())
    }
    pub fn parameters(&self, id: PassId) -> Result<[f32; 4], String> {
        self.passes
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.parameters)
            .ok_or("unknown pass".into())
    }
    pub fn set_parameters(&mut self, id: PassId, parameters: [f32; 4]) -> Result<(), String> {
        if parameters.iter().any(|v| !v.is_finite()) {
            return Err("non-finite shader parameter".into());
        }
        let pass = self
            .passes
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or("unknown pass")?;
        pass.parameters = parameters;
        self.queue
            .write_buffer(&pass.uniform, 0, bytemuck::cast_slice(&parameters));
        Ok(())
    }
    pub fn remove_texture(&mut self, id: TextureId) -> bool {
        if id == TextureId(0) {
            false
        } else {
            self.textures.remove(&id).is_some()
        }
    }
    pub fn upload(&mut self, id: TextureId, image: &Image) -> Result<(), String> {
        self.upload_format(id, image, wgpu::TextureFormat::Rgba8UnormSrgb)
    }
    /// Linear data (for example masks or numerical fields) must not be decoded
    /// as sRGB by the sampler.
    pub fn upload_linear(&mut self, id: TextureId, image: &Image) -> Result<(), String> {
        self.upload_format(id, image, wgpu::TextureFormat::Rgba8Unorm)
    }
    fn upload_format(
        &mut self,
        id: TextureId,
        image: &Image,
        format: wgpu::TextureFormat,
    ) -> Result<(), String> {
        if self.textures.get(&id).is_some_and(|t| !t.cpu_writable) {
            return Err("cannot upload CPU pixels into an externally owned texture".into());
        }
        Image::new(image.width, image.height, image.rgba.clone())?;
        let max = self.device.limits().max_texture_dimension_2d;
        if image.width > max || image.height > max {
            return Err("texture exceeds GPU limits".into());
        }
        let size = [image.width, image.height];
        if self
            .textures
            .get(&id)
            .is_none_or(|t| t.size != size || t._texture.format() != format)
        {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("CPU image upload"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.textures.insert(
                id,
                GpuTexture {
                    _texture: texture,
                    bind,
                    size,
                    cpu_writable: true,
                },
            );
        }
        if !self.textures[&id].cpu_writable {
            return Err("cannot upload CPU pixels into an externally owned texture".into());
        }
        let texture = &self.textures[&id]._texture;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width * 4),
                rows_per_image: Some(image.height),
            },
            texture.size(),
        );
        Ok(())
    }
    /// Enable HDR for world layers; higher layers are drawn after tone mapping.
    /// Passing None restores the original render path and releases HDR targets.
    pub fn set_hdr(&mut self, settings: Option<HdrSettings>) -> Result<(), String> {
        if let Some(settings) = settings {
            if !settings.exposure.is_finite()
                || settings.exposure <= 0.
                || !settings.bloom_strength.is_finite()
                || !(0. ..=4.).contains(&settings.bloom_strength)
                || !settings.bloom_threshold.is_finite()
                || settings.bloom_threshold < 0.
            {
                return Err("invalid HDR settings".into());
            }
            let size = [self.config.width, self.config.height];
            if let Some(hdr) = &mut self.hdr {
                if hdr.size == size {
                    hdr.update(&self.queue, settings);
                    return Ok(());
                }
            }
            self.hdr = Some(hdr::HdrPost::new(
                &self.device,
                size,
                self.config.format,
                settings,
            ));
        } else {
            self.hdr = None;
        }
        Ok(())
    }
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        let max = self.device.limits().max_texture_dimension_2d;
        if width > max || height > max {
            return Err("surface exceeds GPU dimensions".into());
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        if let Some(settings) = self.hdr.as_ref().map(|h| h.settings) {
            self.set_hdr(Some(settings))?;
        }
        Ok(())
    }
}
impl Backend for GpuRenderer {
    fn render(&mut self, frame: &DrawFrame) -> Result<(), String> {
        if frame.size.contains(&0) {
            return Ok(());
        }
        if frame.size != [self.config.width, self.config.height] {
            self.resize(frame.size[0], frame.size[1])?
        }
        let output = match self.surface.get_current_texture() {
            Ok(v) => v,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout | wgpu::SurfaceError::Other) => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        let mut ordered: Vec<_> = frame.quads.iter().collect();
        ordered.sort_by_key(|q| q.layer);
        let mut vertices = Vec::with_capacity(ordered.len() * 6);
        for q in &ordered {
            if !self.textures.contains_key(&q.texture)
                || !self.passes.iter().any(|p| p.id == q.pass)
            {
                return Err("draw references missing texture or pass".into());
            }
            if q.rect
                .iter()
                .chain(q.uv.iter())
                .chain(q.tint.iter())
                .any(|v| !v.is_finite())
            {
                return Err("non-finite draw data".into());
            }
            let [x, y, w, h] = q.rect;
            let [u, v, uw, vh] = q.uv;
            for (px, py, tx, ty) in [
                (x, y, u, v),
                (x + w, y, u + uw, v),
                (x, y + h, u, v + vh),
                (x, y + h, u, v + vh),
                (x + w, y, u + uw, v),
                (x + w, y + h, u + uw, v + vh),
            ] {
                vertices.push(Vertex {
                    position: [
                        px / frame.size[0] as f32 * 2. - 1.,
                        1. - py / frame.size[1] as f32 * 2.,
                    ],
                    uv: [tx, ty],
                    tint: q.tint,
                })
            }
        }
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("frame quads"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let view = output.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let stages = if self.hdr.is_some() { 2 } else { 1 };
        for stage in 0..stages {
            if stage == 1 {
                self.hdr.as_ref().unwrap().encode(&mut encoder, &view);
            }
            let target = if stage == 0 {
                self.hdr.as_ref().map_or(&view, |h| &h.scene)
            } else {
                &view
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("registered passes"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if stage == 1 {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(wgpu::Color {
                                r: frame.clear[0],
                                g: frame.clear[1],
                                b: frame.clear[2],
                                a: frame.clear[3],
                            })
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_vertex_buffer(0, buffer.slice(..));
            for pipeline in &self.passes {
                if !ordered.iter().any(|q| q.pass == pipeline.id) {
                    continue;
                }
                let clip = frame.clips.get(&pipeline.id).copied().unwrap_or([
                    0,
                    0,
                    frame.size[0],
                    frame.size[1],
                ]);
                let x = clip[0].min(frame.size[0]);
                let y = clip[1].min(frame.size[1]);
                let w = clip[2].min(frame.size[0] - x);
                let h = clip[3].min(frame.size[1] - y);
                if w == 0 || h == 0 {
                    continue;
                }
                pass.set_scissor_rect(x, y, w, h);
                if let Some(id) = pipeline.sampled {
                    let texture = self.textures.get(&id).ok_or("sampled pass image removed")?;
                    pass.set_bind_group(2, &texture.bind, &[]);
                }
                pass.set_pipeline(if self.hdr.is_some() && stage == 0 {
                    &pipeline.hdr_pipeline
                } else {
                    &pipeline.pipeline
                });
                pass.set_bind_group(1, &pipeline.bind, &[]);

                let mut i = 0;
                while i < ordered.len() {
                    let in_stage = |q: &&Quad| {
                        self.hdr
                            .as_ref()
                            .is_none_or(|h| (q.layer <= h.settings.max_world_layer) == (stage == 0))
                    };
                    if ordered[i].pass != pipeline.id || !in_stage(&ordered[i]) {
                        i += 1;
                        continue;
                    }
                    let texture = ordered[i].texture;
                    let start = i;
                    i += 1;
                    while i < ordered.len()
                        && ordered[i].pass == pipeline.id
                        && ordered[i].texture == texture
                        && in_stage(&ordered[i])
                    {
                        i += 1;
                    }
                    pass.set_bind_group(0, &self.textures[&texture].bind, &[]);
                    pass.draw(start as u32 * 6..i as u32 * 6, 0..1);
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        output.present();
        Ok(())
    }
}

/// Explicit diagnostic wait; normal rendering remains asynchronous.
pub fn wait_for_completion() -> wgpu::PollType {
    wgpu::PollType::Wait
}
