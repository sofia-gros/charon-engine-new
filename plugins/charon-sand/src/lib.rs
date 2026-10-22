//! Sand plugin: immutable reads, parallel proposals, deterministic conflict commit.

use charon_image::TiledImage;
use charon_runtime::Simulation;
use rayon::prelude::*;
use std::time::Instant;

pub const AIR: u8 = 0;
pub const SAND: u8 = 1;
pub const WALL: u8 = 2;
const TILE: usize = 32;

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Paint { x: usize, y: usize, material: u8 },
    Clear,
}

pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub image: TiledImage,
    pub converted_cells: usize,
    pub sand: usize,
    pub active_tiles: usize,
    pub visited_cells: usize,
    pub workers_configured: usize,
    pub workers_used: u32,
    pub update_ms: f64,
    pub snapshot_ms: f64,
}

pub struct SandWorld {
    width: usize,
    height: usize,
    cells: Vec<u8>,
    sand: usize,
    image: TiledImage,
    dirty: Vec<bool>,
    awake: Vec<bool>,
    pool: rayon::ThreadPool,
    tick: u64,
    full_scan: bool,
}

fn priority(index: usize, tick: u64) -> u64 {
    let mut value = (index as u64) ^ tick.wrapping_mul(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value ^ (value >> 27)
}

impl SandWorld {
    /// Load a bounded material grid. Protected side/bottom walls must be present.
    pub fn from_cells(
        width: usize,
        height: usize,
        workers: usize,
        cells: Vec<u8>,
    ) -> Result<Self, String> {
        let mut world = Self::new(width, height, workers)?;
        if cells.len() != width * height
            || cells.iter().any(|&cell| cell > WALL)
            || (0..height).any(|y| cells[y * width] != WALL || cells[y * width + width - 1] != WALL)
            || cells[(height - 1) * width..]
                .iter()
                .any(|&cell| cell != WALL)
        {
            return Err("material grid must match dimensions and contain valid materials and protected walls".into());
        }
        world.sand = cells.iter().filter(|&&cell| cell == SAND).count();
        world.cells = cells;
        Ok(world)
    }

    /// Stable checksum of the authoritative materials, independent of worker order.
    pub fn checksum(&self) -> u64 {
        self.cells.iter().fold(0xcbf29ce484222325, |hash, &cell| {
            (hash ^ u64::from(cell)).wrapping_mul(0x100000001b3)
        })
    }

    pub fn new(width: usize, height: usize, workers: usize) -> Result<Self, String> {
        if !(8..=1920).contains(&width)
            || !(8..=1080).contains(&height)
            || !(1..=32).contains(&workers)
        {
            return Err("grid must be 8..1920 by 8..1080 and workers 1..32".into());
        }
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .thread_name(|i| format!("charon-sand-{i}"))
            .build()
            .map_err(|e| e.to_string())?;
        let mut world = Self {
            width,
            height,
            cells: vec![AIR; width * height],
            sand: 0,
            image: TiledImage::new(width, height, TILE),
            dirty: vec![true; width.div_ceil(TILE) * height.div_ceil(TILE)],
            awake: vec![true; width.div_ceil(TILE) * height.div_ceil(TILE)],
            pool,
            tick: 0,
            full_scan: false,
        };
        world.reset();
        Ok(world)
    }

    fn reset(&mut self) {
        self.cells.fill(AIR);
        self.sand = 0;
        self.dirty.fill(true);
        for y in 0..self.height {
            self.cells[y * self.width] = WALL;
            self.cells[y * self.width + self.width - 1] = WALL;
        }
        self.cells[(self.height - 1) * self.width..].fill(WALL);
        self.awake.fill(true);
    }

    fn wake(&mut self, index: usize) {
        let (x, y) = (index % self.width, index / self.width);
        let columns = self.width.div_ceil(TILE);
        self.dirty[y / TILE * columns + x / TILE] = true;
        for ty in y.saturating_sub(1) / TILE..=((y + 1).min(self.height - 1) / TILE) {
            for tx in x.saturating_sub(1) / TILE..=((x + 1).min(self.width - 1) / TILE) {
                self.awake[ty * columns + tx] = true;
            }
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Clear => self.reset(),
            Command::Paint { x, y, material } => {
                if ![AIR, SAND, WALL].contains(&material) {
                    return;
                }
                for py in y.saturating_sub(3)..=y.saturating_add(3).min(self.height - 1) {
                    for px in x.saturating_sub(3)..=x.saturating_add(3).min(self.width - 1) {
                        if px > 0 && px < self.width - 1 && py < self.height - 1 {
                            let index = py * self.width + px;
                            if self.cells[index] != material {
                                self.sand -= usize::from(self.cells[index] == SAND);
                                self.sand += usize::from(material == SAND);
                                self.cells[index] = material;
                                self.wake(index);
                            }
                        }
                    }
                }
            }
        }
    }

    fn advance(&mut self) -> (usize, usize, u32) {
        let columns = self.width.div_ceil(TILE);
        let cells = &self.cells;
        let tick = self.tick;
        let width = self.width;
        let height = self.height;
        let batches: Vec<_> = self.pool.install(|| {
            self.awake
                .par_iter()
                .enumerate()
                .filter_map(|(tile, awake)| {
                    if !awake && !self.full_scan {
                        return None;
                    }
                    let mut moves = Vec::new();
                    let mut visited = 0;
                    let (tx, ty) = ((tile % columns) * TILE, (tile / columns) * TILE);
                    for y in ty..(ty + TILE).min(height - 1) {
                        for x in tx.max(1)..(tx + TILE).min(width - 1) {
                            visited += 1;
                            let source = y * width + x;
                            if cells[source] != SAND {
                                continue;
                            }
                            let below = source + width;
                            let diagonals = if priority(source, tick) & 1 == 0 {
                                [below - 1, below + 1]
                            } else {
                                [below + 1, below - 1]
                            };
                            if let Some(destination) = [below, diagonals[0], diagonals[1]]
                                .into_iter()
                                .find(|&index| cells[index] == AIR)
                            {
                                moves.push((source, destination));
                            }
                        }
                    }
                    let worker = rayon::current_thread_index().unwrap_or(0);
                    Some((moves, visited, 1_u64 << worker))
                })
                .collect()
        });
        let active = batches.len();
        let visited = batches.iter().map(|batch| batch.1).sum();
        let workers = batches
            .iter()
            .fold(0, |mask, batch| mask | batch.2)
            .count_ones();
        let mut proposals: Vec<_> = batches.into_iter().flat_map(|batch| batch.0).collect();
        proposals.sort_unstable_by_key(|&(source, destination)| {
            (priority(source, tick), source, destination)
        });
        self.awake.fill(false);
        // A source is sand and a destination was air in the read snapshot, so
        // source/destination sets cannot overlap; only destination conflicts remain.
        for (source, destination) in proposals {
            if self.cells[destination] == AIR {
                self.cells[source] = AIR;
                self.cells[destination] = SAND;
                self.wake(source);
                self.wake(destination);
            }
        }
        self.tick += 1;
        (active, visited, workers)
    }
}

impl Simulation for SandWorld {
    type Command = Command;
    type Frame = Frame;
    fn step(&mut self, commands: Vec<Command>) -> Frame {
        let start = Instant::now();
        for command in commands {
            self.command(command);
        }
        let (active_tiles, visited_cells, workers_used) = self.advance();
        let update_ms = start.elapsed().as_secs_f64() * 1000.0;
        let snapshot_start = Instant::now();
        let mut converted_cells = 0;
        for tile in 0..self.dirty.len() {
            if !self.dirty[tile] {
                continue;
            }
            let rect = self.image.rect(tile);
            let mut rgba = Vec::with_capacity(rect.width * rect.height * 4);
            for y in rect.y..rect.y + rect.height {
                for x in rect.x..rect.x + rect.width {
                    rgba.extend_from_slice(&color(self.cells[y * self.width + x]));
                }
            }
            converted_cells += rect.width * rect.height;
            self.image.replace(tile, rgba);
            self.dirty[tile] = false;
        }
        Frame {
            width: self.width,
            height: self.height,
            image: self.image.clone(),
            converted_cells,
            sand: self.sand,
            active_tiles,
            visited_cells,
            workers_configured: self.pool.current_num_threads(),
            workers_used,
            update_ms,
            snapshot_ms: snapshot_start.elapsed().as_secs_f64() * 1000.0,
        }
    }
}

fn color(material: u8) -> [u8; 4] {
    match material {
        SAND => [232, 181, 72, 255],
        WALL => [82, 100, 121, 255],
        _ => [16, 23, 36, 255],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_image_matches_cells(world: &SandWorld, frame: &Frame) {
        let expected: Vec<_> = world.cells.iter().flat_map(|&cell| color(cell)).collect();
        assert_eq!(frame.image.to_rgba(), expected);
        assert_eq!(
            frame.sand,
            world.cells.iter().filter(|&&cell| cell == SAND).count()
        );
    }

    #[test]
    fn tiled_images_preserve_skipped_edits_and_old_snapshots() {
        let mut world = SandWorld::new(65, 66, 4).unwrap();
        let initial = world.step(vec![]);
        let original_pixels = initial.image.to_rgba();
        let edited = world.step(vec![Command::Paint {
            x: 4,
            y: 4,
            material: WALL,
        }]);
        let idle = world.step(vec![]);
        assert_eq!(idle.converted_cells, 0);
        assert!(
            !idle.image.shares_tile(&initial.image, 0),
            "skipped edit was lost"
        );
        assert!(idle.image.shares_tile(&edited.image, 0));
        assert!(
            edited.converted_cells < 65 * 66,
            "local edit converted the whole image"
        );
        for tile in 1..initial.image.tile_count() {
            assert!(idle.image.shares_tile(&initial.image, tile));
        }
        for tick in 0..180 {
            let commands = match tick {
                0 => vec![
                    Command::Paint {
                        x: 32,
                        y: 31,
                        material: WALL,
                    },
                    Command::Paint {
                        x: 31,
                        y: 15,
                        material: SAND,
                    },
                ],
                60 => vec![Command::Paint {
                    x: 32,
                    y: 31,
                    material: AIR,
                }],
                90 => vec![Command::Clear],
                100 => vec![Command::Paint {
                    x: 63,
                    y: 63,
                    material: SAND,
                }],
                _ => vec![],
            };
            let frame = world.step(commands);
            assert_image_matches_cells(&world, &frame);
        }
        assert_eq!(
            initial.image.to_rgba(),
            original_pixels,
            "published pixels mutated"
        );
        let clear = world.step(vec![Command::Clear]);
        assert_eq!(clear.image.to_rgba(), original_pixels);
        let idle = world.step(vec![]);
        assert_eq!(idle.converted_cells, 0);
        for tile in 0..idle.image.tile_count() {
            assert!(idle.image.shares_tile(&clear.image, tile));
        }
    }

    #[test]
    fn sleeping_beds_wake_across_tile_edges_without_losing_mass() {
        for (gap_x, floor_y) in [(31, 31), (32, 32), (63, 63), (64, 64)] {
            let mut world = SandWorld::new(130, 98, 4).unwrap();
            for y in floor_y - 8..floor_y {
                world.cells[y * 130 + 1..y * 130 + 129].fill(SAND);
            }
            world.cells[floor_y * 130 + 1..floor_y * 130 + 129].fill(WALL);
            world.advance();
            assert!(world.awake.iter().all(|awake| !awake));
            let mass = world.cells.iter().filter(|&&c| c == SAND).count();
            let mut reference = SandWorld::from_cells(130, 98, 1, world.cells.clone()).unwrap();
            reference.tick = world.tick;
            reference.full_scan = true;
            let opening = Command::Paint {
                x: gap_x,
                y: floor_y + 3,
                material: AIR,
            };
            world.command(opening);
            reference.command(opening);
            let before = world.checksum();
            world.advance();
            reference.advance();
            assert_ne!(world.checksum(), before, "sleeping support did not wake");
            assert_eq!(world.cells, reference.cells);
            for _ in 0..180 {
                world.advance();
                reference.advance();
                assert_eq!(world.cells, reference.cells);
                assert_eq!(world.cells.iter().filter(|&&c| c == SAND).count(), mass);
            }
        }
    }

    #[test]
    fn mixed_edits_match_reference_on_partial_tiles_for_each_worker_count() {
        for workers in [1, 2, 4] {
            for seed in [1_u64, 42, 999] {
                let mut world = SandWorld::new(65, 66, workers).unwrap();
                let mut reference = SandWorld::new(65, 66, 1).unwrap();
                reference.full_scan = true;
                let mut rng = seed;
                for tick in 0..120 {
                    for _ in 0..4 {
                        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                        let edit = Command::Paint {
                            x: (rng >> 16) as usize % 72,
                            y: (rng >> 32) as usize % 72,
                            material: ((rng >> 48) % 3) as u8,
                        };
                        world.command(edit);
                        reference.command(edit);
                    }
                    let mass = world.cells.iter().filter(|&&c| c == SAND).count();
                    world.advance();
                    reference.advance();
                    assert_eq!(
                        world.cells, reference.cells,
                        "workers={workers} seed={seed} tick={tick}"
                    );
                    assert_eq!(world.cells.iter().filter(|&&c| c == SAND).count(), mass);
                }
            }
        }
    }

    #[test]
    fn imported_cells_reject_incomplete_or_open_boundaries() {
        assert!(SandWorld::from_cells(8, 8, 1, vec![AIR; 63]).is_err());
        assert!(SandWorld::from_cells(8, 8, 1, vec![AIR; 64]).is_err());
        let mut world = SandWorld::new(8, 8, 1).unwrap();
        world.cells[10] = 255;
        assert!(SandWorld::from_cells(8, 8, 1, world.cells).is_err());
    }

    #[test]
    fn competing_grains_preserve_mass_and_walls() {
        let mut world = SandWorld::new(32, 32, 2).unwrap();
        world.cells[4 * 32 + 10] = SAND;
        world.cells[4 * 32 + 12] = SAND;
        for x in [9, 10, 12, 13] {
            world.cells[5 * 32 + x] = WALL;
        }
        let before = world.cells.iter().filter(|&&c| c == WALL).count();
        world.advance();
        assert_eq!(world.cells.iter().filter(|&&c| c == SAND).count(), 2);
        assert_eq!(world.cells[5 * 32 + 11], SAND);
        for _ in 0..100 {
            world.advance();
        }
        assert_eq!(world.cells.iter().filter(|&&c| c == SAND).count(), 2);
        assert_eq!(world.cells.iter().filter(|&&c| c == WALL).count(), before);
    }

    #[test]
    fn parallel_and_sleeping_updates_match_full_scan_after_boundary_edits() {
        let mut reference = SandWorld::new(96, 96, 1).unwrap();
        reference.full_scan = true;
        let mut parallel = SandWorld::new(96, 96, 4).unwrap();
        for tick in 0..320 {
            let commands = match tick {
                0 => vec![
                    Command::Paint {
                        x: 32,
                        y: 12,
                        material: SAND,
                    },
                    Command::Paint {
                        x: 31,
                        y: 63,
                        material: WALL,
                    },
                ],
                140 => vec![Command::Paint {
                    x: 31,
                    y: 63,
                    material: AIR,
                }],
                220 => vec![Command::Paint {
                    x: 64,
                    y: 20,
                    material: SAND,
                }],
                _ => vec![],
            };
            reference.step(commands.clone());
            parallel.step(commands);
            assert_eq!(
                reference.cells, parallel.cells,
                "different result at tick {tick}"
            );
        }
        let frame = parallel.step(vec![]);
        assert_eq!(frame.active_tiles, 0, "settled world did not sleep");
        assert_eq!(frame.visited_cells, 0);
    }

    #[test]
    fn out_of_bounds_input_is_ignored_and_clear_restores_world() {
        let mut world = SandWorld::new(16, 16, 1).unwrap();
        world.step(vec![Command::Paint {
            x: usize::MAX,
            y: usize::MAX,
            material: SAND,
        }]);
        assert_eq!(world.step(vec![]).sand, 0);
        assert!(
            world
                .step(vec![Command::Paint {
                    x: 8,
                    y: 4,
                    material: SAND
                }])
                .sand
                > 0
        );
        assert_eq!(world.step(vec![Command::Clear]).sand, 0);
        world.cells[8] = SAND;
        world.wake(8);
        world.advance();
        assert_eq!(world.cells[16 + 8], SAND, "top-row sand did not fall");
    }
}
