mod debug_text;
mod indexed;
mod sprites;
pub use charon_image::IndexedImage;
pub use charon_image::TiledImage;
pub use debug_text::{append_debug_text, append_performance_overlay, DebugPerformance};
pub use indexed::TileAppearance;
pub use sprites::{Sprite, SpriteTexture};
use std::sync::Arc;
use winit::{dpi::PhysicalSize, event_loop::ActiveEventLoop, window::Window};
/// Borrowed RGBA8 image, independent of any simulation plugin.
#[derive(Clone, Copy)]
pub struct ImageView<'a> {
    pub width: usize,
    pub height: usize,
    pub rgba: &'a [u8],
}
use std::{error::Error, sync::mpsc, time::Duration};

/// Opaque screen-aligned rectangle in logical image coordinates, independent of terrain.
#[derive(Clone, Copy, PartialEq)]
pub struct OverlayRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: [f32; 4],
}

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
    sprites: sprites::SpriteLayer,
    indexed: Option<indexed::IndexedLayer>,
    indexed_active: bool,
    format: wgpu::TextureFormat,
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    uploaded_tiles: Option<TiledImage>,
    overlay_pipeline: wgpu::RenderPipeline,
    overlay_buffer: wgpu::Buffer,
    overlay_vertices: u32,
    last_overlays: Vec<OverlayRect>,
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
        let overlay_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay"),
            source: wgpu::ShaderSource::Wgsl(include_str!("overlay.wgsl").into()),
        });
        let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &overlay_shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 24,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 8,
                            shader_location: 1,
                        },
                    ],
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &overlay_shader,
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
        let overlay_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay-vertices"),
            size: 1024 * 6 * 24,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
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
        let sprites = sprites::SpriteLayer::new(&device, format);
        Ok(Self {
            sprites,
            indexed: None,
            indexed_active: false,
            format,
            device,
            queue,
            pipeline,
            texture,
            bind,
            uploaded_tiles: None,
            overlay_pipeline,
            overlay_buffer,
            overlay_vertices: 0,
            last_overlays: Vec::new(),
        })
    }

    pub fn upload_sprite_texture(&self, texture: &charon_assets::Texture) -> SpriteTexture {
        self.sprites
            .upload_texture(&self.device, &self.queue, texture)
    }

    pub fn upload(&mut self, frame: &ImageView<'_>) {
        self.indexed_active = false;
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
        self.indexed_active = false;
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

    pub fn upload_indexed(
        &mut self,
        image: &IndexedImage,
        looks: &[TileAppearance],
        atlas: &Arc<charon_assets::Texture>,
    ) -> Result<[usize; 3]> {
        let layer = self.indexed.get_or_insert_with(|| {
            indexed::IndexedLayer::new(
                &self.device,
                self.format,
                self.texture.width(),
                self.texture.height(),
            )
        });
        let bytes = layer.upload(&self.device, &self.queue, image, looks, atlas)?;
        self.indexed_active = true;
        Ok(bytes)
    }

    fn set_overlays(&mut self, rectangles: &[OverlayRect]) -> usize {
        if rectangles == self.last_overlays {
            return 0;
        }
        assert!(
            rectangles.len() <= 1024,
            "initial overlay batch limit is 1024 rectangles"
        );
        let mut bytes = Vec::with_capacity(rectangles.len() * 6 * 24);
        for rect in rectangles {
            assert!([rect.x, rect.y, rect.width, rect.height]
                .into_iter()
                .chain(rect.color)
                .all(f32::is_finite));
            for (dx, dy) in [(0., 0.), (1., 0.), (0., 1.), (0., 1.), (1., 0.), (1., 1.)] {
                let x = (rect.x + dx * rect.width) / self.texture.width() as f32 * 2.0 - 1.0;
                let y = 1.0 - (rect.y + dy * rect.height) / self.texture.height() as f32 * 2.0;
                for value in [
                    x,
                    y,
                    rect.color[0],
                    rect.color[1],
                    rect.color[2],
                    rect.color[3],
                ] {
                    bytes.extend_from_slice(&value.to_ne_bytes());
                }
            }
        }
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.overlay_buffer, 0, &bytes);
        }
        self.overlay_vertices = rectangles.len() as u32 * 6;
        self.last_overlays = rectangles.to_vec();
        bytes.len()
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
        if self.indexed_active {
            self.indexed
                .as_ref()
                .expect("uploaded indexed layer")
                .draw(&mut pass);
        } else {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.sprites.draw(&mut pass);
        if self.overlay_vertices > 0 {
            pass.set_pipeline(&self.overlay_pipeline);
            pass.set_vertex_buffer(0, self.overlay_buffer.slice(..));
            pass.draw(0..self.overlay_vertices, 0..1);
        }
    }

    /// Exercise the same shader/pipeline as the window and verify every output
    /// pixel against the nearest-neighbour scaled simulation snapshot.
    pub fn smoke(&mut self, frame: &ImageView<'_>) -> Result<()> {
        self.upload(frame);
        self.verify(frame)
    }

    /// Verify the actual partial-upload path against a complete CPU image.
    pub fn smoke_tiled(&mut self, image: &TiledImage) -> Result<usize> {
        let bytes = self.upload_tiled(image);
        self.verify(&ImageView {
            width: image.width(),
            height: image.height(),
            rgba: &image.to_rgba(),
        })?;
        Ok(bytes)
    }

    pub fn smoke_overlay(&mut self, image: &TiledImage) -> Result<()> {
        self.upload_tiled(image);
        let (x, y, width, height) = (2, 2, 3, 4);
        self.set_overlays(&[OverlayRect {
            x: x as f32,
            y: y as f32,
            width: width as f32,
            height: height as f32,
            color: [1., 0., 0., 1.],
        }]);
        let mut rgba = image.to_rgba();
        for py in y..y + height {
            for px in x..x + width {
                let i = (py * image.width() + px) * 4;
                rgba[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let result = self.verify(&ImageView {
            width: image.width(),
            height: image.height(),
            rgba: &rgba,
        });
        self.set_overlays(&[]);
        result
    }

    /// Verify source rectangles, horizontal flipping and transparent pixels using
    /// the same sprite pipeline as the window, without exporting image files.
    pub fn smoke_sprites(&mut self, image: &TiledImage) -> Result<()> {
        let pixels = vec![255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 0, 255];
        let cpu = charon_assets::Texture::from_rgba(2, 2, pixels)?;
        let texture = self.upload_sprite_texture(&cpu);
        self.upload_tiled(image);
        self.set_overlays(&[]);
        for source in [[0, 0, 2, 2], [1, 0, 1, 2]] {
            for flip_x in [false, true] {
                self.sprites.set(
                    &self.queue,
                    &[Sprite {
                        texture: &texture,
                        source,
                        destination: [2., 2., 4., 4.],
                        flip_x,
                    }],
                    image.width() as u32,
                    image.height() as u32,
                )?;
                let mut rgba = image.to_rgba();
                for y in 0..4 {
                    for x in 0..4 {
                        let sx = if flip_x { 3 - x } else { x };
                        let pixel = ((y * source[3] as usize / 4 + source[1] as usize) * 2
                            + sx * source[2] as usize / 4
                            + source[0] as usize)
                            * 4;
                        if cpu.rgba()[pixel + 3] == 255 {
                            let dest = ((y + 2) * image.width() + x + 2) * 4;
                            rgba[dest..dest + 4].copy_from_slice(&cpu.rgba()[pixel..pixel + 4]);
                        }
                    }
                }
                self.verify(&ImageView {
                    width: image.width(),
                    height: image.height(),
                    rgba: &rgba,
                })?;
            }
        }
        self.sprites.set(
            &self.queue,
            &[],
            image.width() as u32,
            image.height() as u32,
        )?;
        Ok(())
    }

    pub fn smoke_indexed(
        &mut self,
        image: &IndexedImage,
        looks: &[TileAppearance],
        atlas: &Arc<charon_assets::Texture>,
    ) -> Result<[usize; 3]> {
        let bytes = self.upload_indexed(image, looks, atlas)?;
        let mut rgba = Vec::with_capacity(image.width() * image.height() * 4);
        for (index, id) in image.to_bytes().into_iter().enumerate() {
            let look = looks[id as usize];
            let mut color = look.color;
            if let Some([x, y, w, h]) = look.source {
                let px = index % image.width() % w as usize + x as usize;
                let py = index / image.width() % h as usize + y as usize;
                let texel = &atlas.rgba()[(py * atlas.width() as usize + px) * 4..][..4];
                for channel in 0..3 {
                    color[channel] = ((u32::from(texel[channel]) * u32::from(texel[3])
                        + u32::from(color[channel]) * (255 - u32::from(texel[3]))
                        + 127)
                        / 255) as u8;
                }
            }
            rgba.extend_from_slice(&color);
        }
        self.verify(&ImageView {
            width: image.width(),
            height: image.height(),
            rgba: &rgba,
        })?;
        Ok(bytes)
    }

    fn verify(&self, frame: &ImageView<'_>) -> Result<()> {
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
        println!("shader_pixels_checked={}", WIDTH * HEIGHT);
        Ok(())
    }
}

pub struct WindowRenderer {
    pub window: Arc<Window>,
    pub software_adapter: bool,
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
        let software_adapter = adapter.get_info().device_type == wgpu::DeviceType::Cpu;
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
            software_adapter,
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
        self.present_image(image.map(Upload::Full), &[], &[])
    }
    pub fn present_with_overlays(
        &mut self,
        image: Option<ImageView<'_>>,
        overlays: &[OverlayRect],
    ) -> Result<Option<RenderStats>> {
        self.present_image(image.map(Upload::Full), overlays, &[])
    }
    pub fn present_tiled(&mut self, image: &TiledImage) -> Result<Option<RenderStats>> {
        self.present_image(Some(Upload::Tiled(image)), &[], &[])
    }
    pub fn present_tiled_with_overlays(
        &mut self,
        image: &TiledImage,
        overlays: &[OverlayRect],
    ) -> Result<Option<RenderStats>> {
        self.present_image(Some(Upload::Tiled(image)), overlays, &[])
    }
    pub fn upload_sprite_texture(&self, texture: &charon_assets::Texture) -> SpriteTexture {
        self.gpu.upload_sprite_texture(texture)
    }
    pub fn present_tiled_with_sprites(
        &mut self,
        image: &TiledImage,
        sprites: &[Sprite<'_>],
        overlays: &[OverlayRect],
    ) -> Result<Option<RenderStats>> {
        self.present_image(Some(Upload::Tiled(image)), overlays, sprites)
    }
    pub fn present_indexed_with_sprites(
        &mut self,
        image: &IndexedImage,
        looks: &[TileAppearance],
        atlas: &Arc<charon_assets::Texture>,
        sprites: &[Sprite<'_>],
        overlays: &[OverlayRect],
    ) -> Result<Option<RenderStats>> {
        self.present_image(
            Some(Upload::Indexed {
                image,
                looks,
                atlas,
            }),
            overlays,
            sprites,
        )
    }
    fn present_image(
        &mut self,
        image: Option<Upload<'_>>,
        overlays: &[OverlayRect],
        sprites: &[Sprite<'_>],
    ) -> Result<Option<RenderStats>> {
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
        let mut appearance_upload_bytes = 0;
        let mut atlas_upload_bytes = 0;
        let upload_bytes = match image {
            Some(Upload::Full(image)) => {
                self.gpu.upload(&image);
                image.rgba.len()
            }
            Some(Upload::Tiled(image)) => self.gpu.upload_tiled(image),
            Some(Upload::Indexed {
                image,
                looks,
                atlas,
            }) => {
                let [cells, appearance, atlas] = self.gpu.upload_indexed(image, looks, atlas)?;
                appearance_upload_bytes = appearance;
                atlas_upload_bytes = atlas;
                cells
            }
            None => 0,
        };
        let overlay_upload_bytes = self.gpu.set_overlays(overlays);
        let sprite_upload_bytes = self.gpu.sprites.set(
            &self.gpu.queue,
            sprites,
            self.gpu.texture.width(),
            self.gpu.texture.height(),
        )?;
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
            appearance_upload_bytes,
            atlas_upload_bytes,
            overlay_upload_bytes,
            sprite_upload_bytes,
            acquire_ms,
            encode_present_ms: encode_start.elapsed().as_secs_f64() * 1000.0,
        }))
    }
}
#[derive(Clone, Copy, Default)]
pub struct RenderStats {
    pub upload_enqueue_ms: f64,
    pub upload_bytes: usize,
    pub appearance_upload_bytes: usize,
    pub atlas_upload_bytes: usize,
    pub overlay_upload_bytes: usize,
    pub sprite_upload_bytes: usize,
    pub acquire_ms: f64,
    pub encode_present_ms: f64,
}

enum Upload<'a> {
    Full(ImageView<'a>),
    Tiled(&'a TiledImage),
    Indexed {
        image: &'a IndexedImage,
        looks: &'a [TileAppearance],
        atlas: &'a Arc<charon_assets::Texture>,
    },
}
