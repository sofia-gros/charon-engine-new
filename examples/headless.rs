//! Run the physics core without a window or renderer.
use charon_physics::sim::{grid::Grid, material::Cell, material::CellData, physics};
use rand::{SeedableRng, rngs::StdRng};

fn main() {
    let mut grid = Grid::new(365, 365);
    let mut rng = StdRng::seed_from_u64(42);
    for y in 0..20 {
        for x in 172..192 {
            grid.set(x, y, CellData::sand(0));
        }
    }
    for _ in 0..500 {
        physics::step(&mut grid, &mut rng);
    }
    let sand = grid
        .cells
        .iter()
        .filter(|cell| cell.kind == Cell::Sand)
        .count();
    println!("500 ticks completed; {sand} sand cells remain.");
}
