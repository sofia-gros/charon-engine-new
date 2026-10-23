use crate::Result;
use charon_assets::Texture;

/// Uploaded once and reused by every sprite sharing this image/atlas.
pub struct SpriteTexture {
    bind: wgpu::BindGroup,
    width: u32,
    height: u32,
}

pub struct Sprite<'a> {
    pub texture: &'a SpriteTexture,
    pub source: [u32; 4],
    /// x, y, width, height in the same logical coordinates as the terrain image.
    pub destination: [f32; 4],
    pub flip_x: bool,
}

pub(crate) struct SpriteLayer {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    binds: Vec<wgpu::BindGroup>,
}
impl SpriteLayer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprites"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprites.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprites"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 8,
                            shader_location: 1,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprite vertices"),
            size: 1024 * 6 * 16,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            buffer,
            binds: vec![],
        }
    }
    pub fn upload_texture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &Texture,
    ) -> SpriteTexture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sprite atlas"),
            size: wgpu::Extent3d {
                width: image.width(),
                height: image.height(),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            image.rgba(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width() * 4),
                rows_per_image: None,
            },
            texture.size(),
        );
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite atlas"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &texture.create_view(&Default::default()),
                ),
            }],
        });
        SpriteTexture {
            bind,
            width: image.width(),
            height: image.height(),
        }
    }
    pub fn set(
        &mut self,
        queue: &wgpu::Queue,
        sprites: &[Sprite<'_>],
        width: u32,
        height: u32,
    ) -> Result<usize> {
        if sprites.len() > 1024 {
            return Err("initial sprite batch supports at most 1024 sprites".into());
        }
        let mut bytes = Vec::with_capacity(sprites.len() * 6 * 16);
        let mut binds = Vec::with_capacity(sprites.len());
        for sprite in sprites {
            let [sx, sy, sw, sh] = sprite.source;
            let [x, y, w, h] = sprite.destination;
            if sw == 0
                || sh == 0
                || sx.checked_add(sw).is_none_or(|v| v > sprite.texture.width)
                || sy.checked_add(sh).is_none_or(|v| v > sprite.texture.height)
                || ![x, y, w, h].into_iter().all(f32::is_finite)
                || w <= 0.
                || h <= 0.
            {
                return Err("invalid sprite source/destination rectangle".into());
            }
            for (dx, dy) in [(0., 0.), (1., 0.), (0., 1.), (0., 1.), (1., 0.), (1., 1.)] {
                let u = if sprite.flip_x { 1. - dx } else { dx };
                for value in [
                    (x + dx * w) / width as f32 * 2. - 1.,
                    1. - (y + dy * h) / height as f32 * 2.,
                    sx as f32 + u * sw as f32,
                    sy as f32 + dy * sh as f32,
                ] {
                    bytes.extend_from_slice(&value.to_ne_bytes());
                }
            }
            binds.push(sprite.texture.bind.clone());
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.buffer, 0, &bytes);
        }
        self.binds = binds;
        Ok(bytes.len())
    }
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.binds.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.buffer.slice(..));
        for (index, bind) in self.binds.iter().enumerate() {
            pass.set_bind_group(0, bind, &[]);
            pass.draw(index as u32 * 6..index as u32 * 6 + 6, 0..1);
        }
    }
}
