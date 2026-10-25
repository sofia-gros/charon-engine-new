//! Initial-condition presets and "live spawner" ticks for each scenario.
//!
//! A scenario is a (`apply`, `spawn`) pair: `apply` paints the starting grid
//! state once, `spawn` runs every tick to inject new particles (e.g. for the
//! sand-rain or volcano scenarios). Keeping the spawn as a tiny function
//! rather than baked frames makes them indefinitely stable.

use charon_physics::sim::block::Block;
use charon_physics::sim::grid::Grid;
use charon_physics::sim::material::{Cell, CellData};
use rand::Rng;
use rand::RngExt;

pub const NAMES: &[&str] = &[
    "Triangle on Floor",
    "Falling Circle",
    "Hourglass",
    "Sand Rain",
    "Block in Pile",
    "Volcano",
    "Waterfall",
    "Dam Break",
    "Lava Flow",
    "Layer Cake",
    "Dual Sand Sort",
    "Oil Spill",
    "Acid Rain",
    "Erosion Cliff",
    "Geyser",
    "River Delta",
    "Sandstorm",
    "Explosion",
];

pub fn apply(index: usize, grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    grid.clear();
    match index % NAMES.len() {
        0 => triangle_on_floor(grid, block, rng),
        1 => falling_circle(grid, block, rng),
        2 => hourglass(grid, block, rng),
        3 => sand_rain(grid, block),
        4 => block_in_pile(grid, block, rng),
        5 => volcano(grid, block, rng),
        6 => waterfall(grid, block, rng),
        7 => dam_break(grid, block, rng),
        8 => lava_flow(grid, block, rng),
        9 => layer_cake(grid, block, rng),
        10 => dual_sand_sort(grid, block, rng),
        11 => oil_spill(grid, block, rng),
        12 => acid_rain(grid, block, rng),
        13 => erosion_cliff(grid, block, rng),
        14 => geyser(grid, block, rng),
        15 => river_delta(grid, block, rng),
        16 => sandstorm(grid, block),
        17 => explosion(grid, block, rng),
        _ => unreachable!(),
    }
    block.rasterize(grid);
}

pub fn spawner_tick(scenario: usize, grid: &mut Grid, rng: &mut impl Rng) {
    match scenario % NAMES.len() {
        3 => rain_sand(grid, rng),
        5 => volcano_spawn(grid, rng),
        6 => waterfall_spawn(grid, rng),
        11 => oil_spill_spawn(grid, rng),
        12 => acid_rain_spawn(grid, rng),
        13 => erosion_cliff_spawn(grid, rng),
        14 => geyser_spawn(grid, rng),
        15 => river_delta_spawn(grid, rng),
        16 => sandstorm_spawn(grid, rng),
        _ => {}
    }
}

// ============ Presets ============

fn triangle_on_floor(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let cx = grid.w / 2;
    let base = grid.w / 3;
    let peak_y = grid.h / 4;
    let floor_y = grid.h - 10;
    for y in peak_y..floor_y {
        let frac = (y - peak_y) as f32 / (floor_y - peak_y) as f32;
        let hw = (frac * base as f32 / 2.0) as usize;
        for x in cx.saturating_sub(hw)..=(cx + hw).min(grid.w - 1) {
            grid.set(x, y, CellData::sand(rng.random()));
        }
    }
    *block = Block::new(grid.w as f32 * 0.8, grid.h as f32 * 0.5, 30.0, 15.0);
}

fn falling_circle(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let cx = grid.w as f32 / 2.0;
    let cy = grid.h as f32 / 4.0;
    let r = (grid.w.min(grid.h) as f32 / 6.0).min(120.0);
    for y in 0..grid.h {
        for x in 0..grid.w {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy <= r * r {
                grid.set(x, y, CellData::sand(rng.random()));
            }
        }
    }
    *block = Block::new(cx, grid.h as f32 * 0.75, 40.0, 15.0);
}

