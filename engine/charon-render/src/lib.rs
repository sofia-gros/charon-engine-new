pub use charon_image::TiledImage;
use std::sync::Arc;
use winit::{dpi::PhysicalSize, event_loop::ActiveEventLoop, window::Window};
/// Borrowed RGBA8 image, independent of any simulation plugin.
#[derive(Clone, Copy)]
pub struct ImageView<'a> {
    pub width: usize,
    pub height: usize,
    pub rgba: &'a [u8],
}
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
    uploaded_tiles: Option<TiledImage>,
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
            uploaded_tiles: None,
        })
    }

    pub fn upload(&mut self, frame: &ImageView<'_>) {
        assert_eq!(
            (frame.width, frame.height),
            (
                self.texture.width() as usize,
                self.texture.height() as usize
            )
        );
        assert_eq!(frame.rgba.len(), frame.width * frame.height * 4);
        self.uploaded_tiles = None;
        self.queue.write_texture(
            self.texture.as_image_copy(),
            frame.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(frame.width as u32 * 4),
                rows_per_image: None,
            },
            self.texture.size(),
        );
    }

    /// Compare against the last uploaded image, not the preceding simulation tick.
    /// Retaining handles prevents lost updates when the consumer skips snapshots.
    pub fn upload_tiled(&mut self, image: &TiledImage) -> usize {
        assert_eq!(
            (image.width(), image.height()),
            (
                self.texture.width() as usize,
                self.texture.height() as usize
            )
        );
        let mut bytes = 0;
        for index in 0..image.tile_count() {
            if self
                .uploaded_tiles
                .as_ref()
                .is_some_and(|previous| image.shares_tile(previous, index))
            {
                continue;
            }
            let rect = image.rect(index);
            let mut destination = self.texture.as_image_copy();
            destination.origin = wgpu::Origin3d {
                x: rect.x as u32,
                y: rect.y as u32,
                z: 0,
            };
            self.queue.write_texture(
                destination,
                image.pixels(index),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(rect.width as u32 * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: rect.width as u32,
                    height: rect.height as u32,
                    depth_or_array_layers: 1,
                },
            );
            bytes += image.pixels(index).len();
        }
        if bytes > 0 || self.uploaded_tiles.is_none() {
            self.uploaded_tiles = Some(image.clone());
        }
        bytes
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
    pub fn smoke(&mut self, frame: &ImageView<'_>, output: &Path) -> Result<()> {
        self.upload(frame);
        self.verify(frame, output)
    }

    /// Verify the actual partial-upload path against a complete CPU image.
    pub fn smoke_tiled(&mut self, image: &TiledImage, output: &Path) -> Result<usize> {
        let bytes = self.upload_tiled(image);
        self.verify(
            &ImageView {
                width: image.width(),
                height: image.height(),
                rgba: &image.to_rgba(),
            },
            output,
        )?;
        Ok(bytes)
    }

    fn verify(&self, frame: &ImageView<'_>, output: &Path) -> Result<()> {
        const WIDTH: u32 = 1920;
        const HEIGHT: u32 = 1080;
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

pub struct WindowRenderer {
    pub window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
}
impl WindowRenderer {
    pub fn new(
        event_loop: &ActiveEventLoop,
        title: &str,
        width: u32,
        height: u32,
        software: bool,
    ) -> Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(title)
                    .with_inner_size(PhysicalSize::new(1920, 1080)),
            )?,
        );
        let instance = window_instance(event_loop.owned_display_handle());
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(adapter(&instance, Some(&surface), software))?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface has no supported format")?;
        config.format = surface
            .get_capabilities(&adapter)
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(config.format);
        config.present_mode = wgpu::PresentMode::AutoVsync;
        let gpu = pollster::block_on(Gpu::new(&adapter, config.format, width, height))?;
        surface.configure(&gpu.device, &config);
        println!(
            "surface={}x{} present={:?} image={}x{}",
            config.width, config.height, config.present_mode, width, height
        );
        println!("Draw FPS counts submissions; GPU execution and physical presentation are not measured.");
        Ok(Self {
            window,
            instance,
            surface,
            config,
            gpu,
        })
    }
    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width > 0 && size.height > 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.gpu.device, &self.config);
        }
    }
    /// Times CPU calls only. Upload bytes are zero when reusing the previous image.
    pub fn present(&mut self, image: Option<ImageView<'_>>) -> Result<Option<RenderStats>> {
        self.present_image(image.map(Upload::Full))
    }
    pub fn present_tiled(&mut self, image: &TiledImage) -> Result<Option<RenderStats>> {
        self.present_image(Some(Upload::Tiled(image)))
    }
    fn present_image(&mut self, image: Option<Upload<'_>>) -> Result<Option<RenderStats>> {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(None);
        }
        let acquire_start = std::time::Instant::now();
        let (texture, reconfigure) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.gpu.device, &self.config);
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self.instance.create_surface(self.window.clone())?;
                self.surface.configure(&self.gpu.device, &self.config);
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => {
                return Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("surface validation failed".into())
            }
        };
        let acquire_ms = acquire_start.elapsed().as_secs_f64() * 1000.0;
        let upload_start = std::time::Instant::now();
        let upload_bytes = match image {
            Some(Upload::Full(image)) => {
                self.gpu.upload(&image);
                image.rgba.len()
            }
            Some(Upload::Tiled(image)) => self.gpu.upload_tiled(image),
            None => 0,
        };
        let upload_enqueue_ms = upload_start.elapsed().as_secs_f64() * 1000.0;
        let encode_start = std::time::Instant::now();
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.gpu.draw(
            &mut encoder,
            &texture.texture.create_view(&Default::default()),
        );
        self.gpu.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        self.gpu.queue.present(texture);
        if reconfigure {
            self.surface.configure(&self.gpu.device, &self.config);
        }
        Ok(Some(RenderStats {
            upload_enqueue_ms,
            upload_bytes,
            acquire_ms,
            encode_present_ms: encode_start.elapsed().as_secs_f64() * 1000.0,
        }))
    }
}
#[derive(Clone, Copy, Default)]
pub struct RenderStats {
    pub upload_enqueue_ms: f64,
    pub upload_bytes: usize,
    pub acquire_ms: f64,
    pub encode_present_ms: f64,
}

enum Upload<'a> {
    Full(ImageView<'a>),
    Tiled(&'a TiledImage),
}
