//! Optional cached cell-resolution light field. Material optics and sources are
//! supplied by the game; this module has no sand, collision or gameplay policy.
use crate::{Camera2D, Result};
use charon_image::Rect;
const TILE: u32 = 32;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightMaterial {
    pub emission: [f32; 3],
    pub opacity: f32,
}
impl Default for LightMaterial {
    fn default() -> Self {
        Self {
            emission: [0.; 3],
            opacity: 0.,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointLight {
    pub position: [f32; 2],
    pub color: [f32; 3],
    pub radius: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Lighting {
    pub materials: Vec<LightMaterial>,
    pub lights: Vec<PointLight>,
    pub ambient: f32,
    pub interior_darkness: f32,
    pub interior_radius: u32,
}
impl Lighting {
    pub fn validate(&self) -> Result<()> {
        if self.materials.len() != 256
            || self.lights.len() > 32
            || !self.ambient.is_finite()
            || !(0.0..=1.0).contains(&self.ambient)
            || !self.interior_darkness.is_finite()
            || !(0.0..=1.0).contains(&self.interior_darkness)
            || !(1..=16).contains(&self.interior_radius)
            || self.materials.iter().any(|m| {
                !m.opacity.is_finite()
                    || !(0.0..=1.0).contains(&m.opacity)
                    || m.emission
                        .iter()
                        .any(|v| !v.is_finite() || !(0.0..=16.0).contains(v))
            })
            || self.lights.iter().any(|l| {
                !l.radius.is_finite()
                    || !(1.0..=128.0).contains(&l.radius)
                    || l.position.iter().any(|v| !v.is_finite() || v.abs() > 1e6)
                    || l.color
                        .iter()
                        .any(|v| !v.is_finite() || !(0.0..=16.0).contains(v))
            })
        {
            return Err("lighting requires 256 optical materials, <=32 lights, finite values, radius <=128 and interior radius <=16".into());
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Default, Debug)]
pub struct LightingStats {
    pub cells_updated: usize,
    pub dispatches: usize,
    pub lights: usize,
    /// Latest completed compute measurement; None when timestamps are unsupported.
    pub gpu_ms: Option<f64>,
}
pub(crate) struct LightField {
    pub texture: wgpu::Texture,
    pub enabled: wgpu::Buffer,
    optics: wgpu::Buffer,
    settings: wgpu::Buffer,
    tiles: wgpu::Buffer,
    pipeline: Option<wgpu::ComputePipeline>,
    bind: Option<wgpu::BindGroup>,
    terrain: wgpu::TextureView,
    config: Option<Lighting>,
    dirty: Vec<bool>,
    width: u32,
    height: u32,
    pub stats: LightingStats,
    timer: Option<Timer>,
}
struct Timer {
    query: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    pending: bool,
    encoded: bool,
    rx: std::sync::mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
    tx: std::sync::mpsc::Sender<std::result::Result<(), wgpu::BufferAsyncError>>,
    period: f64,
}
impl LightField {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, terrain: &wgpu::Texture) -> Self {
        let (width, height) = (terrain.width(), terrain.height());
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cached cell light field"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let optics = buffer(
            "optical materials",
            4096,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let settings = buffer(
            "lighting sources",
            1040,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let tiles = buffer(
            "dirty light tiles",
            u64::from(width.div_ceil(TILE) * height.div_ceil(TILE)) * 8,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let enabled = buffer(
            "lighting enabled",
            16,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let timer = if device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            let (tx, rx) = std::sync::mpsc::channel();
            Some(Timer {
                query: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("lighting timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: 2,
                }),
                resolve: buffer(
                    "lighting timestamp resolve",
                    256,
                    wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                ),
                read: buffer(
                    "lighting timestamp readback",
                    16,
                    wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                ),
                pending: false,
                encoded: false,
                rx,
                tx,
                period: f64::from(queue.get_timestamp_period()),
            })
        } else {
            None
        };
        Self {
            texture,
            enabled,
            optics,
            settings,
            tiles,
            pipeline: None,
            bind: None,
            terrain: terrain.create_view(&Default::default()),
            config: None,
            dirty: vec![true; (width.div_ceil(TILE) * height.div_ceil(TILE)) as usize],
            width,
            height,
            stats: LightingStats::default(),
            timer,
        }
    }
    fn initialize(&mut self, device: &wgpu::Device) {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("2D lighting"),
            source: wgpu::ShaderSource::Wgsl(include_str!("lighting.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("2D direct lighting"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let terrain_view = self.terrain.clone();
        let light_view = self.texture.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lighting compute"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&terrain_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.optics.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.settings.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.tiles.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&light_view),
                },
            ],
        });
        self.pipeline = Some(pipeline);
        self.bind = Some(bind);
    }
    pub fn configure(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: Option<&Lighting>,
    ) -> Result<()> {
        if self.config.as_ref() == config {
            return Ok(());
        }
        if let Some(config) = config {
            config.validate()?;
            if self.pipeline.is_none() {
                self.initialize(device);
            }
            let optical: Vec<_> = config
                .materials
                .iter()
                .flat_map(|m| m.emission.into_iter().chain([m.opacity]))
                .flat_map(f32::to_ne_bytes)
                .collect();
            queue.write_buffer(&self.optics, 0, &optical);
            let mut data: Vec<f32> = vec![
                config.ambient,
                config.interior_darkness,
                config.interior_radius as f32,
                config.lights.len() as f32,
            ];
            for light in &config.lights {
                data.extend([
                    light.position[0],
                    light.position[1],
                    light.radius,
                    0.,
                    light.color[0],
                    light.color[1],
                    light.color[2],
                    0.,
                ]);
            }
            data.resize(260, 0.);
            queue.write_buffer(
                &self.settings,
                0,
                &data
                    .into_iter()
                    .flat_map(f32::to_ne_bytes)
                    .collect::<Vec<_>>(),
            );
        }
        queue.write_buffer(&self.enabled, 0, &u32::from(config.is_some()).to_ne_bytes());
        self.config = config.cloned();
        self.dirty.fill(true);
        Ok(())
    }
    pub fn invalidate(&mut self, rect: Rect) {
        let Some(config) = &self.config else {
            return;
        };
        let radius = config
            .lights
            .iter()
            .map(|l| l.radius.ceil() as usize)
            .max()
            .unwrap_or(0)
            .max(config.interior_radius as usize);
        let x0 = rect.x.saturating_sub(radius) as u32 / TILE;
        let y0 = rect.y.saturating_sub(radius) as u32 / TILE;
        let x1 = (rect.x + rect.width + radius)
            .min(self.width as usize)
            .saturating_sub(1) as u32
            / TILE;
        let y1 = (rect.y + rect.height + radius)
            .min(self.height as usize)
            .saturating_sub(1) as u32
            / TILE;
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.dirty[(y * self.width.div_ceil(TILE) + x) as usize] = true;
            }
        }
    }
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: Camera2D,
    ) {
        let _ = device.poll(wgpu::PollType::Poll);
        if let Some(timer) = &mut self.timer {
            if let Ok(result) = timer.rx.try_recv() {
                if result.is_ok() {
                    let range = timer
                        .read
                        .get_mapped_range(..)
                        .expect("completed lighting timestamp mapping");
                    let a = u64::from_ne_bytes(range[..8].try_into().unwrap());
                    let b = u64::from_ne_bytes(range[8..16].try_into().unwrap());
                    self.stats.gpu_ms = Some(b.saturating_sub(a) as f64 * timer.period / 1e6);
                    drop(range);
                    timer.read.unmap();
                }
                timer.pending = false;
            }
        }
        self.stats.cells_updated = 0;
        self.stats.dispatches = 0;
        self.stats.lights = self.config.as_ref().map_or(0, |c| c.lights.len());
        if self.config.is_none() {
            return;
        }
        let x0 = (camera.origin()[0].floor().max(0.) as u32 / TILE).min(self.width.div_ceil(TILE));
        let y0 = (camera.origin()[1].floor().max(0.) as u32 / TILE).min(self.height.div_ceil(TILE));
        let x1 = ((camera.origin()[0] + camera.size()[0]).ceil().max(0.) as u32)
            .min(self.width)
            .div_ceil(TILE);
        let y1 = ((camera.origin()[1] + camera.size()[1]).ceil().max(0.) as u32)
            .min(self.height)
            .div_ceil(TILE);
        let mut tiles: Vec<u32> = vec![];
        for y in y0..y1 {
            for x in x0..x1 {
                let index = (y * self.width.div_ceil(TILE) + x) as usize;
                if self.dirty[index] {
                    self.dirty[index] = false;
                    tiles.extend([x * TILE, y * TILE]);
                    self.stats.cells_updated += ((self.width - x * TILE).min(TILE)
                        * (self.height - y * TILE).min(TILE))
                        as usize;
                }
            }
        }
        if tiles.is_empty() {
            return;
        }
        queue.write_buffer(
            &self.tiles,
            0,
            &tiles
                .iter()
                .flat_map(|v| v.to_ne_bytes())
                .collect::<Vec<_>>(),
        );
        let timer = self.timer.as_mut().filter(|t| !t.pending);
        let timestamp_writes = timer.as_ref().map(|t| wgpu::ComputePassTimestampWrites {
            query_set: &t.query,
            beginning_of_pass_write_index: Some(0),
            end_of_pass_write_index: Some(1),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("dirty lighting tiles"),
                timestamp_writes,
            });
            pass.set_pipeline(self.pipeline.as_ref().expect("enabled lighting pipeline"));
            pass.set_bind_group(
                0,
                self.bind.as_ref().expect("enabled lighting bindings"),
                &[],
            );
            pass.dispatch_workgroups(4, 4, (tiles.len() / 2) as u32);
        }
        if let Some(timer) = timer {
            encoder.resolve_query_set(&timer.query, 0..2, &timer.resolve, 0);
            encoder.copy_buffer_to_buffer(&timer.resolve, 0, &timer.read, 0, 16);
            timer.pending = true;
            timer.encoded = true;
        }
        self.stats.dispatches = 1;
    }
    pub fn after_submit(&mut self) {
        if let Some(timer) = &mut self.timer {
            if timer.encoded {
                let tx = timer.tx.clone();
                timer
                    .read
                    .map_async(wgpu::MapMode::Read, .., move |result| {
                        let _ = tx.send(result);
                    });
                timer.encoded = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optical_configuration_rejects_unbounded_or_nonfinite_work() {
        let base = Lighting {
            materials: vec![LightMaterial::default(); 256],
            lights: vec![],
            ambient: 0.8,
            interior_darkness: 0.9,
            interior_radius: 16,
        };
        assert!(base.validate().is_ok());
        let light = PointLight {
            position: [2., 2.],
            color: [1.; 3],
            radius: 64.,
        };
        let mut bad = base.clone();
        bad.lights = vec![light; 33];
        assert!(bad.validate().is_err());
        bad = base.clone();
        bad.materials[0].opacity = f32::NAN;
        assert!(bad.validate().is_err());
        bad = base.clone();
        bad.interior_radius = 17;
        assert!(bad.validate().is_err());
        bad = base;
        bad.lights.push(PointLight {
            radius: 129.,
            ..light
        });
        assert!(bad.validate().is_err());
    }
}
