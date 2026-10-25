//! Per-material movement rules for the falling-sand cellular automaton.
//!
//! ## Update order
//!
//! Each active tick we hand `cells` to the working buffer without copying, then
//! sweep `next` bottom-to-top. For
//! each row the horizontal direction is randomised; without this, sequential
//! left-to-right updates produce a clearly visible bias (everything drifts
//! right). Hankinson, *"Making Sandspiel"* (2020) and the Powder Toy source
//! both document this trick.
//!
//! ## Movement model
//!
//! Granulars (`Sand`, `Gravel`) fall straight down, then diagonally. Liquids
//! additionally spread sideways up to `spread` cells per tick — a coarse
//! hydrostatic approximation that converges to a flat surface but is far
//! cheaper than a real pressure solve (Mueller et al., *"Particle-Based Fluid
//! Simulation for Interactive Applications"*, SCA 2003 is the alternative).
//! Density determines which material can displace which: heavier sinks,
//! lighter rises.
//!
//! ## Reactions
//!
//! - **Fire ↔ Oil**: fire ignites adjacent oil at ~12% probability per tick.
//!   At 120 ticks/s, expected ignition is ~70 ms after first contact.
//! - **Fire ↔ Water**: contact instantly converts both cells to steam, a
//!   gross simplification of the ~2.26 MJ/kg enthalpy of vaporisation.
//! - **Fire decay**: the `extra` field is a life counter (0..120). On
//!   `life == 0` the cell becomes steam (85%) or gravel/ash (15%).
//! - **Acid dissolves silicates** (Sand, Gravel, Stone) at ~12% probability
//!   per tick. This intentionally models a **hydrofluoric-acid-class
//!   etchant**: HF is the only common acid that dissolves SiO₂. Generic
//!   strong acids (HCl, HNO₃, H₂SO₄) do *not* attack quartz — but they're
//!   visually identical, and treating "acid" as HF gives the most
//!   interesting gameplay.
//! - **Acid + Water → Water** (5%): acid is consumed as it dilutes.
//! - **Acid + Oil → Steam + Steam** (8%): a stand-in for the
//!   exothermic reactions of strong oxidising acids with hydrocarbons.
//! - **Steam decay**: life counter (0..80). On `life == 0` the cell becomes
//!   air — modelling condensation back to (negligible) liquid water.
//!
//! Probabilities are tuned for visual plausibility, **not** kinetic theory.
//! See the README's "Physical accuracy" section for a complete list of
//! deliberate departures from real physics.

use super::grid::{CHUNK, Grid};
use super::material::{Cell, CellData};
use rand::Rng;
use rand::RngExt;

/// Step the simulation one tick using only chunks marked dirty.
pub fn step(grid: &mut Grid, rng: &mut impl Rng) {
    grid.rotate_dirty();
    if !grid.dirty.iter().any(|&active| active) {
        return;
    }
    // Every update rule reads/writes `next`; `cells` is not observed during
    // the exclusive sweep. Hand off the current allocation instead of copying.
    std::mem::swap(&mut grid.cells, &mut grid.next);
    grid.moved.fill(false);

    for cy in (0..grid.chunks_y).rev() {
        for cx in 0..grid.chunks_x {
            if !grid.chunk_dirty(cx, cy) {
                continue;
            }
            step_chunk(grid, cx, cy, rng);
        }
    }

    grid.swap_buffers();
}

fn step_chunk(grid: &mut Grid, cx: usize, cy: usize, rng: &mut impl Rng) {
    let x0 = cx * CHUNK;
    let y0 = cy * CHUNK;
    let x1 = (x0 + CHUNK).min(grid.w);
    let y1 = (y0 + CHUNK).min(grid.h);

    for y in (y0..y1).rev() {
        let l2r: bool = rng.random();
        if l2r {
            for x in x0..x1 {
                update_cell(grid, x, y, rng);
            }
        } else {
            for x in (x0..x1).rev() {
                update_cell(grid, x, y, rng);
            }
        }
    }
}

fn update_cell(grid: &mut Grid, x: usize, y: usize, rng: &mut impl Rng) {
    let i = grid.idx(x, y);
    if grid.moved[i] {
        return;
    }
    let cell = grid.next[i];
    match cell.kind {
        Cell::Sand | Cell::Gravel => update_granular(grid, x, y, i, cell, rng),
        Cell::Water => update_liquid(grid, x, y, i, cell, 5, rng),
        Cell::Oil => update_oil(grid, x, y, i, cell, rng),
        Cell::Acid => update_acid(grid, x, y, i, cell, rng),
        Cell::Fire => update_fire(grid, x, y, i, cell, rng),
        Cell::Steam => update_steam(grid, x, y, i, cell, rng),
        _ => {}
    }
}

