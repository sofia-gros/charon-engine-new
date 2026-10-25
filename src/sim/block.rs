//! An interactive rigid axis-aligned block that the user can drag through the
//! simulation.
//!
//! Moving the block has to displace any granular/fluid particles that occupy
//! the new bounding box. We use a small breadth-first search to push those
//! particles into the nearest empty cell, capped so the operation stays O(1)
//! amortised. If no path of empty cells is found within `MAX_SEARCH` we
//! refuse the move — this keeps the user from clipping the block into stone
//! or compressed sand.

use super::grid::Grid;
use super::material::{Cell, CellData};
use glam::Vec2;
use std::collections::{HashSet, VecDeque};

const MAX_SEARCH: usize = 400;

#[derive(Clone)]
pub struct Block {
    pub pos: Vec2,
    pub half_size: Vec2,
    pub grabbed: bool,
    pub grab_offset: Vec2,
}

impl Block {
    pub fn new(cx: f32, cy: f32, hw: f32, hh: f32) -> Self {
        Self {
            pos: Vec2::new(cx, cy),
            half_size: Vec2::new(hw, hh),
            grabbed: false,
            grab_offset: Vec2::ZERO,
        }
    }

    pub fn min(&self) -> (i32, i32) {
        (
            (self.pos.x - self.half_size.x).floor() as i32,
            (self.pos.y - self.half_size.y).floor() as i32,
        )
    }

    pub fn max(&self) -> (i32, i32) {
        (
            (self.pos.x + self.half_size.x).ceil() as i32,
            (self.pos.y + self.half_size.y).ceil() as i32,
        )
    }

    fn covered_cells(&self, w: usize, h: usize) -> Vec<(usize, usize)> {
        let (minx, miny) = self.min();
        let (maxx, maxy) = self.max();
        let mut out = Vec::new();
        for y in miny.max(0)..maxy.min(h as i32) {
            for x in minx.max(0)..maxx.min(w as i32) {
                out.push((x as usize, y as usize));
            }
        }
        out
    }

    pub fn contains(&self, gx: f32, gy: f32) -> bool {
        let dx = (gx - self.pos.x).abs();
        let dy = (gy - self.pos.y).abs();
        dx <= self.half_size.x && dy <= self.half_size.y
    }

    pub fn rasterize(&self, grid: &mut Grid) {
        for (x, y) in self.covered_cells(grid.w, grid.h) {
            let i = grid.idx(x, y);
            grid.cells[i] = CellData::block();
            grid.mark_dirty(x, y);
        }
    }

    pub fn clear_raster(&self, grid: &mut Grid) {
        for (x, y) in self.covered_cells(grid.w, grid.h) {
            let i = grid.idx(x, y);
            if grid.cells[i].kind == Cell::Block {
                grid.cells[i] = CellData::AIR;
                grid.mark_dirty(x, y);
            }
        }
    }

    /// Move the block to `new_pos`. Any particles in the way are displaced by
    /// BFS into the nearest empty cell; if displacement fails the move is
    /// reverted so the block can't squash through walls.
    pub fn move_and_displace(&mut self, new_pos: Vec2, grid: &mut Grid) {
        let old_pos = self.pos;
        let old_cells = self.covered_cells(grid.w, grid.h);
        self.pos = new_pos;
        let new_cells = self.covered_cells(grid.w, grid.h);

        for &(x, y) in &old_cells {
            let i = grid.idx(x, y);
            if grid.cells[i].kind == Cell::Block {
                grid.cells[i] = CellData::AIR;
                grid.mark_dirty(x, y);
            }
        }

        let blocked: HashSet<(usize, usize)> = new_cells.iter().copied().collect();
        let mut failed = false;
        for &(x, y) in &new_cells {
            let i = grid.idx(x, y);
            let k = grid.cells[i].kind;
            if k == Cell::Stone {
                failed = true;
                break;
            }
            if k != Cell::Air && k != Cell::Block {
                let particle = grid.cells[i];
                grid.cells[i] = CellData::AIR;
                if !bfs_push(grid, x, y, &blocked, particle) {
                    grid.cells[i] = particle;
                    failed = true;
                    break;
                }
            }
        }

        if failed {
            self.pos = old_pos;
            for &(x, y) in &self.covered_cells(grid.w, grid.h) {
                let i = grid.idx(x, y);
                if grid.cells[i].kind == Cell::Block {
                    grid.cells[i] = CellData::AIR;
                    grid.mark_dirty(x, y);
                }
            }
        }

        self.rasterize(grid);
    }
}

fn bfs_push(
    grid: &mut Grid,
    sx: usize,
    sy: usize,
    blocked: &HashSet<(usize, usize)>,
    particle: CellData,
) -> bool {
    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();
    queue.push_back((sx, sy));
    visited.insert((sx, sy));

    let dirs: [(i32, i32); 4] = [(0, -1), (-1, 0), (1, 0), (0, 1)];
    let mut steps = 0;

    while let Some((cx, cy)) = queue.pop_front() {
        steps += 1;
        if steps > MAX_SEARCH {
            return false;
        }
        for (dx, dy) in dirs {
            let nx = cx as i32 + dx;
            let ny = cy as i32 + dy;
            if !grid.in_bounds(nx, ny) {
                continue;
            }
            let (ux, uy) = (nx as usize, ny as usize);
            if visited.contains(&(ux, uy)) || blocked.contains(&(ux, uy)) {
                continue;
            }
            visited.insert((ux, uy));
            let ni = grid.idx(ux, uy);
            let nk = grid.cells[ni].kind;
            if nk == Cell::Air {
                grid.cells[ni] = particle;
                grid.mark_dirty(ux, uy);
                return true;
            }
            if nk != Cell::Stone && nk != Cell::Block {
                queue.push_back((ux, uy));
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_rasterizes_and_clears() {
        let mut g = Grid::new(32, 32);
        let block = Block::new(16.0, 16.0, 4.0, 2.0);
        block.rasterize(&mut g);
        assert_eq!(g.get(16, 16).kind, Cell::Block);
        block.clear_raster(&mut g);
        assert_eq!(g.get(16, 16).kind, Cell::Air);
    }

    #[test]
    fn block_displaces_sand_when_moved() {
        let mut g = Grid::new(32, 32);
        for x in 8..24 {
            for y in 16..22 {
                g.set(x, y, CellData::sand(0));
            }
        }
        let mut block = Block::new(16.0, 4.0, 4.0, 2.0);
        block.rasterize(&mut g);
        let before = g.particle_count();
        block.move_and_displace(Vec2::new(16.0, 18.0), &mut g);
        let after = g.particle_count();
        assert_eq!(before, after, "moving a block must not delete particles");
    }
}