fn hourglass(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let w = grid.w;
    let h = grid.h;
    let mid_y = h / 2;
    let gap = 8;
    let ww = 8;
    let inset = w / 5;
    for y in 0..h {
        for wx in 0..ww {
            grid.set(inset + wx, y, CellData::stone(rng.random()));
            grid.set(w - inset - ww + wx, y, CellData::stone(rng.random()));
        }
    }
    for x in (inset + ww)..(w - inset - ww) {
        let cx = w / 2;
        if (x as i32 - cx as i32).unsigned_abs() > gap {
            for wy in 0..ww {
                grid.set(x, mid_y + wy, CellData::stone(rng.random()));
            }
        }
    }
    for y in ww..mid_y {
        for x in (inset + ww + 1)..(w - inset - ww - 1) {
            grid.set(x, y, CellData::sand(rng.random()));
        }
    }
    *block = Block::new(w as f32 / 2.0, mid_y as f32 + 2.0, gap as f32, 3.0);
}

fn sand_rain(grid: &mut Grid, block: &mut Block) {
    *block = Block::new(grid.w as f32 / 2.0, grid.h as f32 / 2.0, 50.0, 15.0);
}

fn block_in_pile(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let fill = grid.h / 2;
    for y in fill..grid.h {
        for x in 0..grid.w {
            grid.set(x, y, CellData::sand(rng.random()));
        }
    }
    *block = Block::new(
        grid.w as f32 / 2.0,
        (fill as f32 + grid.h as f32) / 2.0,
        35.0,
        20.0,
    );
}

fn volcano(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let cx = grid.w / 2;
    let base_y = grid.h - 5;
    let peak_y = grid.h / 3;
    for y in peak_y..base_y {
        let frac = (y - peak_y) as f32 / (base_y - peak_y) as f32;
        let hw = (frac * grid.w as f32 / 4.0) as usize;
        for x in cx.saturating_sub(hw)..=(cx + hw).min(grid.w - 1) {
            grid.set(x, y, CellData::stone(rng.random()));
        }
    }
    let crater_w = 15;
    for y in peak_y..(peak_y + 20) {
        for x in cx.saturating_sub(crater_w)..=(cx + crater_w).min(grid.w - 1) {
            grid.set(x, y, CellData::AIR);
        }
    }
    *block = Block::new(grid.w as f32 * 0.2, grid.h as f32 * 0.5, 25.0, 12.0);
}

fn waterfall(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let ledge_y = grid.h / 3;
    for x in 0..(grid.w * 2 / 3) {
        grid.set(x, ledge_y, CellData::stone(rng.random()));
    }
    let ledge2_y = grid.h * 2 / 3;
    for x in (grid.w / 3)..grid.w {
        grid.set(x, ledge2_y, CellData::stone(rng.random()));
    }
    for y in (ledge_y.saturating_sub(40))..ledge_y {
        for x in 0..(grid.w / 3) {
            grid.set(x, y, CellData::water(rng.random()));
        }
    }
    *block = Block::new(grid.w as f32 / 2.0, ledge_y as f32 - 20.0, 30.0, 10.0);
}

fn dam_break(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let dam_x = grid.w / 3;
    let dam_w = 8;
    for y in (grid.h / 4)..grid.h {
        for wx in 0..dam_w {
            grid.set(dam_x + wx, y, CellData::stone(rng.random()));
        }
    }
    for y in (grid.h / 3)..grid.h {
        for x in 0..dam_x {
            grid.set(x, y, CellData::water(rng.random()));
        }
    }
    for y in (grid.h * 3 / 4)..grid.h {
        for x in (dam_x + dam_w + 20)..grid.w {
            grid.set(x, y, CellData::sand(rng.random()));
        }
    }
    *block = Block::new(
        dam_x as f32 + 4.0,
        grid.h as f32 / 2.0,
        dam_w as f32 / 2.0,
        30.0,
    );
}

