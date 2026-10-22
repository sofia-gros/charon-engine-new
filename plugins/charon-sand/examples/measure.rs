//! Fixed CPU workload; no window, GPU, real-time sleeping, or extra benchmark framework.
use charon_runtime::Simulation;
use charon_sand::{Command, SandWorld, SAND};
use std::time::Instant;

fn main() -> Result<(), String> {
    let workers = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "4".into())
        .parse::<usize>()
        .map_err(|e| e.to_string())?;
    let mut world = SandWorld::new(480, 270, workers)?;
    let mut times = Vec::new();
    let mut final_frame = None;
    for tick in 0..720 {
        let commands = vec![Command::Paint {
            x: 8 + (tick * 37) % 464,
            y: 8,
            material: SAND,
        }];
        let start = Instant::now();
        let frame = world.step(commands);
        if tick >= 120 {
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        final_frame = Some(frame);
    }
    times.sort_by(f64::total_cmp);
    let frame = final_frame.unwrap();
    println!("grid=480x270 warmup=120 samples={} workers={} actual_last_tick={} sand={} visited_last_tick={} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3}",
        times.len(), workers, frame.workers_used, frame.sand, frame.visited_cells,
        times[299], times[569], times[593], times[599]);
    Ok(())
}
