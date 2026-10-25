use charon_physics::sim::{
    grid::Grid,
    material::{Cell, CellData},
    physics,
    rules::MaterialTable,
};
use rand::{SeedableRng, rngs::StdRng};

#[derive(Clone, Copy, Debug)]
pub enum Ground {
    Flat,
    Slope,
    Step,
}
pub struct Measurement {
    pub target: f32,
    pub left_degrees: f64,
    pub right_degrees: f64,
    pub ticks: usize,
    pub sand_before: usize,
    pub sand_after: usize,
    pub settled: bool,
}
pub fn measure(angle: f32, ground: Ground, seed: u64) -> Measurement {
    let mut g = Grid::new(365, 250);
    let mut rules = MaterialTable::default();
    rules.set_repose(Cell::Sand, angle).unwrap();
    g.set_materials(rules);
    for x in 0..365 {
        let floor = match ground {
            Ground::Flat => 220,
            Ground::Slope => (220isize + (x as isize - 182) / 8).clamp(200, 240) as usize,
            Ground::Step => {
                if x < 182 {
                    214
                } else {
                    222
                }
            }
        };
        for y in floor..250 {
            g.set(x, y, CellData::stone(0));
        }
    }
    for y in 5..65 {
        for x in 165..200 {
            g.set(x, y, CellData::sand((x ^ y) as u8));
        }
    }
    let sand_before = g.cells.iter().filter(|c| c.kind == Cell::Sand).count();
    let mut rng = StdRng::seed_from_u64(seed);
    let mut ticks = 0;
    while ticks < 3000 {
        physics::step(&mut g, &mut rng);
        ticks += 1;
        if ticks > 250 && g.dirty_chunk_count() == 0 {
            break;
        }
    }
    let heights: Vec<_> = (0..365)
        .map(|x| {
            (0..250)
                .find(|&y| g.get(x, y).kind == Cell::Sand)
                .map_or(0, |y| 250 - y)
        })
        .collect();
    let max = *heights.iter().max().unwrap();
    let peaks: Vec<_> = heights
        .iter()
        .enumerate()
        .filter(|(_, h)| **h == max)
        .map(|(x, _)| x)
        .collect();
    let center = peaks.iter().sum::<usize>() as f64 / peaks.len() as f64;
    let mut angles = Vec::new();
    for left in [true, false] {
        let points: Vec<_> = heights
            .iter()
            .enumerate()
            .filter(|(x, h)| {
                **h > 0
                    && (((*x as f64) < center - 8.0 && left)
                        || ((*x as f64) > center + 8.0 && !left))
            })
            .map(|(x, &h)| (x as f64, h as f64))
            .collect();
        let n = points.len() as f64;
        let sx = points.iter().map(|p| p.0).sum::<f64>();
        let sy = points.iter().map(|p| p.1).sum::<f64>();
        let slope = (n * points.iter().map(|p| p.0 * p.1).sum::<f64>() - sx * sy)
            / (n * points.iter().map(|p| p.0 * p.0).sum::<f64>() - sx * sx);
        angles.push(slope.abs().atan().to_degrees());
    }
    Measurement {
        target: angle,
        left_degrees: angles[0],
        right_degrees: angles[1],
        ticks,
        sand_before,
        sand_after: g.cells.iter().filter(|c| c.kind == Cell::Sand).count(),
        settled: g.dirty_chunk_count() == 0,
    }
}
