//! Optional safe parallelism for spatially independent active column islands.
//! Columns include a movement/read halo and the full height (repose queries read
//! column surfaces). Interacting islands and custom updaters retain serial order.
use super::{
    grid::{CHUNK, Grid},
    material::Cell,
    physics,
    rules::{Movement, REPOSE_SPAN},
};
use charon_tasks::TaskPool;
use rand::{Rng, RngExt, SeedableRng, rngs::StdRng};
use std::sync::Arc;
struct Island {
    start: usize,
    grid: Grid,
    dirty: Arc<Vec<bool>>,
    chunks_x: usize,
    seed: u64,
}
fn run(mut island: Island) -> Island {
    let mut rng = StdRng::seed_from_u64(island.seed);
    let end = island.start + island.grid.w;
    for cy in (0..island.grid.chunks_y).rev() {
        for cx in 0..island.chunks_x {
            if !island.dirty[cy * island.chunks_x + cx] {
                continue;
            }
            let x0 = (cx * CHUNK).max(island.start);
            let x1 = ((cx + 1) * CHUNK).min(end);
            if x0 >= x1 {
                continue;
            }
            let y0 = cy * CHUNK;
            let y1 = ((cy + 1) * CHUNK).min(island.grid.h);
            for y in (y0..y1).rev() {
                if rng.random::<bool>() {
                    for x in x0..x1 {
                        physics::update_cell(&mut island.grid, x - island.start, y, &mut rng);
                    }
                } else {
                    for x in (x0..x1).rev() {
                        physics::update_cell(&mut island.grid, x - island.start, y, &mut rng);
                    }
                }
            }
        }
    }
    island
}
/// Returns actual independent jobs. The single-grid serial solver remains the
/// fallback when dependencies connect all regions or a custom updater is present.
pub fn step(grid: &mut Grid, rng: &mut impl Rng, pool: &TaskPool) -> usize {
    grid.rotate_dirty();
    if !grid.dirty.iter().any(|v| *v) {
        return 0;
    }
    let palette = grid.materials.palette();
    if palette
        .iter()
        .any(|m| grid.materials.get(m.kind).custom_update.is_some())
    {
        physics::step_active(grid, rng);
        return 1;
    }
    let radius = palette
        .iter()
        .map(|m| usize::from(grid.materials.get(m.kind).lateral_spread))
        .max()
        .unwrap_or(0)
        .max(REPOSE_SPAN)
        + 2;
    let mut columns = vec![false; grid.w];
    for cy in 0..grid.chunks_y {
        for cx in 0..grid.chunks_x {
            if !grid.chunk_dirty(cx, cy) {
                continue;
            }
            for y in cy * CHUNK..((cy + 1) * CHUNK).min(grid.h) {
                for (x, active) in columns
                    .iter_mut()
                    .enumerate()
                    .take(((cx + 1) * CHUNK).min(grid.w))
                    .skip(cx * CHUNK)
                {
                    let kind = grid.cells[y * grid.w + x].kind;
                    if !matches!(kind, Cell::Air | Cell::Block | Cell::Stone)
                        && grid.materials.get(kind).movement != Movement::Solid
                    {
                        *active = true;
                    }
                }
            }
        }
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (x, active) in columns.into_iter().enumerate() {
        if !active {
            continue;
        }
        let start = x.saturating_sub(radius);
        let end = x.saturating_add(radius + 1).min(grid.w);
        if let Some(last) = ranges.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            ranges.push((start, end));
        }
    }
    if ranges.len() < 2 {
        physics::step_active(grid, rng);
        return usize::from(!ranges.is_empty());
    }
    let dirty = Arc::new(grid.dirty.clone());
    let mut islands = Vec::new();
    for (start, end) in ranges {
        let mut local = Grid::new(end - start, grid.h);
        local.materials = grid.materials.clone();
        for y in 0..grid.h {
            local.next[y * local.w..(y + 1) * local.w]
                .copy_from_slice(&grid.cells[y * grid.w + start..y * grid.w + end]);
        }
        islands.push(Island {
            start,
            grid: local,
            dirty: dirty.clone(),
            chunks_x: grid.chunks_x,
            seed: rng.random(),
        });
    }
    let count = islands.len();
    let results = match pool.map(islands, run) {
        Ok(results) => results,
        Err(_) => {
            physics::step_active(grid, rng);
            return 1;
        }
    };
    grid.moved.fill(false);
    for island in results {
        let local = island.grid;
        for y in 0..grid.h {
            for x in 0..local.w {
                let i = y * local.w + x;
                let gx = island.start + x;
                let index = y * grid.w + gx;
                if grid.cells[index] != local.next[i] || local.moved[i] {
                    grid.cells[index] = local.next[i];
                    grid.moved[index] = local.moved[i];
                    grid.mark_dirty(gx, y);
                }
            }
        }
    }
    count
}