#[inline]
fn can_displace(mover: Cell, target: Cell) -> bool {
    if target == Cell::Air {
        return true;
    }
    if target == Cell::Block || target == Cell::Stone {
        return false;
    }
    mover.density() > target.density()
}

fn swap_cells(grid: &mut Grid, i: usize, j: usize) {
    grid.next.swap(i, j);
    grid.moved[i] = true;
    grid.moved[j] = true;
    // Mark both endpoints' chunks dirty for next tick.
    let yi = i / grid.w;
    let xi = i - yi * grid.w;
    grid.mark_dirty(xi, yi);
    let yj = j / grid.w;
    let xj = j - yj * grid.w;
    grid.mark_dirty(xj, yj);
}

fn place(grid: &mut Grid, i: usize, cell: CellData) {
    grid.next[i] = cell;
    grid.moved[i] = true;
    let y = i / grid.w;
    let x = i - y * grid.w;
    grid.mark_dirty(x, y);
}

// --- GRANULAR (Sand, Gravel) ---

fn update_granular(
    grid: &mut Grid,
    x: usize,
    y: usize,
    i: usize,
    cell: CellData,
    rng: &mut impl Rng,
) {
    if y + 1 < grid.h {
        let below = grid.idx(x, y + 1);
        if can_displace(cell.kind, grid.next[below].kind) {
            swap_cells(grid, i, below);
            return;
        }
    }

    if y + 1 < grid.h {
        let go_left: bool = rng.random();
        for dx in if go_left { [-1i32, 1] } else { [1i32, -1] } {
            let nx = x as i32 + dx;
            if nx >= 0 && (nx as usize) < grid.w {
                let di = grid.idx(nx as usize, y + 1);
                if can_displace(cell.kind, grid.next[di].kind) {
                    swap_cells(grid, i, di);
                    return;
                }
            }
        }
    }
}

// --- LIQUID (Water) ---

fn update_liquid(
    grid: &mut Grid,
    x: usize,
    y: usize,
    i: usize,
    cell: CellData,
    spread: i32,
    rng: &mut impl Rng,
) {
    if y + 1 < grid.h {
        let below = grid.idx(x, y + 1);
        if can_displace(cell.kind, grid.next[below].kind) {
            swap_cells(grid, i, below);
            return;
        }
    }

    if y + 1 < grid.h {
        let go_left: bool = rng.random();
        for dx in if go_left { [-1i32, 1] } else { [1i32, -1] } {
            let nx = x as i32 + dx;
            if nx >= 0 && (nx as usize) < grid.w {
                let di = grid.idx(nx as usize, y + 1);
                if can_displace(cell.kind, grid.next[di].kind) {
                    swap_cells(grid, i, di);
                    return;
                }
            }
        }
    }

    let amt: i32 = rng.random_range(1..=spread);
    let dir: i32 = if rng.random() { -1 } else { 1 };
    for s in 1..=amt {
        let nx = x as i32 + dir * s;
        if nx < 0 || (nx as usize) >= grid.w {
            break;
        }
        let si = grid.idx(nx as usize, y);
        let sk = grid.next[si].kind;
        if sk == Cell::Air {
            place(grid, si, cell);
            place(grid, i, CellData::AIR);
            return;
        }
        if sk != Cell::Water {
            break;
        }
    }
}

// --- OIL ---

fn update_oil(grid: &mut Grid, x: usize, y: usize, i: usize, cell: CellData, rng: &mut impl Rng) {
    let dirs: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    for (dx, dy) in dirs {
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;
        if grid.in_bounds(nx, ny) {
            let ni = grid.idx(nx as usize, ny as usize);
            if grid.next[ni].kind == Cell::Fire {
                place(grid, i, CellData::fire(rng.random_range(80..120)));
                return;
            }
        }
    }

    if y + 1 < grid.h {
        let below = grid.idx(x, y + 1);
        if grid.next[below].kind == Cell::Air {
            swap_cells(grid, i, below);
            return;
        }
    }

    if y + 1 < grid.h {
        let go_left: bool = rng.random();
        for dx in if go_left { [-1i32, 1] } else { [1i32, -1] } {
            let nx = x as i32 + dx;
            if nx >= 0 && (nx as usize) < grid.w {
                let di = grid.idx(nx as usize, y + 1);
                if grid.next[di].kind == Cell::Air {
                    swap_cells(grid, i, di);
                    return;
                }
            }
        }
    }

    if y > 0 {
        let above = grid.idx(x, y - 1);
        if grid.next[above].kind == Cell::Water {
            swap_cells(grid, i, above);
            return;
        }
    }

    let dir: i32 = if rng.random() { -1 } else { 1 };
    for s in 1..=4i32 {
        let nx = x as i32 + dir * s;
        if nx < 0 || (nx as usize) >= grid.w {
            break;
        }
        let si = grid.idx(nx as usize, y);
        let sk = grid.next[si].kind;
        if sk == Cell::Air {
            place(grid, si, cell);
            place(grid, i, CellData::AIR);
            return;
        }
        if sk != Cell::Oil {
            break;
        }
    }
}

