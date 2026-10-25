use super::*;
use charon_assets::{Image, ShaderSource};
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
}
struct Pass {
    id: PassId,
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
}
pub struct GpuRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    textures: HashMap<TextureId, GpuTexture>,
    passes: Vec<Pass>,
    texture_layout: wgpu::BindGroupLayout,
    params_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pub adapter_name: String,
}
impl GpuRenderer {
    pub async fn new(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window).map_err(|e| e.to_string())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        let adapter_name = adapter.get_info().name;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(|e| e.to_string())?;
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface has no compatible format")?;
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
    /// Registration is transactional: failed shaders leave the old pipeline alive.
    pub async fn register_pass(
        &mut self,
        id: PassId,
        source: &ShaderSource,
        parameters: [f32; 4],
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
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("quad layout"),
                bind_group_layouts: &[&self.texture_layout, &self.params_layout],
                push_constant_ranges: &[],
            });
        let pipeline=self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor{
   label:Some("quad pass"),layout:Some(&layout),vertex:wgpu::VertexState{module:&shader,entry_point:Some("vs_main"),compilation_options:Default::default(),buffers:&[wgpu::VertexBufferLayout{array_stride:32,step_mode:wgpu::VertexStepMode::Vertex,attributes:&wgpu::vertex_attr_array![0=>Float32x2,1=>Float32x2,2=>Float32x4]}]},
   fragment:Some(wgpu::FragmentState{module:&shader,entry_point:Some("fs_main"),compilation_options:Default::default(),targets:&[Some(wgpu::ColorTargetState{format:self.config.format,blend:Some(wgpu::BlendState::ALPHA_BLENDING),write_mask:wgpu::ColorWrites::ALL})]}),
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
            uniform,
            bind,
        };
        if let Some(old) = self.passes.iter_mut().find(|p| p.id == id) {
            *old = pass;
        } else {
            self.passes.push(pass)
        }
        Ok(())
    }
    pub fn set_parameters(&mut self, id: PassId, parameters: [f32; 4]) -> Result<(), String> {
        if parameters.iter().any(|v| !v.is_finite()) {
            return Err("non-finite shader parameter".into());
        }
        let pass = self
            .passes
            .iter()
            .find(|p| p.id == id)
            .ok_or("unknown pass")?;
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
        Image::new(image.width, image.height, image.rgba.clone())?;
        let max = self.device.limits().max_texture_dimension_2d;
        if image.width > max || image.height > max {
            return Err("texture exceeds GPU limits".into());
        }
        let size = [image.width, image.height];
        if self.textures.get(&id).is_none_or(|t| t.size != size) {
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
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
                },
            );
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
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("registered passes"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: frame.clear[0],
                            g: frame.clear[1],
                            b: frame.clear[2],
                            a: frame.clear[3],
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_vertex_buffer(0, buffer.slice(..));
            for pipeline in &self.passes {
                pass.set_pipeline(&pipeline.pipeline);
                pass.set_bind_group(1, &pipeline.bind, &[]);

                let mut i = 0;
                while i < ordered.len() {
                    if ordered[i].pass != pipeline.id {
                        i += 1;
                        continue;
                    }
                    let texture = ordered[i].texture;
                    let start = i;
                    i += 1;
                    while i < ordered.len()
                        && ordered[i].pass == pipeline.id
                        && ordered[i].texture == texture
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
