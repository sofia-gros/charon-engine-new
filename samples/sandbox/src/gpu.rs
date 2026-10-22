use charon_sand::Frame;
use std::{error::Error, fs::File, io::BufWriter, path::Path, sync::mpsc, time::Duration};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN | wgpu::Backends::DX12 | wgpu::Backends::GL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    })
}

pub fn window_instance(display: winit::event_loop::OwnedDisplayHandle) -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN | wgpu::Backends::DX12 | wgpu::Backends::GL,
        ..wgpu::InstanceDescriptor::new_with_display_handle(Box::new(display))
    })
}

pub async fn adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
    software: bool,
) -> Result<wgpu::Adapter> {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: surface,
            force_fallback_adapter: software,
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        })
        .await?;
    let info = adapter.get_info();
    println!(
        "adapter={} backend={:?} device_type={:?}",
        info.name, info.backend, info.device_type
    );
    Ok(adapter)
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
}

impl Gpu {
    pub async fn new(
        adapter: &wgpu::Adapter,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("charon-device"),
                ..Default::default()
            })
            .await?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cells"),
            source: wgpu::ShaderSource::Wgsl(include_str!("cells.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cells"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(format.into())],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cell-snapshot"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cells"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &texture.create_view(&Default::default()),
                ),
            }],
        });
        Ok(Self {
            device,
            queue,
            pipeline,
            texture,
            bind,
        })
    }

    pub fn upload(&self, frame: &Frame) {
        self.queue.write_texture(
            self.texture.as_image_copy(),
            &frame.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(frame.width as u32 * 4),
                rows_per_image: None,
            },
            self.texture.size(),
        );
    }

    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cells"),
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
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Exercise the same shader/pipeline as the window and verify every output
    /// pixel against the nearest-neighbour scaled simulation snapshot.
    pub fn smoke(&self, frame: &Frame, output: &Path) -> Result<()> {
        const WIDTH: u32 = 1920;
        const HEIGHT: u32 = 1080;
        self.upload(frame);
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let pitch = (WIDTH * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("smoke-readback"),
            size: u64::from(pitch * HEIGHT),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.draw(&mut encoder, &target.create_view(&Default::default()));
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pitch),
                    rows_per_image: None,
                },
            },
            target.size(),
        );
        self.queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })?;
        receiver.recv_timeout(Duration::from_secs(30))??;
        let mapped = buffer.slice(..).get_mapped_range()?;
        let mut rgba = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
        for row in mapped.chunks_exact(pitch as usize) {
            rgba.extend_from_slice(&row[..WIDTH as usize * 4]);
        }
        for y in 0..HEIGHT as usize {
            for x in 0..WIDTH as usize {
                let source = (((2 * y + 1) * frame.height / (2 * HEIGHT as usize)) * frame.width
                    + (2 * x + 1) * frame.width / (2 * WIDTH as usize))
                    * 4;
                let destination = (y * WIDTH as usize + x) * 4;
                if rgba[destination..destination + 4] != frame.rgba[source..source + 4] {
                    return Err(format!("shader output mismatch at ({x},{y})").into());
                }
            }
        }
        if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let mut png = png::Encoder::new(BufWriter::new(File::create(output)?), WIDTH, HEIGHT);
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.write_header()?.write_image_data(&rgba)?;
        println!(
            "shader_pixels_checked={} screenshot={}",
            WIDTH * HEIGHT,
            output.display()
        );
        Ok(())
    }
}
