//! Repeatable CPU workloads. CSV on stdout; diagnostics on stderr. No GPU/FPS claim.
use charon_runtime::Simulation;
use charon_sand::{Command, SandWorld, AIR, SAND, WALL};
use std::time::Instant;

const WARMUP: usize = 60;
const SAMPLES: usize = 240;
fn percentile(values: &mut [f64], p: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values[((values.len() - 1) as f64 * p).ceil() as usize]
}
fn fixture(width: usize, height: usize, scene: &str) -> Vec<u8> {
    let mut cells = vec![AIR; width * height];
    for y in 0..height {
        cells[y * width] = WALL;
        cells[y * width + width - 1] = WALL;
    }
    cells[(height - 1) * width..].fill(WALL);
    if scene == "settled" {
        for y in height / 2..height - 1 {
            cells[y * width + 1..y * width + width - 1].fill(SAND);
        }
    } else if scene == "collapse" {
        for y in height / 4..height / 2 {
            cells[y * width + width / 4..y * width + width * 3 / 4].fill(SAND);
        }
        cells[height / 2 * width + 1..height / 2 * width + width - 1].fill(WALL);
    }
    cells
}
fn main() -> Result<(), String> {
    println!("scene,width,height,workers,warmup,samples,total_p50_ms,total_p95_ms,total_p99_ms,total_max_ms,update_p95_ms,snapshot_p95_ms,rgba_bytes_per_tick,peak_workers_used,final_sand,final_active_tiles,final_visited_cells,checksum");
    for (width, height) in [(480, 270), (960, 540)] {
        for scene in ["sparse", "rain", "collapse", "settled"] {
            let mut reference = None;
            for workers in [1, 2, 4] {
                let mut world =
                    SandWorld::from_cells(width, height, workers, fixture(width, height, scene))?;
                let mut total = Vec::new();
                let mut update = Vec::new();
                let mut snapshot = Vec::new();
                let mut peak_workers = 0;
                let mut final_frame = None;
                for tick in 0..WARMUP + SAMPLES {
                    let mut commands = Vec::new();
                    if scene == "rain" || (scene == "sparse" && tick % 30 == 0) {
                        // Fixed integer input sequence; independent of worker arrival order.
                        commands.push(Command::Paint {
                            x: 8 + (tick * 37) % (width - 16),
                            y: 8,
                            material: SAND,
                        });
                    }
                    if scene == "collapse" && tick == WARMUP {
                        for x in (width / 4..width * 3 / 4).step_by(7) {
                            commands.push(Command::Paint {
                                x,
                                y: height / 2 + 3,
                                material: AIR,
                            });
                        }
                    }
                    let start = Instant::now();
                    let frame = world.step(commands);
                    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                    if tick >= WARMUP {
                        total.push(elapsed);
                        update.push(frame.update_ms);
                        snapshot.push(frame.snapshot_ms);
                        peak_workers = peak_workers.max(frame.workers_used);
                    }
                    final_frame = Some(frame);
                }
                let frame = final_frame.unwrap();
                let result = (world.checksum(), frame.sand);
                if let Some(expected) = reference {
                    assert_eq!(
                        result, expected,
                        "worker divergence in {scene} {width}x{height}"
                    );
                } else {
                    reference = Some(result);
                }
                let p50 = percentile(&mut total, 0.5);
                let p95 = percentile(&mut total, 0.95);
                let p99 = percentile(&mut total, 0.99);
                let max = *total.last().unwrap();
                println!("{scene},{width},{height},{workers},{WARMUP},{SAMPLES},{p50:.6},{p95:.6},{p99:.6},{max:.6},{:.6},{:.6},{},{peak_workers},{},{},{},{:016x}",
                    percentile(&mut update,0.95),percentile(&mut snapshot,0.95),frame.rgba.len(),frame.sand,frame.active_tiles,frame.visited_cells,result.0);
            }
        }
    }
    eprintln!(
        "24 workloads completed; final cell checksums and sand counts match across 1/2/4 workers."
    );
    Ok(())
}
