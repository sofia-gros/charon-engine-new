//! Standalone command/snapshot interface. This module has no Core, window, or
//! rendering dependencies. External solids are a separate collision layer.
use super::{
    grid::Grid,
    material::{Cell, CellData},
    physics,
};
use rand::{SeedableRng, rngs::StdRng};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Fill empty cells only. Air erases simulation cells explicitly; external
    /// colliders are never painted over. Radius is measured in simulation cells.
    Brush {
        x: i32,
        y: i32,
        radius: u16,
        cell: CellData,
    },
    /// Adding a solid rejects the entire clipped rectangle if any cell is occupied.
    Collider {
        rect: Rect,
        solid: bool,
    },
    /// Release existing external solids as material, once per solid cell.
    Release {
        rect: Rect,
        cell: CellData,
    },
    SetPaused(bool),
    Reset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// Strictly increasing per Simulation, including failed requests.
    pub id: u64,
    pub expected_revision: Option<u64>,
    pub operation: Operation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    StaleRevision,
    DuplicateOrOutOfOrder,
    Occupied,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub id: u64,
    pub revision: u64,
    pub outcome: Outcome,
    pub changed: usize,
    /// Brush cells skipped because a collider or particle already occupied them.
    pub blocked: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contact {
    pub collider: (usize, usize),
    pub particle: (usize, usize),
    pub material: Cell,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<CellData>,
    /// Row-major positions in the independent collision layer.
    pub colliders: Vec<usize>,
    pub ticks: u64,
    pub revision: u64,
    pub active_chunks: usize,
    pub paused: bool,
}

pub struct Simulation {
    grid: Grid,
    solids: BTreeSet<usize>,
    rng: StdRng,
    seed: u64,
    ticks: u64,
    revision: u64,
    last_request: Option<u64>,
    paused: bool,
}

impl Simulation {
    pub fn new(width: usize, height: usize, seed: u64) -> Self {
        Self {
            grid: Grid::new(width, height),
            solids: BTreeSet::new(),
            rng: StdRng::seed_from_u64(seed),
            seed,
            ticks: 0,
            revision: 0,
            last_request: None,
            paused: false,
        }
    }

