//! Compare the optimized buffer handoff with the original full-copy sweep.
use super::*;
use rand::{SeedableRng, rngs::StdRng};

fn copy_step(grid: &mut Grid, rng: &mut StdRng) {
    grid.rotate_dirty();
    grid.next.copy_from_slice(&grid.cells);
    grid.moved.fill(false);
    for cy in (0..grid.chunks_y).rev() {
        for cx in 0..grid.chunks_x {
            if grid.chunk_dirty(cx, cy) {
                step_chunk(grid, cx, cy, rng);
            }
        }
    }
    grid.swap_buffers();
}

#[test]
fn buffer_handoff_matches_copy_for_motion_reactions_and_external_edits() {
    for (w, h) in [(31, 31), (65, 67), (97, 35)] {
        for scene in 0..4 {
            let mut optimized = Grid::new(w, h);
            for x in 0..w {
                optimized.set(x, h - 1, CellData::stone(0));
            }
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let cell = match scene {
                        0 if y < h / 3 && x > w / 3 && x < 2 * w / 3 => {
                            CellData::sand((x + y) as u8)
                        }
                        1 if x < w / 3 => CellData::water(0),
                        2 if y > h / 3 && y < 2 * h / 3 => match (x + y) % 5 {
                            0 => CellData::oil(0),
                            1 => CellData::fire(120),
                            2 => CellData::acid(200),
                            3 => CellData::water(0),
                            _ => CellData::stone(0),
                        },
                        3 => CellData::stone(0),
                        _ => CellData::AIR,
                    };
                    if cell.kind != Cell::Air {
                        optimized.set(x, y, cell);
                    }
                }
            }
            let mut reference = optimized.clone();
            let mut a = StdRng::seed_from_u64(0x4348_4152);
            let mut b = StdRng::seed_from_u64(0x4348_4152);
            for tick in 0..180 {
                if [12, 40, 90, 140].contains(&tick) {
                    // Includes writing exactly at a chunk edge and waking settled regions.
                    let x = 32.min(w - 2);
                    let y = h / 2;
                    for grid in [&mut optimized, &mut reference] {
                        grid.set(x, y, CellData::AIR);
                        grid.cells[1] = CellData::sand(7);
                        grid.mark_dirty(1, 0);
                    }
                }
                step(&mut optimized, &mut a);
                copy_step(&mut reference, &mut b);
                assert_eq!(
                    optimized.cells, reference.cells,
                    "{w}x{h} scene={scene} tick={tick}"
                );
                assert_eq!(optimized.dirty, reference.dirty);
                assert_eq!(optimized.dirty_next, reference.dirty_next);
                assert_eq!(optimized.moved, reference.moved);
                // Consume the same probe on both streams to compare their states.
                assert_eq!(a.random::<u64>(), b.random::<u64>());
            }
        }
    }
}

#[test]
fn idle_sweep_preserves_cells_and_wakes_after_support_removal() {
    let mut grid = Grid::new(65, 65);
    for x in 0..65 {
        grid.set(x, 32, CellData::stone(0));
    }
    grid.set(32, 31, CellData::sand(0));
    let mut rng = StdRng::seed_from_u64(7);
    for _ in 0..100 {
        step(&mut grid, &mut rng);
    }
    assert_eq!(grid.dirty_chunk_count(), 0);
    let before = grid.cells.clone();
    for _ in 0..10 {
        step(&mut grid, &mut rng);
    }
    assert_eq!(before, grid.cells);
    grid.set(32, 32, CellData::AIR);
    for _ in 0..20 {
        step(&mut grid, &mut rng);
    }
    assert!(
        grid.cells[33 * 65..]
            .iter()
            .any(|cell| cell.kind == Cell::Sand)
    );
}
