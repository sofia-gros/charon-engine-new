#![cfg(feature = "parallel")]
use charon_physics::sim::{
    grid::Grid,
    material::{Cell, CellData},
    parallel,
};
use charon_tasks::TaskPool;
use rand::{SeedableRng, rngs::StdRng};
fn grid() -> Grid {
    let mut g = Grid::new(256, 96);
    for x in 0..256 {
        g.set(x, 95, CellData::stone(0));
    }
    for x in [36, 40, 44, 198, 202, 206] {
        for y in 5..20 {
            g.set(x, y, CellData::sand(0));
        }
    }
    g
}
#[test]
fn independent_islands_conserve_mass_and_are_deterministic_across_worker_counts() {
    let mut a = grid();
    let mut b = a.clone();
    let mut ar = StdRng::seed_from_u64(7);
    let mut br = StdRng::seed_from_u64(7);
    let one = TaskPool::new(0);
    let many = TaskPool::new(2);
    let mass = a.cells.iter().filter(|c| c.kind == Cell::Sand).count();
    let mut jobs = 0;
    for _ in 0..140 {
        jobs = jobs.max(parallel::step(&mut a, &mut ar, &one));
        parallel::step(&mut b, &mut br, &many);
        assert_eq!(a.cells, b.cells);
    }
    assert!(jobs >= 2);
    assert_eq!(
        a.cells.iter().filter(|c| c.kind == Cell::Sand).count(),
        mass
    );
}

#[test]
fn separate_water_regions_keep_mass_and_match_across_cpu_budgets() {
    let mut a = grid();
    for c in &mut a.cells {
        if c.kind == Cell::Sand {
            *c = CellData::water(0);
        }
    }
    let mut b = a.clone();
    let mass = a.cells.iter().filter(|c| c.kind == Cell::Water).count();
    let (mut ar, mut br) = (StdRng::seed_from_u64(11), StdRng::seed_from_u64(11));
    let (one, many) = (TaskPool::new(0), TaskPool::new(2));
    for _ in 0..90 {
        parallel::step(&mut a, &mut ar, &one);
        parallel::step(&mut b, &mut br, &many);
        assert_eq!(a.cells, b.cells);
    }
    assert_eq!(
        a.cells.iter().filter(|c| c.kind == Cell::Water).count(),
        mass
    );
}