fn lava_flow(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    for y in 0..grid.h {
        let slope_x = (y as f32 / grid.h as f32 * grid.w as f32 * 0.6) as usize;
        for x in slope_x..grid.w.min(slope_x + 6) {
            grid.set(x, y, CellData::stone(rng.random()));
        }
    }
    for y in 10..60 {
        for x in 10..80 {
            grid.set(x, y, CellData::fire(rng.random_range(60..120)));
        }
    }
    *block = Block::new(grid.w as f32 * 0.7, grid.h as f32 * 0.4, 25.0, 15.0);
}

fn layer_cake(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let layers = 8;
    let layer_h = grid.h / (layers + 2);
    for l in 0..layers {
        let y_start = grid.h - (l + 1) * layer_h;
        let y_end = y_start + layer_h;
        for y in y_start..y_end {
            for x in 0..grid.w {
                let cell = match l % 4 {
                    0 => CellData::sand(rng.random()),
                    1 => CellData::water(rng.random()),
                    2 => CellData::stone(rng.random()),
                    _ => CellData::sand(rng.random()),
                };
                grid.set(x, y, cell);
            }
        }
    }
    *block = Block::new(grid.w as f32 / 2.0, grid.h as f32 / 2.0, 40.0, 20.0);
}

fn dual_sand_sort(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let wall_x = grid.w / 2;
    let wall_w = 6.min(grid.w / 16).max(1);
    let fill_top = grid.h / 4;
    let gap_h = (grid.h / 9).max(4);
    let gap_top = grid.h.saturating_sub(gap_h + grid.h / 24);
    for y in 0..grid.h {
        if y >= gap_top && y < gap_top + gap_h {
            continue;
        }
        for wx in 0..wall_w {
            grid.set(wall_x + wx, y, CellData::stone(rng.random()));
        }
    }
    for y in fill_top..grid.h {
        for x in 0..wall_x {
            grid.set(x, y, CellData::gravel(rng.random()));
        }
    }
    for y in fill_top..grid.h {
        for x in (wall_x + wall_w)..grid.w {
            grid.set(x, y, CellData::sand(rng.random()));
        }
    }
    *block = Block::new(
        wall_x as f32 + 3.0,
        (gap_top + gap_h / 2) as f32,
        wall_w as f32 / 2.0 + 1.0,
        gap_h as f32 / 2.0 + 1.0,
    );
}

fn oil_spill(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let w = grid.w;
    let h = grid.h;
    let left = w / 4;
    let right = w * 3 / 4;
    let top = h / 2;
    let wall_t = 8;
    for x in left..right {
        for dy in 0..wall_t {
            grid.set(x, h - 1 - dy, CellData::stone(rng.random()));
        }
    }
    for y in top..h {
        for dx in 0..wall_t {
            grid.set(left + dx, y, CellData::stone(rng.random()));
            grid.set(right - 1 - dx, y, CellData::stone(rng.random()));
        }
    }
    for y in (top + 80)..(h - wall_t) {
        for x in (left + wall_t)..(right - wall_t) {
            grid.set(x, y, CellData::water(rng.random()));
        }
    }
    for y in (top + 50)..(top + 80) {
        for x in (left + wall_t)..(right - wall_t) {
            grid.set(x, y, CellData::oil(rng.random()));
        }
    }
    *block = Block::new(w as f32 / 2.0, top as f32 - 40.0, 30.0, 15.0);
}

fn acid_rain(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let w = grid.w;
    let h = grid.h;
    let pillar_w = 30;
    let spacing = w / 6;
    for i in 1..=5usize {
        let px = i * spacing;
        let top = h / 3 + (i % 3) * 60;
        for y in top..h {
            for dx in 0..pillar_w {
                let x = px.saturating_sub(pillar_w / 2) + dx;
                if x < w {
                    grid.set(x, y, CellData::stone(rng.random()));
                }
            }
        }
    }
    let arch_y = h * 2 / 3;
    for x in (2 * spacing)..(4 * spacing).min(w) {
        for dy in 0..12 {
            grid.set(x, arch_y + dy, CellData::stone(rng.random()));
        }
    }
    *block = Block::new(w as f32 / 2.0, h as f32 / 5.0, 25.0, 12.0);
}

