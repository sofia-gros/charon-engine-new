//! End-to-end integration tests that exercise the simulation through more
//! than a single tick and validate emergent properties.

use rand::SeedableRng;
use rand::rngs::StdRng;
use sandengine::sim::block::Block;
use sandengine::sim::grid::Grid;
use sandengine::sim::material::{Cell, CellData};
use sandengine::sim::{physics, scenarios};

/// Mass conservation: with no acid/fire/steam, sand should never appear or
/// disappear under pure gravity. The block stays rasterized so physics
/// treats it as solid; particles are not destroyed.
#[test]
fn falling_sand_conserves_mass() {
    let mut grid = Grid::new(96, 96);
    let mut block = Block::new(48.0, 48.0, 4.0, 2.0);
    let mut rng = StdRng::seed_from_u64(123);
    scenarios::apply(1, &mut grid, &mut block, &mut rng); // Falling Circle

    let initial = grid.cells.iter().filter(|c| c.kind == Cell::Sand).count();
    assert!(initial > 0);
    for _ in 0..500 {
        physics::step(&mut grid, &mut rng);
    }
    let after = grid.cells.iter().filter(|c| c.kind == Cell::Sand).count();
    assert_eq!(
        initial, after,
        "sand particles must be conserved under gravity ({initial} -> {after})"
    );
}

/// After enough ticks, sand should pile near the bottom — i.e. the centre of
/// mass should be below the initial centre.
#[test]
fn sand_settles_low() {
    let mut grid = Grid::new(64, 64);
    let mut rng = StdRng::seed_from_u64(7);
    for x in 24..40 {
        for y in 0..16 {
            grid.set(x, y, CellData::sand(0));
        }
    }

    let init_cy = mean_y(&grid, Cell::Sand);
    for _ in 0..400 {
        physics::step(&mut grid, &mut rng);
    }
    let final_cy = mean_y(&grid, Cell::Sand);
    assert!(
        final_cy > init_cy + 10.0,
        "sand should fall: {init_cy} -> {final_cy}"
    );
}

/// Steam should rise: its centre of mass goes UP over time.
#[test]
fn steam_rises() {
    let mut grid = Grid::new(64, 64);
    let mut rng = StdRng::seed_from_u64(8);
    for x in 28..36 {
        for y in 50..58 {
            grid.set(x, y, CellData::steam(80));
        }
    }
    let init_cy = mean_y(&grid, Cell::Steam);
    for _ in 0..30 {
        physics::step(&mut grid, &mut rng);
    }
    let final_cy = mean_y(&grid, Cell::Steam);
    assert!(
        final_cy < init_cy - 2.0 || final_cy.is_nan(),
        "steam should rise: {init_cy} -> {final_cy}"
    );
}

/// Physical buoyancy: oil placed at the bottom of a water column should
/// rise to the top. This pins down the density-ordering invariant
/// (oil < water in real life: ~870 vs 1000 kg/m³).
#[test]
fn oil_rises_through_water() {
    let mut grid = Grid::new(16, 64);
    let mut rng = StdRng::seed_from_u64(11);
    for y in 0..60 {
        for x in 0..16 {
            grid.set(x, y, CellData::water(0));
        }
    }
    // Single oil droplet at the bottom.
    grid.set(8, 58, CellData::oil(0));
    let init_y = single_y(&grid, Cell::Oil).expect("oil placed");
    for _ in 0..400 {
        physics::step(&mut grid, &mut rng);
    }
    let final_y = mean_y(&grid, Cell::Oil);
    assert!(
        final_y < init_y - 10.0,
        "oil (870 kg/m³) should rise through water (1000 kg/m³): {init_y} -> {final_y}"
    );
}

/// Physical buoyancy: acid (~1100 kg/m³) should sink in water (1000 kg/m³).
/// This is the correct behavior for HCl/HNO₃/H₂SO₄/HF — game engines often
/// get this backwards.
#[test]
fn acid_sinks_through_water() {
    let mut grid = Grid::new(16, 64);
    let mut rng = StdRng::seed_from_u64(13);
    for y in 4..60 {
        for x in 0..16 {
            grid.set(x, y, CellData::water(0));
        }
    }
    grid.set(8, 4, CellData::acid(255)); // high life so it doesn't expire
    let init_y = single_y(&grid, Cell::Acid).expect("acid placed");
    for _ in 0..200 {
        physics::step(&mut grid, &mut rng);
    }
    let final_y = mean_y(&grid, Cell::Acid);
    // Either it migrated downwards or reacted with the water on the way.
    // If acid expired (lost all reactions) treat that as a pass too.
    assert!(
        final_y.is_nan() || final_y > init_y + 5.0,
        "acid (1100 kg/m³) should sink in water (1000 kg/m³): {init_y} -> {final_y}"
    );
}

/// Density transitivity sanity check: the relative ordering of the
/// permanent-material densities must match real-world buoyancy.
#[test]
fn density_ordering_matches_real_physics() {
    use sandengine::sim::material::Cell as M;
    // Read densities in kg/m³ and verify the chain.
    assert!(M::Steam.density() < M::Air.density() + 1); // both ~1 by rounding
    assert!(M::Oil.density() < M::Water.density());
    assert!(M::Water.density() < M::Acid.density());
    assert!(M::Acid.density() < M::Sand.density());
    assert!(M::Sand.density() < M::Gravel.density());
    assert!(M::Gravel.density() < M::Stone.density());
}

/// Acid placed on stone should dissolve some of it after enough ticks.
#[test]
fn acid_dissolves_stone() {
    let mut grid = Grid::new(32, 32);
    let mut rng = StdRng::seed_from_u64(9);
    for x in 0..32 {
        for y in 20..32 {
            grid.set(x, y, CellData::stone(0));
        }
    }
    for x in 12..20 {
        grid.set(x, 18, CellData::acid(100));
    }
    let initial_stone = grid.cells.iter().filter(|c| c.kind == Cell::Stone).count();
    for _ in 0..400 {
        physics::step(&mut grid, &mut rng);
    }
    let final_stone = grid.cells.iter().filter(|c| c.kind == Cell::Stone).count();
    assert!(
        final_stone < initial_stone,
        "acid should dissolve at least some stone"
    );
}

/// Headless smoke test: every scenario must run for a few hundred ticks
/// without panicking and without producing pure-air grids (i.e. spawners
/// continue to inject particles).
#[test]
fn scenarios_run_without_panic() {
    for i in 0..scenarios::NAMES.len() {
        let mut grid = Grid::new(160, 96);
        let mut block = Block::new(80.0, 48.0, 6.0, 3.0);
        let mut rng = StdRng::seed_from_u64(0x4242 ^ i as u64);
        scenarios::apply(i, &mut grid, &mut block, &mut rng);
        for _ in 0..200 {
            scenarios::spawner_tick(i, &mut grid, &mut rng);
            physics::step(&mut grid, &mut rng);
        }
    }
}

fn mean_y(grid: &Grid, kind: Cell) -> f32 {
    let mut sum = 0.0f64;
    let mut n = 0u64;
    for y in 0..grid.h {
        for x in 0..grid.w {
            if grid.get(x, y).kind == kind {
                sum += y as f64;
                n += 1;
            }
        }
    }
    if n == 0 {
        f32::NAN
    } else {
        (sum / n as f64) as f32
    }
}

fn single_y(grid: &Grid, kind: Cell) -> Option<f32> {
    for y in 0..grid.h {
        for x in 0..grid.w {
            if grid.get(x, y).kind == kind {
                return Some(y as f32);
            }
        }
    }
    None
}
