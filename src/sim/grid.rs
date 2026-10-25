//! Double-buffered 2D grid with per-chunk dirty-rect tracking.
//!
//! Chunked simulation is the technique Petri Purho describes in his
//! ["Exploring the tech and design of Noita"](https://www.youtube.com/watch?v=prXuyMCgbTc)
//! GDC 2019 talk: the world is partitioned into fixed-size chunks; a chunk is
//! only re-simulated while it (or one of its neighbours) contains active
//! particles. The same idea is used in Sandspiel and Powder Toy. It is what
//! keeps a 1280x720 grid (~920k cells) interactive on a single thread.

use super::material::{Cell, CellData};
use super::rules::MaterialTable;

/// Size of a simulation chunk, in cells. 32 is a good trade-off between
/// dirty-rect granularity and per-chunk overhead (Noita uses 64).
pub const CHUNK: usize = 32;

#[inline]
fn div_up(a: usize, b: usize) -> usize {
    a.div_ceil(b)
}

#[derive(Clone)]
pub struct Grid {
    pub materials: MaterialTable,
    surface: Vec<usize>,
    surface_dirty: Vec<bool>,
    pub w: usize,
    pub h: usize,
    /// Current frame's state (read by rendering and physics).
    pub cells: Vec<CellData>,
    /// Working/spare allocation used during a sweep. This is scratch storage,
    /// not a snapshot of the previous frame.
    pub next: Vec<CellData>,
    /// Per-cell flag: set by `physics::step` when a cell has been written this
    /// sweep so it isn't moved twice in the same tick.
    pub moved: Vec<bool>,

    /// Dirty flag per chunk for the *current* tick. Set when a cell inside a
    /// chunk was touched or modified externally (e.g. brush, scenario reset).
    pub dirty: Vec<bool>,
    /// Dirty flag carried over to the *next* tick. Materials that move write
    /// their destination chunk here so the chunk simulates again next frame.
    pub dirty_next: Vec<bool>,
    pub chunks_x: usize,
    pub chunks_y: usize,
}

impl Grid {
    pub fn new(w: usize, h: usize) -> Self {
        assert!(w > 0 && h > 0, "grid dimensions must be positive");
        let n = w * h;
        let chunks_x = div_up(w, CHUNK);
        let chunks_y = div_up(h, CHUNK);
        Self {
            materials: MaterialTable::default(),
            surface: vec![h; w],
            surface_dirty: vec![true; w],
            w,
            h,
            cells: vec![CellData::AIR; n],
            next: vec![CellData::AIR; n],
            moved: vec![false; n],
            dirty: vec![true; chunks_x * chunks_y],
            dirty_next: vec![false; chunks_x * chunks_y],
            chunks_x,
            chunks_y,
        }
    }

    #[inline]
    pub fn idx(&self, x: usize, y: usize) -> usize {
        y * self.w + x
    }

    #[inline]
    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && (x as usize) < self.w && y >= 0 && (y as usize) < self.h
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> CellData {
        debug_assert!(x < self.w && y < self.h);
        self.cells[self.idx(x, y)]
    }

    /// Bounds-checked write. Out-of-bounds calls are silently dropped so
    /// scenario painters can use coarse arithmetic without splattering
    /// `if in_bounds` everywhere.
    pub fn set(&mut self, x: usize, y: usize, cell: CellData) {
        if x >= self.w || y >= self.h {
            return;
        }
        assert!(
            self.materials.contains(cell.kind),
            "material ID is not registered in this grid"
        );
        let i = self.idx(x, y);
        self.cells[i] = cell;
        self.mark_dirty(x, y);
    }

    /// Mark the chunk containing (x,y) and its 8 neighbours dirty for the next
    /// tick. Neighbour propagation prevents particles from getting "stuck" at
    /// chunk boundaries.
    #[inline]
    pub fn mark_dirty(&mut self, x: usize, y: usize) {
        self.surface_dirty[x] = true;
        let cx = x / CHUNK;
        let cy = y / CHUNK;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let nx = cx as i32 + dx;
                let ny = cy as i32 + dy;
                if nx < 0 || ny < 0 {
                    continue;
                }
                let (nx, ny) = (nx as usize, ny as usize);
                if nx >= self.chunks_x || ny >= self.chunks_y {
                    continue;
                }
                self.dirty_next[ny * self.chunks_x + nx] = true;
            }
        }
    }

    #[inline]
    pub fn chunk_dirty(&self, cx: usize, cy: usize) -> bool {
        self.dirty[cy * self.chunks_x + cx]
    }

    /// Move `dirty_next` into `dirty` at the start of a frame.
    pub fn rotate_dirty(&mut self) {
        std::mem::swap(&mut self.dirty, &mut self.dirty_next);
        self.dirty_next.fill(false);
    }

    /// Swap the working buffer into `cells` and reset the per-tick moved flags.
    pub fn swap_buffers(&mut self) {
        std::mem::swap(&mut self.cells, &mut self.next);
        self.moved.fill(false);
    }

    pub fn clear(&mut self) {
        self.surface_dirty.fill(true);
        self.cells.fill(CellData::AIR);
        self.next.fill(CellData::AIR);
        self.moved.fill(false);
        self.dirty.fill(true);
        self.dirty_next.fill(true);
    }

    /// Allocated vector payloads, excluding struct headers and allocator overhead.
    pub fn buffer_bytes(&self) -> usize {
        self.materials.buffer_bytes()
            + self.surface.capacity() * std::mem::size_of::<usize>()
            + self.surface_dirty.capacity() * std::mem::size_of::<bool>()
            + (self.cells.capacity() + self.next.capacity()) * std::mem::size_of::<CellData>()
            + (self.moved.capacity() + self.dirty.capacity() + self.dirty_next.capacity())
                * std::mem::size_of::<bool>()
    }

    pub(super) fn surface_y(&mut self, x: usize) -> usize {
        if self.surface_dirty[x] {
            self.surface[x] = (0..self.h)
                .find(|&y| {
                    let kind = self.next[y * self.w + x].kind;
                    kind != Cell::Air
                        && matches!(
                            self.materials.get(kind).movement,
                            super::rules::Movement::Solid | super::rules::Movement::Granular
                        )
                })
                .unwrap_or(self.h);
            self.surface_dirty[x] = false;
        }
        self.surface[x]
    }

    pub fn set_materials(&mut self, materials: MaterialTable) {
        assert!(
            self.cells.iter().all(|cell| materials.contains(cell.kind)),
            "replacement registry is missing active material IDs"
        );
        self.materials = materials;
        self.dirty_next.fill(true);
        self.surface_dirty.fill(true);
    }

    pub fn particle_count(&self) -> usize {
        self.cells
            .iter()
            .filter(|c| c.kind != Cell::Air && c.kind != Cell::Block)
            .count()
    }

    pub fn dirty_chunk_count(&self) -> usize {
        self.dirty.iter().filter(|d| **d).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_grid_is_air() {
        let g = Grid::new(64, 64);
        assert_eq!(g.particle_count(), 0);
    }

    #[test]
    fn set_marks_chunk_dirty() {
        let mut g = Grid::new(64, 64);
        g.dirty_next.fill(false);
        g.set(40, 40, CellData::sand(0));
        g.rotate_dirty();
        assert!(g.chunk_dirty(40 / CHUNK, 40 / CHUNK));
    }

    #[test]
    fn in_bounds_handles_negatives() {
        let g = Grid::new(16, 16);
        assert!(!g.in_bounds(-1, 0));
        assert!(!g.in_bounds(0, -1));
        assert!(!g.in_bounds(16, 0));
        assert!(g.in_bounds(15, 15));
    }
}
