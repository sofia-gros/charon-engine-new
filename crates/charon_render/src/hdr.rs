use wgpu::util::DeviceExt;
/// Fixed exposure keeps visual comparisons meaningful. UI is rendered afterwards.
#[derive(Clone, Copy, Debug)]
pub struct HdrSettings {
    pub exposure: f32,
    pub bloom_strength: f32,
    pub bloom_threshold: f32,
    pub max_world_layer: i32,
}
impl Default for HdrSettings {
    fn default() -> Self {
        Self {
            exposure: 1.,
            bloom_strength: 0.18,
            bloom_threshold: 1.,
            max_world_layer: 3,
        }
    }
}
pub struct HdrPost {
    pub scene: wgpu::TextureView,
    _textures: [wgpu::Texture; 3],
    views: [wgpu::TextureView; 2],
    groups: [wgpu::BindGroup; 3],
    pipelines: [wgpu::RenderPipeline; 3],
    pub settings: HdrSettings,
    pub size: [u32; 2],
    uniform: wgpu::Buffer,
}
impl HdrPost {
    pub fn new(
        device: &wgpu::Device,
        size: [u32; 2],
        format: wgpu::TextureFormat,
        settings: HdrSettings,
    ) -> Self {
        let texture = |label, width, height| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
        };
        let scene = texture("HDR scene color", size[0], size[1]);
        let b1 = texture(
            "HDR threshold horizontal bloom",
            (size[0] / 4).max(1),
            (size[1] / 4).max(1),
        );
        let b2 = texture("HDR vertical bloom", b1.width(), b1.height());
        let scene_view = scene.create_view(&Default::default());
        let b1_view = b1.create_view(&Default::default());
        let b2_view = b2.create_view(&Default::default());
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("HDR exposure and bloom"),
            contents: bytemuck::cast_slice(&[
                settings.exposure,
                settings.bloom_strength,
                settings.bloom_threshold,
                0_f32,
            ]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("HDR post textures"),
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
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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
        let group = |source: &wgpu::TextureView, bloom: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("HDR post inputs"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(bloom),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform.as_entire_binding(),
                    },
                ],
            })
        };
        let groups = [
            group(&scene_view, &scene_view),
            group(&b1_view, &b1_view),
            group(&scene_view, &b2_view),
        ];
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("HDR bloom and tone mapping"),
            source: wgpu::ShaderSource::Wgsl(include_str!("hdr.wgsl").into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("HDR post layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = |entry, target| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let pipelines = [
            pipeline("bloom_x", wgpu::TextureFormat::Rgba16Float),
            pipeline("bloom_y", wgpu::TextureFormat::Rgba16Float),
            pipeline("present", format),
        ];
        Self {
            scene: scene_view,
            _textures: [scene, b1, b2],
            views: [b1_view, b2_view],
            groups,
            pipelines,
            settings,
            size,
            uniform,
        }
    }
    pub fn update(&mut self, queue: &wgpu::Queue, settings: HdrSettings) {
        self.settings = settings;
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::cast_slice(&[
                settings.exposure,
                settings.bloom_strength,
                settings.bloom_threshold,
                0_f32,
            ]),
        );
    }
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, output: &wgpu::TextureView) {
        for i in 0..3 {
            let view = if i < 2 { &self.views[i] } else { output };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("HDR postprocess"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipelines[i]);
            pass.set_bind_group(0, &self.groups[i], &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