// --- ACID ---

fn update_acid(grid: &mut Grid, x: usize, y: usize, i: usize, cell: CellData, rng: &mut impl Rng) {
    let life = cell.extra;
    if life == 0 {
        place(grid, i, CellData::AIR);
        return;
    }
    let new_acid = CellData::acid(life.saturating_sub(1));

    let dirs: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    for (dx, dy) in dirs {
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;
        if !grid.in_bounds(nx, ny) {
            continue;
        }
        let ni = grid.idx(nx as usize, ny as usize);
        let nk = grid.next[ni].kind;

        if (nk == Cell::Sand || nk == Cell::Gravel || nk == Cell::Stone) && rng.random_bool(0.12) {
            place(grid, ni, CellData::AIR);
            let remain = life.saturating_sub(15);
            place(
                grid,
                i,
                if remain > 0 {
                    CellData::acid(remain)
                } else {
                    CellData::AIR
                },
            );
            return;
        }

        if nk == Cell::Water && rng.random_bool(0.05) {
            place(grid, i, CellData::water(rng.random()));
            return;
        }

        if nk == Cell::Oil && rng.random_bool(0.08) {
            place(grid, ni, CellData::steam(40));
            place(grid, i, CellData::steam(30));
            return;
        }
    }

    if y + 1 < grid.h {
        let below = grid.idx(x, y + 1);
        let bk = grid.next[below].kind;
        if bk == Cell::Air || bk == Cell::Steam || bk == Cell::Fire {
            let displaced = grid.next[below];
            place(grid, below, new_acid);
            place(
                grid,
                i,
                if bk == Cell::Air {
                    CellData::AIR
                } else {
                    displaced
                },
            );
            return;
        }
    }

    if y + 1 < grid.h {
        let go_left: bool = rng.random();
        for dx in if go_left { [-1i32, 1] } else { [1i32, -1] } {
            let nx = x as i32 + dx;
            if nx >= 0 && (nx as usize) < grid.w {
                let di = grid.idx(nx as usize, y + 1);
                if grid.next[di].kind == Cell::Air {
                    place(grid, di, new_acid);
                    place(grid, i, CellData::AIR);
                    return;
                }
            }
        }
    }

    let dir: i32 = if rng.random() { -1 } else { 1 };
    for s in 1..=3i32 {
        let nx = x as i32 + dir * s;
        if nx < 0 || (nx as usize) >= grid.w {
            break;
        }
        let si = grid.idx(nx as usize, y);
        if grid.next[si].kind == Cell::Air {
            place(grid, si, new_acid);
            place(grid, i, CellData::AIR);
            return;
        }
        if grid.next[si].kind != Cell::Acid {
            break;
        }
    }

    place(grid, i, new_acid);
}

// --- FIRE ---

fn update_fire(grid: &mut Grid, x: usize, y: usize, i: usize, cell: CellData, rng: &mut impl Rng) {
    let life = cell.extra;
    if life == 0 {
        if rng.random_bool(0.15) {
            place(grid, i, CellData::gravel(rng.random()));
        } else {
            place(grid, i, CellData::steam(rng.random_range(40..70)));
        }
        return;
    }
    let new_cell = CellData::fire(life.saturating_sub(1));

    let dirs: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    for (dx, dy) in dirs {
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;
        if grid.in_bounds(nx, ny) {
            let ni = grid.idx(nx as usize, ny as usize);
            if grid.next[ni].kind == Cell::Water {
                place(grid, ni, CellData::steam(50));
                place(grid, i, CellData::steam(20));
                return;
            }
        }
    }

    // Oil is the only flammable material in this model. Sand (SiO₂) does
    // not combust — it melts above ~1700°C but does not undergo
    // exothermic oxidation. The 0.12 ignition probability per
    // fire-adjacent tick (≈8 ticks @ 120 Hz ≈ 70 ms expected ignition)
    // is a game-feel constant, not an Arrhenius rate.
    for (dx, dy) in dirs {
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;
        if grid.in_bounds(nx, ny) {
            let ni = grid.idx(nx as usize, ny as usize);
            if grid.next[ni].kind == Cell::Oil && rng.random_bool(0.12) {
                place(grid, ni, CellData::fire(rng.random_range(80..120)));
            }
        }
    }

    if y > 0 {
        let dx: i32 = rng.random_range(-1..=1);
        let nx = (x as i32 + dx).clamp(0, grid.w as i32 - 1) as usize;
        let above = grid.idx(nx, y - 1);
        let ak = grid.next[above].kind;
        if ak == Cell::Air || ak == Cell::Steam {
            let displaced = grid.next[above];
            place(grid, above, new_cell);
            place(
                grid,
                i,
                if ak == Cell::Air {
                    CellData::AIR
                } else {
                    displaced
                },
            );
            return;
        }
    }

    place(grid, i, new_cell);
}