    /// Read-only access keeps external solid / particle occupancy consistent.
    pub fn grid(&self) -> &Grid {
        &self.grid
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn ticks(&self) -> u64 {
        self.ticks
    }
    pub fn is_solid(&self, x: usize, y: usize) -> bool {
        x < self.grid.w && y < self.grid.h && self.solids.contains(&self.grid.idx(x, y))
    }

    fn bounds(&self, rect: Rect) -> (usize, usize, usize, usize) {
        let x0 = i64::from(rect.x).clamp(0, self.grid.w as i64) as usize;
        let y0 = i64::from(rect.y).clamp(0, self.grid.h as i64) as usize;
        let x1 = (i64::from(rect.x) + i64::from(rect.w)).clamp(0, self.grid.w as i64) as usize;
        let y1 = (i64::from(rect.y) + i64::from(rect.h)).clamp(0, self.grid.h as i64) as usize;
        (x0, y0, x1, y1)
    }

    pub fn apply(&mut self, request: Request) -> Receipt {
        let mut receipt = Receipt {
            id: request.id,
            revision: self.revision,
            outcome: Outcome::Applied,
            changed: 0,
            blocked: 0,
        };
        if self.last_request.is_some_and(|last| request.id <= last) {
            receipt.outcome = Outcome::DuplicateOrOutOfOrder;
            return receipt;
        }
        self.last_request = Some(request.id);
        if request
            .expected_revision
            .is_some_and(|revision| revision != self.revision)
        {
            receipt.outcome = Outcome::StaleRevision;
            return receipt;
        }
        match request.operation {
            Operation::Brush { x, y, radius, cell } => {
                let r = i64::from(radius);
                let x0 = (i64::from(x) - r).clamp(0, self.grid.w as i64) as usize;
                let y0 = (i64::from(y) - r).clamp(0, self.grid.h as i64) as usize;
                let x1 = (i64::from(x) + r + 1).clamp(0, self.grid.w as i64) as usize;
                let y1 = (i64::from(y) + r + 1).clamp(0, self.grid.h as i64) as usize;
                for py in y0..y1 {
                    for px in x0..x1 {
                        let dx = px as i64 - i64::from(x);
                        let dy = py as i64 - i64::from(y);
                        if dx * dx + dy * dy > r * r {
                            continue;
                        }
                        let i = self.grid.idx(px, py);
                        if self.solids.contains(&i)
                            || (cell.kind != Cell::Air && self.grid.cells[i].kind != Cell::Air)
                        {
                            receipt.blocked += 1;
                        } else if self.grid.cells[i] != cell {
                            self.grid.set(px, py, cell);
                            receipt.changed += 1;
                        }
                    }
                }
            }
            Operation::Collider { rect, solid } => {
                let (x0, y0, x1, y1) = self.bounds(rect);
                if solid {
                    for y in y0..y1 {
                        for x in x0..x1 {
                            if self.grid.get(x, y).kind != Cell::Air {
                                receipt.blocked += 1;
                            }
                        }
                    }
                    if receipt.blocked != 0 {
                        receipt.outcome = Outcome::Occupied;
                        return receipt;
                    }
                }
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = self.grid.idx(x, y);
                        let changed = if solid {
                            self.solids.insert(i)
                        } else {
                            self.solids.remove(&i)
                        };
                        if changed {
                            self.grid.mark_dirty(x, y);
                            receipt.changed += 1;
                        }
                    }
                }
            }
            Operation::Release { rect, cell } => {
                let (x0, y0, x1, y1) = self.bounds(rect);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = self.grid.idx(x, y);
                        if self.solids.remove(&i) {
                            self.grid.set(x, y, cell);
                            receipt.changed += 1;
                        }
                    }
                }
            }
            Operation::SetPaused(paused) => {
                receipt.changed = usize::from(self.paused != paused);
                self.paused = paused;
            }
            Operation::Reset => {
                self.grid.clear();
                self.solids.clear();
                self.rng = StdRng::seed_from_u64(self.seed);
                self.ticks = 0;
                self.paused = false;
                receipt.changed = 1;
            }
        }
        if receipt.changed != 0 {
            self.revision = self.revision.saturating_add(1);
        }
        receipt.revision = self.revision;
        receipt
    }

    pub fn step(&mut self) {
        if self.paused {
            return;
        }
        // Physics treats Block as immovable and non-reactive. Overlay only empty
        // external solid positions, then strip them from the particle layer.
        // No extra collision check is needed in each material's movement rule.
        for &i in &self.solids {
            self.grid.cells[i] = CellData::block();
        }
        physics::step(&mut self.grid, &mut self.rng);
        for &i in &self.solids {
            self.grid.cells[i] = CellData::AIR;
            self.grid.next[i] = CellData::AIR;
        }
        self.ticks = self.ticks.saturating_add(1);
        self.revision = self.revision.saturating_add(1);
    }

    /// Four-neighbor contacts, queried on demand (no unbounded per-tick event list).
    pub fn contacts(&self) -> impl Iterator<Item = Contact> + '_ {
        self.solids.iter().flat_map(move |&i| {
            let (x, y) = (i % self.grid.w, i / self.grid.w);
            [(-1, 0), (1, 0), (0, -1), (0, 1)]
                .into_iter()
                .filter_map(move |(dx, dy)| {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= self.grid.w as i64 || ny >= self.grid.h as i64 {
                        return None;
                    }
                    let material = self.grid.get(nx as usize, ny as usize).kind;
                    (material != Cell::Air).then_some(Contact {
                        collider: (x, y),
                        particle: (nx as usize, ny as usize),
                        material,
                    })
                })
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            width: self.grid.w,
            height: self.grid.h,
            cells: self.grid.cells.clone(),
            colliders: self.solids.iter().copied().collect(),
            ticks: self.ticks,
            revision: self.revision,
            active_chunks: self.grid.dirty_chunk_count(),
            paused: self.paused,
        }
    }
}