fn erosion_cliff(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let w = grid.w;
    let h = grid.h;
    let cliff_w = w / 3;
    let cliff_top = h / 6;
    for y in cliff_top..h {
        for x in 0..cliff_w {
            grid.set(x, y, CellData::stone(rng.random()));
        }
    }
    for y in (cliff_top.saturating_sub(30))..cliff_top {
        for x in 0..cliff_w {
            grid.set(x, y, CellData::water(rng.random()));
        }
    }
    *block = Block::new(w as f32 * 0.6, h as f32 / 2.0, 25.0, 15.0);
}

fn geyser(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let cx = grid.w / 2;
    let h = grid.h;
    let cone_top = h / 2;
    let cone_base_hw = grid.w / 4;
    for y in cone_top..h {
        let frac = (y - cone_top) as f32 / (h - cone_top) as f32;
        let hw = (frac * cone_base_hw as f32) as usize;
        for x in cx.saturating_sub(hw)..=(cx + hw).min(grid.w - 1) {
            grid.set(x, y, CellData::stone(rng.random()));
        }
    }
    let channel_hw = 4;
    for y in cone_top..(h - 20) {
        for x in cx.saturating_sub(channel_hw)..=(cx + channel_hw).min(grid.w - 1) {
            grid.set(x, y, CellData::AIR);
        }
    }
    *block = Block::new(grid.w as f32 * 0.8, h as f32 * 0.3, 20.0, 12.0);
}

fn river_delta(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let w = grid.w;
    let h = grid.h;
    for y in (h / 2)..h {
        for x in 0..w {
            grid.set(x, y, CellData::sand(rng.random()));
        }
    }
    for y in (h / 4)..h {
        let wobble = ((y as f32 * 0.03).sin() * 40.0) as i32;
        let cx = w as i32 / 4 + wobble;
        for dx in -30..30 {
            let x = (cx + dx).clamp(0, w as i32 - 1) as usize;
            grid.set(x, y, CellData::AIR);
        }
    }
    *block = Block::new(w as f32 * 0.7, h as f32 * 0.3, 25.0, 12.0);
}

fn sandstorm(grid: &mut Grid, block: &mut Block) {
    *block = Block::new(grid.w as f32 / 3.0, grid.h as f32 / 2.0, 40.0, 20.0);
}

fn explosion(grid: &mut Grid, block: &mut Block, rng: &mut impl Rng) {
    let cx = grid.w as f32 / 2.0;
    let cy = grid.h as f32 / 2.0;
    let fire_r = (grid.w.min(grid.h) as f32 / 5.0).min(150.0);
    let sand_r = fire_r + 80.0;
    for y in 0..grid.h {
        for x in 0..grid.w {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= fire_r {
                grid.set(x, y, CellData::fire(rng.random_range(80..150)));
            } else if dist <= sand_r {
                grid.set(x, y, CellData::sand(rng.random()));
            }
        }
    }
    *block = Block::new(grid.w as f32 * 0.85, grid.h as f32 * 0.15, 25.0, 15.0);
}

// ============ Live spawners ============

fn rain_sand(grid: &mut Grid, rng: &mut impl Rng) {
    for x in (0..grid.w).step_by(2) {
        if rng.random_bool(0.4) && grid.get(x, 0).kind == Cell::Air {
            grid.set(x, 0, CellData::sand(rng.random()));
        }
    }
}

fn volcano_spawn(grid: &mut Grid, rng: &mut impl Rng) {
    let cx = grid.w / 2;
    for dx in -6..=6i32 {
        let x = (cx as i32 + dx).clamp(0, grid.w as i32 - 1) as usize;
        if rng.random_bool(0.3) && grid.get(x, 0).kind == Cell::Air {
            if rng.random_bool(0.5) {
                grid.set(x, 0, CellData::fire(rng.random_range(80..120)));
            } else {
                grid.set(x, 0, CellData::sand(rng.random()));
            }
        }
    }
}

