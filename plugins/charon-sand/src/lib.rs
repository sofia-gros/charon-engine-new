//! Sand plugin: immutable reads, parallel proposals, deterministic conflict commit.

use charon_image::{IndexedImage, TiledImage};
use charon_runtime::Simulation;
use rayon::prelude::*;
use std::{collections::BTreeMap, sync::Arc, time::Instant};
type DensityRoutes = BTreeMap<u16, Vec<[Option<usize>; 2]>>;

mod material;
mod obstacle;
pub use material::{Material, Materials, Motion};
pub use obstacle::{ObstacleError, ObstacleId};

pub const AIR: u8 = 0;
pub const SAND: u8 = 1;
pub const WALL: u8 = 2;
const TILE: usize = 32;

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Paint {
        x: usize,
        y: usize,
        material: u8,
    },
    FillRect {
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        material: u8,
    },
    Clear,
}

pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub image: TiledImage,
    pub indices: IndexedImage,
    pub materials: Arc<Materials>,
    pub converted_cells: usize,
    /// Total powder cells, including custom powders (legacy field name).
    pub sand: usize,
    /// Total liquid cells; material identity and collision remain game-defined.
    pub liquid: usize,
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
    claimed: Vec<u64>,
    obstacles: obstacle::Obstacles,
    materials: Arc<Materials>,
    sand: usize,
    liquid: usize,
    counts: [usize; 256],
    liquid_rows: Vec<usize>,
    image: TiledImage,
    indices: IndexedImage,
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
        Self::from_material_cells(width, height, workers, cells, Materials::default())
    }

    /// Assemble a sand plugin with game-owned material definitions and a bounded grid.
    pub fn from_material_cells(
        width: usize,
        height: usize,
        workers: usize,
        cells: Vec<u8>,
        materials: Materials,
    ) -> Result<Self, String> {
        let mut world = Self::with_materials(width, height, workers, materials)?;
        if cells.len() != width * height
            || cells
                .iter()
                .any(|&cell| world.materials.get(cell).is_none())
            || (0..height).any(|y| {
                cells[y * width] != world.materials.boundary()
                    || cells[y * width + width - 1] != world.materials.boundary()
            })
            || cells[(height - 1) * width..]
                .iter()
                .any(|&cell| cell != world.materials.boundary())
        {
            return Err("material grid must match dimensions and contain valid materials and protected walls".into());
        }
        world.sand = cells.iter().filter(|&&cell| world.is_powder(cell)).count();
        world.liquid = cells.iter().filter(|&&cell| world.is_liquid(cell)).count();
        for (index, &id) in cells.iter().enumerate() {
            world.liquid_rows[index / width] += usize::from(world.is_liquid(id));
        }
        world.counts.fill(0);
        for &id in &cells {
            world.counts[id as usize] += 1;
        }
        world.cells = cells;
        Ok(world)
    }

    /// Read authoritative material on the simulation owner thread. Out of bounds is None.
    /// Games decide which materials block actors, projectiles, or other systems.
    pub fn material_at(&self, x: i32, y: i32) -> Option<u8> {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            None
        } else {
            Some(self.cells[y as usize * self.width + x as usize])
        }
    }

    /// Stable checksum of the authoritative materials, independent of worker order.
    pub fn checksum(&self) -> u64 {
        self.cells.iter().fold(0xcbf29ce484222325, |hash, &cell| {
            (hash ^ u64::from(cell)).wrapping_mul(0x100000001b3)
        })
    }

    pub fn new(width: usize, height: usize, workers: usize) -> Result<Self, String> {
        Self::with_materials(width, height, workers, Materials::default())
    }

    /// Cached material counts, updated by edits and conserving physical moves.
    pub fn material_count(&self, id: u8) -> usize {
        self.counts[id as usize]
    }
    pub fn materials(&self) -> &Materials {
        &self.materials
    }

    fn is_powder(&self, id: u8) -> bool {
        self.materials
            .get(id)
            .is_some_and(|m| m.motion == Motion::Powder)
    }

    fn is_liquid(&self, id: u8) -> bool {
        self.materials
            .get(id)
            .is_some_and(|m| m.motion == Motion::Liquid)
    }

    pub fn with_materials(
        width: usize,
        height: usize,
        workers: usize,
        materials: Materials,
    ) -> Result<Self, String> {
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
            cells: vec![materials.air(); width * height],
            claimed: vec![0; width * height],
            obstacles: obstacle::Obstacles::default(),
            liquid_rows: vec![0; height],
            materials: Arc::new(materials),
            sand: 0,
            liquid: 0,
            counts: [0; 256],
            image: TiledImage::new(width, height, TILE),
            indices: IndexedImage::new(width, height, TILE),
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
        self.obstacles.clear();
        self.cells.fill(self.materials.air());
        self.sand = 0;
        self.liquid = 0;
        self.liquid_rows.fill(0);
        self.dirty.fill(true);
        for y in 0..self.height {
            self.cells[y * self.width] = self.materials.boundary();
            self.cells[y * self.width + self.width - 1] = self.materials.boundary();
        }
        self.cells[(self.height - 1) * self.width..].fill(self.materials.boundary());
        self.counts.fill(0);
        let walls = 2 * self.height + self.width - 2;
        self.counts[self.materials.boundary() as usize] = walls;
        self.counts[self.materials.air() as usize] = self.width * self.height - walls;
        self.awake.fill(true);
    }

    fn wake(&mut self, index: usize) {
        let (x, y) = (index % self.width, index / self.width);
        let columns = self.width.div_ceil(TILE);
        self.dirty[y / TILE * columns + x / TILE] = true;
        // A changed cell can open a route for a distant liquid on the same or
        // neighboring row. Wake the horizontal strip only when it contains fluid;
        // deeper rows and unrelated powder-only areas retain local sleeping.
        let fluid_nearby = self.liquid_rows[y.saturating_sub(1)..=(y + 1).min(self.height - 1)]
            .iter()
            .any(|&n| n > 0);
        let (left, right) = if fluid_nearby {
            (0, columns - 1)
        } else {
            (
                x.saturating_sub(1) / TILE,
                (x + 1).min(self.width - 1) / TILE,
            )
        };
        for ty in y.saturating_sub(1) / TILE..=((y + 1).min(self.height - 1) / TILE) {
            for tx in left..=right {
                self.awake[ty * columns + tx] = true;
            }
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Clear => self.reset(),
            Command::Paint { x, y, material } => self.paint_rect(
                x.saturating_sub(3),
                y.saturating_sub(3),
                x.saturating_add(4),
                y.saturating_add(4),
                material,
            ),
            Command::FillRect {
                x,
                y,
                width,
                height,
                material,
            } => self.paint_rect(
                x,
                y,
                x.saturating_add(width),
                y.saturating_add(height),
                material,
            ),
        }
    }
    fn paint_rect(&mut self, x: usize, y: usize, end_x: usize, end_y: usize, material: u8) {
        if self.materials.get(material).is_none() {
            return;
        }
        for py in y..end_y.min(self.height - 1) {
            for px in x.max(1)..end_x.min(self.width - 1) {
                let index = py * self.width + px;
                if self.cells[index] != material && !self.obstacles.contains(index) {
                    self.sand -= usize::from(self.is_powder(self.cells[index]));
                    self.sand += usize::from(self.is_powder(material));
                    self.liquid -= usize::from(self.is_liquid(self.cells[index]));
                    self.liquid += usize::from(self.is_liquid(material));
                    self.liquid_rows[py] -= usize::from(self.is_liquid(self.cells[index]));
                    self.liquid_rows[py] += usize::from(self.is_liquid(material));
                    self.counts[self.cells[index] as usize] -= 1;
                    self.counts[material as usize] += 1;
                    self.cells[index] = material;
                    self.wake(index);
                }
            }
        }
    }

    fn can_fall_into(&self, source: u8, target: u8) -> bool {
        target == self.materials.air()
            || self.materials.get(target).is_some_and(|other| {
                other.motion == Motion::Liquid
                    && self.materials.get(source).unwrap().density > other.density
            })
    }
    fn liquid_destination(
        &self,
        source: usize,
        tick: u64,
        routes: Option<&DensityRoutes>,
    ) -> Option<usize> {
        let air = self.materials.air();
        let below = source + self.width;
        if self.can_fall_into(self.cells[source], self.cells[below]) {
            return Some(below);
        }
        let x = source % self.width;
        let directions = if priority(source, tick) & 1 == 0 {
            [-1, 1]
        } else {
            [1, -1]
        };
        // A heavier liquid must also be able to sink diagonally at a vertical
        // interface. Otherwise side-by-side oil/water columns falsely sleep,
        // despite both materials being vertically sorted within each column.
        for direction in directions {
            let side = (source as isize + direction) as usize;
            let target = side + self.width;
            if self.is_liquid(self.cells[target])
                && self.can_fall_into(self.cells[source], self.cells[target])
                && matches!(
                    self.materials.get(self.cells[side]).unwrap().motion,
                    Motion::Empty | Motion::Liquid
                )
            {
                return Some(target);
            }
        }
        // Connected liquid transmits displacement to a lower endpoint. This
        // levels density layers and fills the trailing space of moving walls.
        if let Some(routes) = routes
            .and_then(|routes| routes.get(&self.materials.get(self.cells[source]).unwrap().density))
        {
            for direction in directions {
                if let Some(target) = routes[x][usize::from(direction > 0)] {
                    return Some(target);
                }
            }
        }
        // A filled cell above supplies a local head. Move sideways only into
        // a lower column (empty cell above the target); a flat surface cannot
        // random-walk forever. This local head does not model pressure in closed pipes.
        if source >= self.width
            && matches!(
                self.materials
                    .get(self.cells[source - self.width])
                    .unwrap()
                    .motion,
                Motion::Liquid | Motion::Powder
            )
        {
            for direction in directions {
                let target = (source as isize + direction) as usize;
                if self.cells[target] == air && self.cells[target - self.width] == air {
                    return Some(target);
                }
            }
        }
        None
    }

    fn flow_routes(&self) -> Vec<Option<DensityRoutes>> {
        let mut densities: Vec<_> = (0..=255)
            .filter_map(|id| {
                self.materials
                    .get(id)
                    .filter(|m| m.motion == Motion::Liquid && self.counts[id as usize] > 0)
                    .map(|m| m.density)
            })
            .collect();
        densities.sort_unstable();
        densities.dedup();
        let columns = self.width.div_ceil(TILE);
        (0..self.height - 1)
            .map(|y| {
                if self.liquid_rows[y] == 0
                    || (!self.full_scan
                        && !self.awake[y / TILE * columns..(y / TILE + 1) * columns]
                            .iter()
                            .any(|&v| v))
                {
                    return None;
                }
                let row = &self.cells[y * self.width..(y + 1) * self.width];
                let below = &self.cells[(y + 1) * self.width..(y + 2) * self.width];
                let mut plans = BTreeMap::new();
                for &density in &densities {
                    if !row.iter().any(|&id| {
                        self.is_liquid(id) && self.materials.get(id).unwrap().density == density
                    }) || !below.iter().any(|&id| {
                        id == self.materials.air()
                            || (self.is_liquid(id)
                                && self.materials.get(id).unwrap().density < density)
                    }) {
                        continue;
                    }
                    let mut routes = vec![[None; 2]; self.width];
                    for (direction, reverse) in [false, true].into_iter().enumerate() {
                        let mut drop = None;
                        for i in 0..self.width {
                            let x = if reverse { self.width - 1 - i } else { i };
                            routes[x][direction] = drop;
                            let material = self.materials.get(row[x]).unwrap();
                            if material.motion != Motion::Empty
                                && (material.motion != Motion::Liquid || material.density > density)
                            {
                                drop = None;
                            } else if below[x] == self.materials.air()
                                || (self.is_liquid(below[x])
                                    && self.materials.get(below[x]).unwrap().density < density)
                            {
                                drop = Some((y + 1) * self.width + x);
                            }
                        }
                    }
                    plans.insert(density, routes);
                }
                (!plans.is_empty()).then_some(plans)
            })
            .collect()
    }

    fn advance(&mut self) -> (usize, usize, u32) {
        if !self.full_scan && !self.awake.iter().any(|&v| v) {
            self.tick += 1;
            return (0, 0, 0);
        }
        let columns = self.width.div_ceil(TILE);
        let cells = &self.cells;
        let tick = self.tick;
        let width = self.width;
        let height = self.height;
        let routes = self.flow_routes();
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
                    for (y, row_routes) in routes
                        .iter()
                        .enumerate()
                        .take((ty + TILE).min(height - 1))
                        .skip(ty)
                    {
                        for x in tx.max(1)..(tx + TILE).min(width - 1) {
                            visited += 1;
                            let source = y * width + x;
                            let destination = match self
                                .materials
                                .get(cells[source])
                                .expect("validated cell")
                                .motion
                            {
                                Motion::Powder => {
                                    let below = source + width;
                                    let diagonals = if priority(source, tick) & 1 == 0 {
                                        [below - 1, below + 1]
                                    } else {
                                        [below + 1, below - 1]
                                    };
                                    [below, diagonals[0], diagonals[1]]
                                        .into_iter()
                                        .find(|&index| {
                                            self.can_fall_into(cells[source], cells[index])
                                        })
                                }
                                Motion::Liquid => {
                                    self.liquid_destination(source, tick, row_routes.as_ref())
                                }
                                _ => None,
                            };
                            if let Some(destination) = destination {
                                moves.push((
                                    source,
                                    destination,
                                    cells[source],
                                    cells[destination],
                                ));
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
        proposals.sort_unstable_by_key(|&(source, destination, _, _)| {
            (priority(source, tick), source, destination)
        });
        self.awake.fill(false);
        // Density swaps can share source/destination sets. Claim both endpoints
        // at deterministic commit, so a cell participates in at most one move
        // and neither matter identity nor volume is duplicated/lost.
        let stamp = self.tick + 1;
        for (source, destination, expected_source, expected_target) in proposals {
            if self.claimed[source] != stamp
                && self.claimed[destination] != stamp
                && self.cells[source] == expected_source
                && self.cells[destination] == expected_target
            {
                if source / self.width != destination / self.width {
                    let source_liquid = usize::from(self.is_liquid(expected_source));
                    let target_liquid = usize::from(self.is_liquid(expected_target));
                    self.liquid_rows[source / self.width] =
                        self.liquid_rows[source / self.width] - source_liquid + target_liquid;
                    self.liquid_rows[destination / self.width] =
                        self.liquid_rows[destination / self.width] - target_liquid + source_liquid;
                }
                self.cells.swap(source, destination);
                self.claimed[source] = stamp;
                self.claimed[destination] = stamp;
                self.wake(source);
                self.wake(destination);
            }
        }
        self.tick += 1;
        (active, visited, workers)
    }
    /// Completed physics steps; editing and inspection never advance this clock.
    pub fn tick(&self) -> u64 {
        self.tick
    }
    /// Whether the cell's tile is scheduled for the next physics step.
    pub fn awake_at(&self, x: i32, y: i32) -> Option<bool> {
        self.material_at(x, y)?;
        Some(self.awake[y as usize / TILE * self.width.div_ceil(TILE) + x as usize / TILE])
    }
    /// Owner-thread edits without generating an intermediate image. The next
    /// step/edit publishes all dirty tiles; this does not advance the tick.
    pub fn apply_edits(&mut self, commands: Vec<Command>) {
        for command in commands {
            self.command(command);
        }
    }
    /// Apply explicit editor operations without advancing falling material.
    pub fn edit(&mut self, commands: Vec<Command>) -> Frame {
        let start = Instant::now();
        for command in commands {
            self.command(command);
        }
        self.snapshot(0, 0, 0, start.elapsed().as_secs_f64() * 1000.0)
    }
    fn snapshot(
        &mut self,
        active_tiles: usize,
        visited_cells: usize,
        workers_used: u32,
        update_ms: f64,
    ) -> Frame {
        let snapshot_start = Instant::now();
        let mut converted_cells = 0;
        for tile in 0..self.dirty.len() {
            if !self.dirty[tile] {
                continue;
            }
            let rect = self.image.rect(tile);
            let mut rgba = Vec::with_capacity(rect.width * rect.height * 4);
            let mut indices = Vec::with_capacity(rect.width * rect.height);
            for y in rect.y..rect.y + rect.height {
                for x in rect.x..rect.x + rect.width {
                    indices.push(self.cells[y * self.width + x]);
                    rgba.extend_from_slice(
                        &self
                            .materials
                            .get(self.cells[y * self.width + x])
                            .expect("validated cell")
                            .color,
                    );
                }
            }
            converted_cells += rect.width * rect.height;
            self.image.replace(tile, rgba);
            self.indices.replace(tile, indices);
            self.dirty[tile] = false;
        }
        Frame {
            width: self.width,
            height: self.height,
            image: self.image.clone(),
            indices: self.indices.clone(),
            materials: Arc::clone(&self.materials),
            converted_cells,
            sand: self.sand,
            liquid: self.liquid,
            active_tiles,
            visited_cells,
            workers_configured: self.pool.current_num_threads(),
            workers_used,
            update_ms,
            snapshot_ms: snapshot_start.elapsed().as_secs_f64() * 1000.0,
        }
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
        let (active, visited, workers) = self.advance();
        self.snapshot(
            active,
            visited,
            workers,
            start.elapsed().as_secs_f64() * 1000.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_image_matches_cells(world: &SandWorld, frame: &Frame) {
        let expected: Vec<_> = world
            .cells
            .iter()
            .flat_map(|&cell| world.materials.get(cell).unwrap().color)
            .collect();
        assert_eq!(frame.image.to_rgba(), expected);
        assert_eq!(frame.indices.to_bytes(), world.cells);
        assert_eq!(
            frame.sand,
            world.cells.iter().filter(|&&cell| cell == SAND).count()
        );
    }

    fn liquid_materials() -> Materials {
        let base = Materials::default();
        let mut entries: Vec<_> = (0..3).map(|id| base.get(id).unwrap().clone()).collect();
        for key in ["water", "other-liquid"] {
            entries.push(Material {
                key: key.into(),
                color: [25, 135, 235, 255],
                motion: Motion::Liquid,
                density: 1000,
                blocks_actors: false,
                blocks_projectiles: false,
            });
        }
        Materials::new(entries, AIR, WALL).unwrap()
    }
    #[test]
    fn liquids_sleep_without_jitter_and_wake_across_search_and_tile_boundaries() {
        for workers in [1, 2, 4] {
            let mut world = SandWorld::with_materials(65, 66, workers, liquid_materials()).unwrap();
            world.edit(vec![
                Command::FillRect {
                    x: 1,
                    y: 31,
                    width: 63,
                    height: 1,
                    material: WALL,
                },
                Command::FillRect {
                    x: 10,
                    y: 30,
                    width: 1,
                    height: 1,
                    material: 3,
                },
            ]);
            world.step(vec![]);
            let checksum = world.checksum();
            for _ in 0..5 {
                let frame = world.step(vec![]);
                assert_eq!(world.checksum(), checksum, "resting liquid jittered");
                assert_eq!(
                    (
                        frame.active_tiles,
                        frame.visited_cells,
                        frame.converted_cells,
                        frame.liquid
                    ),
                    (0, 0, 0, 1)
                );
            }
            // The only particle is 22 columns from a newly opened hole in
            // another tile. A one-cell wake halo would leave it asleep forever.
            let frame = world.step(vec![Command::FillRect {
                x: 32,
                y: 31,
                width: 1,
                height: 1,
                material: AIR,
            }]);
            assert_eq!(world.material_at(32, 31), Some(3));
            assert_eq!(world.material_at(10, 30), Some(AIR));
            assert_eq!(frame.liquid, 1);
        }
    }
    #[test]
    fn mixed_liquids_conserve_each_material_and_match_full_scan_after_basin_break() {
        for workers in [1, 2, 4] {
            let mut world = SandWorld::with_materials(65, 66, workers, liquid_materials()).unwrap();
            world.edit(vec![
                Command::FillRect {
                    x: 1,
                    y: 32,
                    width: 63,
                    height: 2,
                    material: WALL,
                },
                Command::FillRect {
                    x: 1,
                    y: 24,
                    width: 30,
                    height: 8,
                    material: 3,
                },
                Command::FillRect {
                    x: 31,
                    y: 24,
                    width: 33,
                    height: 8,
                    material: 4,
                },
            ]);
            let mut reference =
                SandWorld::from_material_cells(65, 66, 1, world.cells.clone(), liquid_materials())
                    .unwrap();
            reference.full_scan = true;
            let original = [3, 4].map(|id| world.cells.iter().filter(|&&c| c == id).count());
            for tick in 0..1000 {
                let commands = if tick == 5 {
                    vec![Command::FillRect {
                        x: 30,
                        y: 32,
                        width: 5,
                        height: 2,
                        material: AIR,
                    }]
                } else {
                    vec![]
                };
                let frame = world.step(commands.clone());
                reference.step(commands);
                assert_eq!(
                    world.cells, reference.cells,
                    "workers={workers} tick={tick}"
                );
                assert_eq!(
                    [3, 4].map(|id| world.cells.iter().filter(|&&c| c == id).count()),
                    original
                );
                assert_eq!(frame.liquid, original.iter().sum());
                assert_image_matches_cells_custom(&world, &frame);
                if tick > 5 && frame.active_tiles == 0 {
                    break;
                }
            }
            assert!(
                world.cells[34 * 65..65 * 65]
                    .iter()
                    .any(|&id| id == 3 || id == 4),
                "basin break did not release water"
            );
            let idle = world.step(vec![]);
            assert_eq!(
                (idle.active_tiles, idle.visited_cells, idle.converted_cells),
                (0, 0, 0)
            );
        }
    }
    #[test]
    fn wide_water_surface_levels_to_one_cell_and_then_sleeps() {
        let mut world = SandWorld::with_materials(130, 66, 4, liquid_materials()).unwrap();
        world.edit(vec![Command::FillRect {
            x: 12,
            y: 3,
            width: 8,
            height: 30,
            material: 3,
        }]);
        for _ in 0..350 {
            world.step(vec![]);
        }
        let heights: Vec<_> = (1..129)
            .map(|x| {
                (0..65)
                    .filter(|&y| world.material_at(x, y) == Some(3))
                    .count()
            })
            .collect();
        assert!(
            heights.iter().max().unwrap() - heights.iter().min().unwrap() <= 1,
            "resting surface retained an artificial mound"
        );
        let checksum = world.checksum();
        for _ in 0..5 {
            let frame = world.step(vec![]);
            assert_eq!(
                (
                    frame.active_tiles,
                    frame.visited_cells,
                    frame.converted_cells
                ),
                (0, 0, 0)
            );
            assert_eq!(world.checksum(), checksum);
        }
    }

    #[test]
    fn adjacent_immiscible_columns_form_horizontal_density_layers() {
        let base = liquid_materials();
        let mut entries: Vec<_> = (0..5).map(|id| base.get(id).unwrap().clone()).collect();
        entries[4].density = 800;
        let materials = Materials::new(entries, AIR, WALL).unwrap();
        let mut world = SandWorld::with_materials(33, 34, 4, materials.clone()).unwrap();
        world.edit(vec![
            Command::FillRect {
                x: 1,
                y: 13,
                width: 15,
                height: 20,
                material: 3,
            },
            Command::FillRect {
                x: 16,
                y: 13,
                width: 16,
                height: 20,
                material: 4,
            },
        ]);
        let mut reference =
            SandWorld::from_material_cells(33, 34, 1, world.cells.clone(), materials).unwrap();
        reference.full_scan = true;
        for _ in 0..500 {
            world.step(vec![]);
            reference.step(vec![]);
            assert_eq!(world.cells, reference.cells);
        }
        let rows = |id| {
            world
                .cells
                .iter()
                .enumerate()
                .filter_map(|(i, &c)| (c == id).then_some(i / 33))
                .collect::<Vec<_>>()
        };
        assert!(
            rows(4).iter().max().unwrap() <= &(rows(3).iter().min().unwrap() + 1),
            "oil remained trapped in a side column"
        );
        assert_eq!(
            (world.material_count(3), world.material_count(4)),
            (300, 320)
        );
        assert_eq!(world.step(vec![]).active_tiles, 0);
    }

    #[test]
    fn dense_powder_displaces_liquid_and_lighter_liquid_rises_without_mass_loss() {
        let mut materials = liquid_materials();
        // Give the second liquid a lighter game-defined density.
        let mut entries: Vec<_> = (0..5)
            .map(|id| materials.get(id).unwrap().clone())
            .collect();
        entries[4].density = 800;
        materials = Materials::new(entries, AIR, WALL).unwrap();
        let mut world = SandWorld::with_materials(33, 34, 4, materials.clone()).unwrap();
        world.edit(vec![
            Command::FillRect {
                x: 1,
                y: 12,
                width: 31,
                height: 21,
                material: 3,
            },
            Command::FillRect {
                x: 5,
                y: 2,
                width: 10,
                height: 6,
                material: SAND,
            },
            Command::FillRect {
                x: 20,
                y: 28,
                width: 5,
                height: 5,
                material: 4,
            },
        ]);
        let mut reference =
            SandWorld::from_material_cells(33, 34, 1, world.cells.clone(), materials).unwrap();
        reference.full_scan = true;
        let counts =
            |w: &SandWorld| [SAND, 3, 4].map(|id| w.cells.iter().filter(|&&c| c == id).count());
        let initial = counts(&world);
        for tick in 0..450 {
            let frame = world.step(vec![]);
            reference.step(vec![]);
            assert_eq!(world.cells, reference.cells, "density tick={tick}");
            assert_eq!(counts(&world), initial);
            assert_eq!(
                (frame.sand, frame.liquid),
                (initial[0], initial[1] + initial[2])
            );
        }
        assert!(
            world.cells[32 * 33..33 * 33].contains(&SAND),
            "sand did not sink"
        );
        let mean_row = |id| {
            let rows: Vec<_> = world
                .cells
                .iter()
                .enumerate()
                .filter_map(|(i, &c)| (c == id).then_some(i / 33))
                .collect();
            rows.iter().sum::<usize>() as f32 / rows.len() as f32
        };
        assert!(mean_row(4) < mean_row(3), "oil did not rise above water");
    }

    #[test]
    fn falling_water_spreads_without_crossing_walls_or_diagonal_closed_corners() {
        let mut world = SandWorld::with_materials(65, 66, 4, liquid_materials()).unwrap();
        world.edit(vec![
            Command::FillRect {
                x: 32,
                y: 0,
                width: 1,
                height: 65,
                material: WALL,
            },
            Command::FillRect {
                x: 14,
                y: 8,
                width: 3,
                height: 15,
                material: 3,
            },
        ]);
        let original = world.liquid;
        for _ in 0..180 {
            world.step(vec![]);
        }
        assert_eq!(world.liquid, original);
        let positions: Vec<_> = world
            .cells
            .iter()
            .enumerate()
            .filter_map(|(i, &c)| (c == 3).then_some(i % 65))
            .collect();
        assert!(
            positions.iter().max().unwrap() - positions.iter().min().unwrap() > 3,
            "water did not spread"
        );
        assert!(
            positions.iter().all(|&x| x < 32),
            "water crossed a sealed wall"
        );
        let mut corner = SandWorld::with_materials(8, 8, 1, liquid_materials()).unwrap();
        corner.edit(vec![
            Command::FillRect {
                x: 2,
                y: 2,
                width: 1,
                height: 1,
                material: 3,
            },
            Command::FillRect {
                x: 1,
                y: 2,
                width: 1,
                height: 1,
                material: WALL,
            },
            Command::FillRect {
                x: 3,
                y: 2,
                width: 1,
                height: 1,
                material: WALL,
            },
            Command::FillRect {
                x: 2,
                y: 3,
                width: 1,
                height: 1,
                material: WALL,
            },
        ]);
        for _ in 0..10 {
            corner.step(vec![]);
        }
        assert_eq!(
            corner.material_at(2, 2),
            Some(3),
            "water leaked through a closed diagonal"
        );
    }

    #[test]
    fn rectangular_edits_fill_exact_blocks_and_preserve_protected_edges() {
        let mut world = SandWorld::new(65, 66, 2).unwrap();
        world.edit(vec![Command::FillRect {
            x: 16,
            y: 32,
            width: 8,
            height: 8,
            material: WALL,
        }]);
        for y in 1..65 {
            for x in 1..64 {
                assert_eq!(
                    world.material_at(x, y),
                    Some(if (16..24).contains(&x) && (32..40).contains(&y) {
                        WALL
                    } else {
                        AIR
                    })
                );
            }
        }
        let frame = world.edit(vec![Command::FillRect {
            x: 0,
            y: 0,
            width: usize::MAX,
            height: usize::MAX,
            material: AIR,
        }]);
        assert_eq!(world.material_at(0, 20), Some(WALL));
        assert_eq!(world.material_at(20, 65), Some(WALL));
        assert_image_matches_cells(&world, &frame);
    }

    #[test]
    fn custom_powders_preserve_identity_sleep_and_wake_independently_of_worker_count() {
        // Deliberately non-default IDs prove the solver does not depend on AIR/SAND/WALL.
        let materials = Materials::new(
            vec![
                Material {
                    key: "barrier".into(),
                    color: [1, 2, 3, 255],
                    motion: Motion::Static,
                    density: u16::MAX,
                    blocks_actors: true,
                    blocks_projectiles: true,
                },
                Material {
                    key: "vacuum".into(),
                    color: [0, 0, 0, 255],
                    motion: Motion::Empty,
                    density: 0,
                    blocks_actors: false,
                    blocks_projectiles: false,
                },
                Material {
                    key: "red".into(),
                    color: [255, 0, 0, 255],
                    motion: Motion::Powder,
                    density: 1600,
                    blocks_actors: false,
                    blocks_projectiles: true,
                },
                Material {
                    key: "blue".into(),
                    color: [0, 0, 255, 255],
                    motion: Motion::Powder,
                    density: 1600,
                    blocks_actors: true,
                    blocks_projectiles: false,
                },
            ],
            1,
            0,
        )
        .unwrap();
        let mut parallel = SandWorld::with_materials(65, 66, 4, materials.clone()).unwrap();
        for y in 20..24 {
            parallel.cells[y * 65 + 4..y * 65 + 31].fill(2);
            parallel.cells[y * 65 + 33..y * 65 + 61].fill(3);
        }
        let mut reference =
            SandWorld::from_material_cells(65, 66, 1, parallel.cells.clone(), materials).unwrap();
        parallel.sand = reference.sand;
        parallel.counts = reference.counts;
        reference.full_scan = true;
        let counts = |w: &SandWorld| [2, 3].map(|id| w.cells.iter().filter(|&&c| c == id).count());
        let original = counts(&parallel);
        for _ in 0..140 {
            parallel.step(vec![]);
            reference.step(vec![]);
            assert_eq!(parallel.cells, reference.cells);
            assert_eq!(counts(&parallel), original);
        }
        let idle = parallel.step(vec![]);
        reference.step(vec![]);
        assert_eq!((idle.active_tiles, idle.converted_cells), (0, 0));
        assert_eq!(idle.sand, original.iter().sum());
        assert_image_matches_cells_custom(&parallel, &idle);
        // Remove a settled grain at the tile boundary; all affected neighbors wake.
        let edit = Command::Paint {
            x: 32,
            y: 61,
            material: 1,
        };
        for tick in 0..40 {
            let commands = if tick == 0 { vec![edit] } else { vec![] };
            let frame = parallel.step(commands.clone());
            reference.step(commands);
            assert_eq!(parallel.cells, reference.cells);
            assert_image_matches_cells_custom(&parallel, &frame);
        }
    }

    fn assert_image_matches_cells_custom(world: &SandWorld, frame: &Frame) {
        let pixels: Vec<_> = world
            .cells
            .iter()
            .flat_map(|&id| world.materials.get(id).unwrap().color)
            .collect();
        assert_eq!(frame.image.to_rgba(), pixels);
        assert_eq!(frame.indices.to_bytes(), world.cells);
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
