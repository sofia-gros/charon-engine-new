use rand::{SeedableRng, rngs::StdRng};
use sandengine::sim::{grid::Grid, material::CellData, physics};

pub const SEED: u64 = 0x4348_4152_4f4e;
pub const SIZES: [(usize, usize); 3] = [(365, 365), (640, 360), (1280, 720)];
pub const SCENES: [&str; 4] = ["falling_sand", "settled_stone", "liquid", "reactions"];

pub fn build(name: &str, w: usize, h: usize) -> Grid {
    let mut grid = Grid::new(w, h);
    // Contained scenes; all material creation happens outside the measured window.
    for x in 0..w {
        grid.set(x, h - 1, CellData::stone(0));
    }
    match name {
        "falling_sand" => {
            for y in h / 16..h / 4 {
                for x in w / 4..3 * w / 4 {
                    grid.set(x, y, CellData::sand((x ^ y) as u8));
                }
            }
        }
        "settled_stone" => {
            grid.cells.fill(CellData::stone(0));
            grid.dirty_next.fill(true);
            let mut rng = StdRng::seed_from_u64(SEED);
            for _ in 0..8 {
                physics::step(&mut grid, &mut rng);
            }
        }
        "liquid" => {
            for y in h / 3..h - 1 {
                for x in 1..w / 4 {
                    grid.set(x, y, CellData::water((x + y) as u8));
                }
            }
        }
        "reactions" => {
            for y in h / 3..2 * h / 3 {
                for x in w / 8..7 * w / 8 {
                    let cell = match (x + 3 * y) % 5 {
                        0 => CellData::fire(120),
                        1 => CellData::oil(0),
                        2 => CellData::water(0),
                        3 => CellData::acid(255),
                        _ => CellData::stone(0),
                    };
                    grid.set(x, y, cell);
                }
            }
        }
        _ => panic!("unknown scene {name}"),
    }
    grid
}
