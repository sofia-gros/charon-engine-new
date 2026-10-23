use crate::Result;
use charon_assets::Texture;
use charon_image::IndexedImage;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileAppearance {
    pub color: [u8; 4],
    /// Repeating source rectangle in the shared atlas; None uses the solid color.
    pub source: Option<[u32; 4]>,
}

pub(crate) struct IndexedLayer {
    pipeline: wgpu::RenderPipeline,
    cells: wgpu::Texture,
    palette: wgpu::Buffer,
    bind: wgpu::BindGroup,
    image: Option<IndexedImage>,
    atlas: Option<Arc<Texture>>,
    appearances: Vec<TileAppearance>,
}
impl IndexedLayer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("indexed terrain"),
            source: wgpu::ShaderSource::Wgsl(include_str!("indexed.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("indexed terrain"),
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
        let make_texture = |width, height, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("indexed terrain texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let cells = make_texture(width, height, wgpu::TextureFormat::R8Uint);
        let atlas = make_texture(1, 1, wgpu::TextureFormat::Rgba8Unorm);
        let palette = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("material appearance table"),
            size: 256 * 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = Self::bind(device, &pipeline, &cells, &palette, &atlas);
        Self {
            pipeline,
            cells,
            palette,
            bind,
            image: None,
            atlas: None,
            appearances: vec![],
        }
    }
    fn bind(
        device: &wgpu::Device,
        pipeline: &wgpu::RenderPipeline,
        cells: &wgpu::Texture,
        palette: &wgpu::Buffer,
        atlas: &wgpu::Texture,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("indexed terrain"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &cells.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: palette.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &atlas.create_view(&Default::default()),
                    ),
                },
            ],
        })
    }
    /// Returns cell, appearance-table and atlas upload byte counts separately.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &IndexedImage,
        looks: &[TileAppearance],
        atlas: &Arc<Texture>,
    ) -> Result<[usize; 3]> {
        if image.width() != self.cells.width() as usize
            || image.height() != self.cells.height() as usize
            || looks.len() != 256
        {
            return Err(
                "indexed image dimensions must match; appearance table needs 256 entries".into(),
            );
        }
        for look in looks {
            if let Some([x, y, w, h]) = look.source {
                if w == 0
                    || h == 0
                    || x.checked_add(w).is_none_or(|v| v > atlas.width())
                    || y.checked_add(h).is_none_or(|v| v > atlas.height())
                {
                    return Err("material appearance outside atlas".into());
                }
            }
        }
        let mut bytes = [0; 3];
        if !self
            .atlas
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, atlas))
        {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("material atlas"),
                size: wgpu::Extent3d {
                    width: atlas.width(),
                    height: atlas.height(),
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
                atlas.rgba(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(atlas.width() * 4),
                    rows_per_image: None,
                },
                texture.size(),
            );
            self.bind = Self::bind(device, &self.pipeline, &self.cells, &self.palette, &texture);
            self.atlas = Some(Arc::clone(atlas));
            bytes[2] = atlas.rgba().len();
        }
        if looks != self.appearances {
            let mut data = Vec::with_capacity(256 * 32);
            for look in looks {
                for value in look
                    .color
                    .map(u32::from)
                    .into_iter()
                    .chain(look.source.unwrap_or([0; 4]))
                {
                    data.extend_from_slice(&value.to_ne_bytes());
                }
            }
            queue.write_buffer(&self.palette, 0, &data);
            bytes[1] = data.len();
            self.appearances = looks.to_vec();
        }
        for index in 0..image.tile_count() {
            if self
                .image
                .as_ref()
                .is_some_and(|old| image.shares_tile(old, index))
            {
                continue;
            }
            let rect = image.rect(index);
            let mut target = self.cells.as_image_copy();
            target.origin = wgpu::Origin3d {
                x: rect.x as u32,
                y: rect.y as u32,
                z: 0,
            };
            queue.write_texture(
                target,
                image.pixels(index),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(rect.width as u32),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: rect.width as u32,
                    height: rect.height as u32,
                    depth_or_array_layers: 1,
                },
            );
            bytes[0] += image.pixels(index).len();
        }
        if bytes[0] > 0 || self.image.is_none() {
            self.image = Some(image.clone());
        }
        Ok(bytes)
    }
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
