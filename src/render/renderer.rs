//! Renders the simulation grid as a fullscreen triangle with a bloom pass
//! for emissive materials (fire / acid).

use bytemuck::{Pod, Zeroable};
use sandengine::sim::grid::Grid;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    inv_size: [f32; 2],
    bloom_strength: f32,
    _pad: f32,
}

pub struct Renderer {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    grid_w: u32,
    grid_h: u32,
    cpu_pixels: Vec<u8>,
    pub bloom_strength: f32,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, gw: u32, gh: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sand_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sand.wgsl").into()),
        });

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grid_tex"),
            size: wgpu::Extent3d {
                width: gw,
                height: gh,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Linear filter on the grid texture would soften individual cells, which
        // we don't want — the sand grain detail comes from the per-cell colour
        // variation in `material::to_rgba`. We use linear sampling only for the
        // bloom taps, which read offset UVs into the same texture.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sand_ub"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sand_bgl"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sand_bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buf.as_entire_binding(),
                },
            ],
        });

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sand_pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sand_pipeline"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let cpu_pixels = vec![0u8; (gw * gh * 4) as usize];

        Self {
            texture,
            bind_group,
            pipeline,
            uniform_buf,
            grid_w: gw,
            grid_h: gh,
            cpu_pixels,
            bloom_strength: 0.85,
        }
    }

    /// CPU -> GPU upload. The inner loop is just a memcpy of u32s per cell so
    /// the bottleneck stays in the physics sweep, not pixel encoding.
    pub fn upload(&mut self, queue: &wgpu::Queue, grid: &Grid) {
        debug_assert_eq!(grid.w as u32, self.grid_w);
        debug_assert_eq!(grid.h as u32, self.grid_h);

        for (i, cell) in grid.cells.iter().enumerate() {
            let rgba = cell.to_rgba();
            let o = i * 4;
            self.cpu_pixels[o] = rgba[0];
            self.cpu_pixels[o + 1] = rgba[1];
            self.cpu_pixels[o + 2] = rgba[2];
            self.cpu_pixels[o + 3] = rgba[3];
        }

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.cpu_pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.grid_w * 4),
                rows_per_image: Some(self.grid_h),
            },
            wgpu::Extent3d {
                width: self.grid_w,
                height: self.grid_h,
                depth_or_array_layers: 1,
            },
        );

        let u = Uniforms {
            inv_size: [1.0 / self.grid_w as f32, 1.0 / self.grid_h as f32],
            bloom_strength: self.bloom_strength,
            _pad: 0.0,
        };
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));
    }

    pub fn render<'a>(&'a self, rpass: &mut wgpu::RenderPass<'a>) {
        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, &self.bind_group, &[]);
        rpass.draw(0..3, 0..1);
    }
}