// --- STEAM ---

fn update_steam(grid: &mut Grid, x: usize, y: usize, i: usize, cell: CellData, rng: &mut impl Rng) {
    let life = cell.extra;
    if life == 0 {
        place(grid, i, CellData::AIR);
        return;
    }
    let nc = CellData::steam(life.saturating_sub(1));

    if y > 0 {
        let dx: i32 = rng.random_range(-1..=1);
        let nx = (x as i32 + dx).clamp(0, grid.w as i32 - 1) as usize;
        let above = grid.idx(nx, y - 1);
        let ak = grid.next[above].kind;
        if ak == Cell::Air {
            place(grid, above, nc);
            place(grid, i, CellData::AIR);
            return;
        }
        if ak == Cell::Water || ak == Cell::Oil || ak == Cell::Acid || ak == Cell::Fire {
            let displaced = grid.next[above];
            place(grid, above, nc);
            place(grid, i, displaced);
            return;
        }
    }

    let dx: i32 = rng.random_range(-1..=1);
    let nx = x as i32 + dx;
    if nx >= 0 && (nx as usize) < grid.w {
        let si = grid.idx(nx as usize, y);
        if grid.next[si].kind == Cell::Air {
            place(grid, si, nc);
            place(grid, i, CellData::AIR);
            return;
        }
    }

    place(grid, i, nc);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deterministic_rng() -> rand::rngs::StdRng {
        use rand::SeedableRng;
        rand::rngs::StdRng::seed_from_u64(42)
    }

    #[test]
    fn sand_falls() {
        let mut g = Grid::new(8, 8);
        g.set(4, 0, CellData::sand(0));
        let mut rng = deterministic_rng();
        for _ in 0..16 {
            step(&mut g, &mut rng);
        }
        let mut found_at_bottom = false;
        for x in 0..g.w {
            if g.get(x, g.h - 1).kind == Cell::Sand {
                found_at_bottom = true;
                break;
            }
        }
        assert!(found_at_bottom, "sand should reach the bottom row");
    }

    #[test]
    fn water_levels_out() {
        let mut g = Grid::new(20, 20);
        for x in 0..5 {
            for y in 5..10 {
                g.set(x, y, CellData::water(0));
            }
        }
        let mut rng = deterministic_rng();
        for _ in 0..200 {
            step(&mut g, &mut rng);
        }
        let water: Vec<usize> = (0..g.w)
            .filter(|x| (0..g.h).any(|y| g.get(*x, y).kind == Cell::Water))
            .collect();
        assert!(
            water.len() > 5,
            "water should spread beyond its initial column"
        );
    }

    #[test]
    fn fire_decays_to_smoke_or_ash() {
        // A single fire cell with life=1 must decay to either steam or ash
        // within a handful of ticks — fire never stays fire forever.
        let mut g = Grid::new(8, 8);
        g.set(4, 4, CellData::fire(1));
        let mut rng = deterministic_rng();
        for _ in 0..10 {
            step(&mut g, &mut rng);
        }
        let any_fire = (0..g.h).any(|y| (0..g.w).any(|x| g.get(x, y).kind == Cell::Fire));
        assert!(
            !any_fire,
            "fire(life=1) should have decayed within 10 ticks"
        );
    }

    #[test]
    fn dirty_chunks_shrink_when_idle() {
        let mut g = Grid::new(128, 128);
        // Fill the whole grid with stone (immobile) and step a few times.
        for y in 0..g.h {
            for x in 0..g.w {
                g.set(x, y, CellData::stone(0));
            }
        }
        let mut rng = deterministic_rng();
        for _ in 0..4 {
            step(&mut g, &mut rng);
        }
        // Stone is immobile, so dirty count should be small in steady state.
        let dirty = g.dirty_chunk_count();
        let total = g.chunks_x * g.chunks_y;
        assert!(
            dirty < total,
            "an all-stone grid should not stay 100% dirty (got {dirty}/{total})"
        );
    }
}

#[cfg(test)]
#[path = "../../tests/support/copy_equivalence.rs"]
mod copy_equivalence;
