//! Game-owned cup shape and reference-frame motion; fluid physics stays in its plugin.
use crate::materials::{AIR, OIL, WALL, WATER};
use charon_fluid::{Config, Fluid, Stats};
use charon_image::{IndexedImage, TiledImage};
use charon_sand::{Command, ObstacleId, SandWorld};
#[derive(Clone, Copy, Debug)]
pub struct Status {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub moves: u64,
    pub blocked: u64,
    pub water: usize,
    pub oil: usize,
    pub fluid: Stats,
    pub surface_delta: f64,
    pub movable: bool,
    pub compute_backend: &'static str,
}
pub struct Cup {
    id: ObstacleId,
    x: i32,
    initial_x: i32,
    y: i32,
    width: i32,
    height: i32,
    phase: u8,
    moves: u64,
    blocked: u64,
    velocity: f64,
    movable: bool,
    error: Option<&'static str>,
    fluid: Fluid,
    grid_width: usize,
    raster: Vec<u8>,
    indices: IndexedImage,
    image: TiledImage,
    base: Option<IndexedImage>,
}
impl Cup {
    pub fn use_gpu(&mut self, gpu: std::sync::Arc<charon_fluid::gpu::GpuContext>) {
        self.fluid.use_gpu(gpu);
    }
    pub fn create(
        world: &mut SandWorld,
        width: usize,
        height: usize,
        workers: usize,
    ) -> Result<Self, &'static str> {
        Self::scene(world, width, height, workers, true)
    }
    pub fn dam(
        world: &mut SandWorld,
        width: usize,
        height: usize,
        workers: usize,
    ) -> Result<Self, &'static str> {
        Self::scene(world, width, height, workers, false)
    }
    fn scene(
        world: &mut SandWorld,
        width: usize,
        height: usize,
        workers: usize,
        movable: bool,
    ) -> Result<Self, &'static str> {
        if width < 128 || height < 100 {
            return Err("CUP NEEDS GRID 128X100");
        }
        let cw = if movable {
            (width / 4).clamp(24, 96)
        } else {
            (width / 3).clamp(32, 128)
        } as i32;
        let ch = if movable {
            (height / 4).clamp(20, 64)
        } else {
            (height / 3).clamp(24, 80)
        } as i32;
        let x = (width as i32 - cw) / 2;
        let y = height as i32 / 2 - if movable { 0 } else { ch / 4 };
        let mask: Vec<_> = (0..ch)
            .flat_map(|py| {
                (0..cw).filter_map(move |px| {
                    (px < 2
                        || px >= cw - 2
                        || py >= ch - 2
                        || (!movable && (cw / 2..cw / 2 + 3).contains(&px) && py > ch * 2 / 3))
                        .then_some([x + px, y + py])
                })
            })
            .collect();
        let mut solid = vec![false; width * height];
        for py in 0..height {
            for px in 0..width {
                solid[py * width + px] = px == 0 || py == 0 || px == width - 1 || py == height - 1;
            }
        }
        for &[px, py] in &mask {
            solid[py as usize * width + px as usize] = true;
        }
        let water = world
            .materials()
            .get(WATER)
            .filter(|m| m.motion == charon_sand::Motion::Liquid)
            .ok_or("WATER MATERIAL MISSING")?
            .density as f64;
        let oil = world
            .materials()
            .get(OIL)
            .filter(|m| m.motion == charon_sand::Motion::Liquid)
            .ok_or("OIL MATERIAL MISSING")?
            .density as f64;
        let mut fluid = Fluid::new(
            Config {
                width,
                height,
                workers,
                density: [1., oil / water],
                drag: 0.8,
                viscosity: 0.5,
                pressure_cutoff: 0.01,
                sleep_tolerance: 0.005,
            },
            solid,
        )
        .map_err(|_| "FLUID CONFIG FAILED")?;
        let fill_top = if movable { y + ch / 2 - 2 } else { y + 4 };
        let fill_right = if movable { x + cw - 2 } else { x + cw / 4 };
        for py in fill_top..y + ch - 2 {
            for px in x + 2..fill_right {
                fluid
                    .add(px as usize, py as usize, 0, 1.)
                    .map_err(|_| "FLUID FILL FAILED")?;
            }
        }
        world.apply_edits(vec![Command::Clear]);
        let id = world
            .add_obstacle(&mask, WALL)
            .map_err(|_| "CUP COULD NOT BE CREATED")?;
        Ok(Self {
            id,
            x,
            initial_x: x,
            y,
            width: cw,
            height: ch,
            phase: 0,
            moves: 0,
            blocked: 0,
            velocity: 0.,
            movable,
            error: None,
            fluid,
            grid_width: width,
            raster: vec![AIR; width * height],
            indices: IndexedImage::new(width, height, 32),
            image: TiledImage::new(width, height, 32),
            base: None,
        })
    }
    pub fn add_oil(&mut self) {
        for y in self.y + 3..self.y + 3 + (self.height / 8).max(1) {
            for x in self.initial_x + 2..self.initial_x + self.width - 2 {
                let f = self.fluid.fraction(x as usize, y as usize).unwrap();
                let space = (1. - f[0] - f[1]).max(0.);
                if space > 1e-8 {
                    self.fluid
                        .add(x as usize, y as usize, 1, space)
                        .expect("valid empty cup cell");
                }
            }
        }
    }
    pub fn step(&mut self, world: &mut SandWorld, direction: i8) -> Option<&'static str> {
        let direction = if self.movable { direction } else { 0 };
        let mut velocity = f64::from(direction) * 15.;
        let mut moved = false;
        if direction == 0 {
            self.phase = 0;
            self.error = None;
        } else {
            self.phase = (self.phase + 1) % 4;
            if self.phase == 0 {
                match world.move_obstacle(self.id, [direction as i32, 0]) {
                    Ok(()) => {
                        self.x += direction as i32;
                        self.moves += 1;
                        moved = true;
                        self.error = None;
                    }
                    Err(_) => {
                        self.blocked += 1;
                        velocity = 0.;
                        self.error = Some("CUP BLOCKED - LIQUID PRESERVED");
                    }
                }
            }
        }
        // Solve in the translating cup frame: gravity and the negative frame
        // acceleration act on the entire velocity field. Constant translation
        // injects no force; stopping/reversing transfers momentum naturally.
        if let Err(e) = self
            .fluid
            .step(1. / 60., [-(velocity - self.velocity) * 60., 180.])
        {
            if moved {
                world
                    .move_obstacle(self.id, [-direction as i32, 0])
                    .expect("reverse unchanged empty boundary");
                self.x -= direction as i32;
                self.moves -= 1;
            }
            eprintln!("Fluid step rejected: {e:?}");
            self.error = Some("FLUID SOLVE FAILED - STATE PRESERVED");
        } else {
            self.velocity = velocity;
        }
        self.error
    }
    pub fn material_at(&self, x: i32, y: i32) -> Option<u8> {
        let x = x - (self.x - self.initial_x);
        if x < 0 || y < 0 {
            return None;
        }
        self.fluid
            .fraction(x as usize, y as usize)
            .and_then(|f| (f[0] + f[1] >= 0.15).then_some(if f[0] >= f[1] { WATER } else { OIL }))
    }
    pub fn status(&self, _world: &SandWorld) -> Status {
        let volume = self.fluid.stats().volume;
        let columns = self
            .fluid
            .column_volumes([
                (self.initial_x + 2) as usize,
                self.y as usize,
                (self.width - 4) as usize,
                (self.height - 2) as usize,
            ])
            .unwrap();
        let band = (columns.len() / 4).max(1);
        let left = columns[..band].iter().map(|f| f[0] + f[1]).sum::<f64>() / band as f64;
        let right = columns[columns.len() - band..]
            .iter()
            .map(|f| f[0] + f[1])
            .sum::<f64>()
            / band as f64;
        Status {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            moves: self.moves,
            blocked: self.blocked,
            water: volume[0].round() as usize,
            oil: volume[1].round() as usize,
            fluid: self.fluid.stats(),
            surface_delta: left - right,
            movable: self.movable,
            compute_backend: self.fluid.backend_label(),
        }
    }
    pub fn signature(&self) -> [u64; 6] {
        [
            self.x as u64,
            self.y as u64,
            self.phase as u64,
            self.moves,
            self.velocity.to_bits(),
            self.fluid.signature(),
        ]
    }
    pub fn compose(&mut self, frame: &mut charon_sand::Frame) {
        let started = std::time::Instant::now();
        let w = self.grid_width;
        let dx = self.x - self.initial_x;
        let next = self.fluid.rasterize(0.15, dx, [AIR, WATER, OIL]);
        let mut changed = vec![false; frame.indices.tile_count()];
        for (tile, dirty) in changed.iter_mut().enumerate() {
            let r = frame.indices.rect(tile);
            *dirty = (r.y..r.y + r.height).any(|y| {
                next[y * w + r.x..y * w + r.x + r.width]
                    != self.raster[y * w + r.x..y * w + r.x + r.width]
            });
        }
        let mut converted = 0;
        for (tile, dirty) in changed.into_iter().enumerate() {
            if !dirty
                && self
                    .base
                    .as_ref()
                    .is_some_and(|b| b.shares_tile(&frame.indices, tile))
            {
                continue;
            }
            let r = frame.indices.rect(tile);
            let base = frame.indices.pixels(tile);
            let mut ids = Vec::with_capacity(base.len());
            for y in r.y..r.y + r.height {
                for x in r.x..r.x + r.width {
                    let b = base[(y - r.y) * r.width + x - r.x];
                    ids.push(if b == AIR && next[y * w + x] != AIR {
                        next[y * w + x]
                    } else {
                        b
                    });
                }
            }
            if self.base.is_none() || self.indices.pixels(tile) != ids {
                let rgba = ids
                    .iter()
                    .flat_map(|&id| frame.materials.get(id).unwrap().color)
                    .collect();
                self.indices.replace(tile, ids);
                self.image.replace(tile, rgba);
                converted += r.width * r.height;
            }
        }
        self.base = Some(frame.indices.clone());
        self.raster = next;
        frame.indices = self.indices.clone();
        frame.image = self.image.clone();
        frame.converted_cells += converted;
        let s = self.fluid.stats();
        frame.liquid = s.volume.iter().sum::<f64>().round() as usize;
        frame.visited_cells += if s.sleeping { 0 } else { s.wet_cells };
        frame.active_tiles += if s.sleeping { 0 } else { s.active_tiles };
        frame.workers_used = frame.workers_used.max(s.workers_used);
        frame.snapshot_ms += started.elapsed().as_secs_f64() * 1000.;
    }
}