fn waterfall_spawn(grid: &mut Grid, rng: &mut impl Rng) {
    for x in 5..30 {
        if rng.random_bool(0.5) && grid.get(x, 0).kind == Cell::Air {
            grid.set(x, 0, CellData::water(rng.random()));
        }
    }
}

fn oil_spill_spawn(grid: &mut Grid, rng: &mut impl Rng) {
    let left = grid.w / 4 + 10;
    let right = grid.w * 3 / 4 - 10;
    for x in (left..right).step_by(4) {
        if rng.random_bool(0.15) && grid.get(x, 0).kind == Cell::Air {
            grid.set(x, 0, CellData::oil(rng.random()));
        }
    }
}

fn acid_rain_spawn(grid: &mut Grid, rng: &mut impl Rng) {
    for x in (0..grid.w).step_by(3) {
        if rng.random_bool(0.15) && grid.get(x, 0).kind == Cell::Air {
            grid.set(x, 0, CellData::acid(rng.random_range(60..100)));
        }
    }
}

fn erosion_cliff_spawn(grid: &mut Grid, rng: &mut impl Rng) {
    for x in 0..(grid.w / 3) {
        if rng.random_bool(0.4) && grid.get(x, 0).kind == Cell::Air {
            grid.set(x, 0, CellData::water(rng.random()));
        }
    }
    for x in 10..(grid.w / 4) {
        if rng.random_bool(0.05) && grid.get(x, 0).kind == Cell::Air {
            grid.set(x, 0, CellData::acid(rng.random_range(60..100)));
        }
    }
}

fn geyser_spawn(grid: &mut Grid, rng: &mut impl Rng) {
    let cx = grid.w / 2;
    let bottom = grid.h - 25;
    for dx in -4..=4i32 {
        let x = (cx as i32 + dx).clamp(0, grid.w as i32 - 1) as usize;
        if rng.random_bool(0.5) && grid.get(x, bottom).kind == Cell::Air {
            if rng.random_bool(0.6) {
                grid.set(x, bottom, CellData::water(rng.random()));
            } else {
                grid.set(x, bottom, CellData::steam(rng.random_range(60..120)));
            }
        }
    }
}

fn river_delta_spawn(grid: &mut Grid, rng: &mut impl Rng) {
    for x in 0..40 {
        if rng.random_bool(0.5) && grid.get(x, 0).kind == Cell::Air {
            grid.set(x, 0, CellData::water(rng.random()));
        }
    }
}

fn sandstorm_spawn(grid: &mut Grid, rng: &mut impl Rng) {
    let x = grid.w - 1;
    for y in (0..grid.h).step_by(2) {
        if rng.random_bool(0.6) && grid.get(x, y).kind == Cell::Air {
            grid.set(x, y, CellData::sand(rng.random()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn every_scenario_applies_without_panic() {
        // Use both a small and a large grid to flush out off-by-ones.
        for (w, h) in [(160usize, 120usize), (1280, 720)] {
            let mut grid = Grid::new(w, h);
            let mut block = Block::new(w as f32 / 2.0, h as f32 / 2.0, 10.0, 5.0);
            let mut rng = rand::rngs::StdRng::seed_from_u64(7);
            for i in 0..NAMES.len() {
                apply(i, &mut grid, &mut block, &mut rng);
                assert!(!grid.cells.is_empty(), "scenario {i} produced state");
            }
        }
    }

    #[test]
    fn spawners_only_touch_air() {
        let mut grid = Grid::new(160, 120);
        let mut rng = rand::rngs::StdRng::seed_from_u64(11);
        for i in 0..NAMES.len() {
            spawner_tick(i, &mut grid, &mut rng);
        }
    }
}
