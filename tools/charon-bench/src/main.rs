//! Fixed-window profiling; timings are observations, never CI speed assertions.
#[path = "../../../tests/support/repose_measurement.rs"]
mod repose_measurement;
mod scenes;
use charon_core::App;
use charon_physics::sim::{grid::Grid, physics, simulation::Simulation};
use charon_sand::{FallingSandPlugin, SandClient, SandState};
use rand::{SeedableRng, rngs::StdRng};
use serde::Serialize;
use std::hint::black_box;
use std::time::Instant;

#[derive(Serialize)]
struct Case {
    scene: String,
    layer: String,
    width: usize,
    height: usize,
    seed: u64,
    trials: usize,
    ticks_per_trial: usize,
    mean_tick_ms: f64,
    p50_tick_ms: f64,
    p95_tick_ms: f64,
    max_tick_ms: f64,
    mean_active_chunks: f64,
    max_active_chunks: usize,
    grid_buffer_bytes: usize,
    snapshot_cell_bytes: usize,
    state_checksum: String,
    // Budgets refer only to this measured lane, with 50% headroom in the period.
    within_60hz_half_budget: bool,
    within_30hz_half_budget: bool,
}
#[derive(Serialize)]
struct Report {
    schema: u32,
    rust_package: String,
    samples: Vec<Case>,
    process_peak_rss_kib: Option<usize>,
}

enum Runner {
    Kernel(Box<Grid>, Box<StdRng>),
    Snapshot(Box<Simulation>),
    Plugin { app: Box<App>, _client: SandClient },
}
impl Runner {
    fn build(layer: &str, grid: Grid) -> Self {
        match layer {
            "kernel" => Self::Kernel(
                Box::new(grid),
                Box::new(StdRng::seed_from_u64(scenes::SEED)),
            ),
            "simulation_snapshot" => {
                Self::Snapshot(Box::new(Simulation::from_grid(grid, scenes::SEED)))
            }
            "plugin_snapshot" => {
                let (plugin, client) =
                    FallingSandPlugin::new(Simulation::from_grid(grid, scenes::SEED), 8).unwrap();
                let mut app = App::new();
                app.add_plugin(plugin).unwrap();
                app.start().unwrap();
                Self::Plugin {
                    app: Box::new(app),
                    _client: client,
                }
            }
            _ => unreachable!(),
        }
    }
    fn step(&mut self) {
        match self {
            Self::Kernel(grid, rng) => physics::step(grid, rng.as_mut()),
            Self::Snapshot(sim) => {
                sim.step();
                black_box(sim.snapshot());
            }
            Self::Plugin { app, .. } => app.tick().unwrap(),
        }
    }
    fn grid(&self) -> &Grid {
        match self {
            Self::Kernel(grid, _) => grid,
            Self::Snapshot(sim) => sim.grid(),
            Self::Plugin { app, .. } => app.world().get::<SandState>().unwrap().0.grid(),
        }
    }
}
fn checksum(grid: &Grid) -> u64 {
    grid.cells.iter().fold(0xcbf29ce484222325, |hash, cell| {
        let hash = (hash ^ u64::from(cell.kind.id())).wrapping_mul(0x100000001b3);
        (hash ^ cell.extra as u64).wrapping_mul(0x100000001b3)
    })
}
fn percentile(samples: &[f64], fraction: f64) -> f64 {
    samples[((samples.len() as f64 * fraction).ceil() as usize)
        .saturating_sub(1)
        .min(samples.len() - 1)]
}
fn run(scene: &str, layer: &str, w: usize, h: usize, trials: usize, ticks: usize) -> Case {
    let initial = scenes::build(scene, w, h);
    let grid_bytes = initial.buffer_bytes();
    let mut samples = Vec::with_capacity(trials * ticks);
    let mut active_sum = 0;
    let mut max_active = 0;
    let mut final_checksum = None;
    for trial in 0..=trials {
        let mut runner = Runner::build(layer, initial.clone());
        for _ in 0..ticks {
            let start = Instant::now();
            runner.step();
            let elapsed = start.elapsed();
            if trial > 0 {
                samples.push(elapsed.as_secs_f64() * 1000.0);
                let active = runner.grid().dirty_chunk_count();
                active_sum += active;
                max_active = max_active.max(active);
            }
        }
        let digest = checksum(runner.grid());
        assert!(
            final_checksum.is_none_or(|last| last == digest),
            "non-deterministic trial"
        );
        final_checksum = Some(digest);
    }
    samples.sort_by(f64::total_cmp);
    let p95 = percentile(&samples, 0.95);
    Case {
        scene: scene.into(),
        layer: layer.into(),
        width: w,
        height: h,
        seed: scenes::SEED,
        trials,
        ticks_per_trial: ticks,
        mean_tick_ms: samples.iter().sum::<f64>() / samples.len() as f64,
        p50_tick_ms: percentile(&samples, 0.5),
        p95_tick_ms: p95,
        max_tick_ms: *samples.last().unwrap(),
        mean_active_chunks: active_sum as f64 / samples.len() as f64,
        max_active_chunks: max_active,
        grid_buffer_bytes: grid_bytes,
        snapshot_cell_bytes: if layer == "kernel" {
            0
        } else {
            w * h * std::mem::size_of::<charon_physics::sim::material::CellData>()
        },
        state_checksum: format!("{:016x}", final_checksum.unwrap()),
        within_60hz_half_budget: p95 <= 1000.0 / 60.0 * 0.5,
        within_30hz_half_budget: p95 <= 1000.0 / 30.0 * 0.5,
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|i| args.get(i + 1))
    };
    if args.iter().any(|arg| arg == "--repose") {
        let output = value("--output").ok_or("--output path is required")?;
        let mut measurements = Vec::new();
        for ground in [
            repose_measurement::Ground::Flat,
            repose_measurement::Ground::Slope,
            repose_measurement::Ground::Step,
        ] {
            for target in [32.0, 40.0, 45.0, 60.0] {
                for seed in [42, 123] {
                    let m = repose_measurement::measure(target, ground, seed);
                    measurements.push(serde_json::json!({"ground":format!("{ground:?}"),"seed":seed,
                    "target":m.target,"left_degrees":m.left_degrees,"right_degrees":m.right_degrees,
                    "ticks":m.ticks,"sand_before":m.sand_before,"sand_after":m.sand_after,"settled":m.settled}));
                }
            }
        }
        std::fs::write(
            output,
            serde_json::to_string_pretty(
                &serde_json::json!({"schema":1,"measurements":measurements}),
            )? + "\n",
        )?;
        return Ok(());
    }
    let trials = value("--trials")
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(12usize);
    let ticks = value("--ticks")
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(16usize);
    if trials == 0 || ticks == 0 {
        return Err("trials and ticks must be positive".into());
    }
    let output = value("--output").ok_or("--output path is required")?;
    let mut samples = Vec::new();
    for (w, h) in scenes::SIZES {
        for scene in scenes::SCENES {
            for layer in ["kernel", "simulation_snapshot", "plugin_snapshot"] {
                let case = run(scene, layer, w, h, trials, ticks);
                eprintln!(
                    "{w}x{h} {scene:15} {layer:20} p95 {:.4} ms",
                    case.p95_tick_ms
                );
                samples.push(case);
            }
        }
    }
    // Confirm all three paths reach identical states per scene, outside timing.
    for group in samples.as_chunks::<3>().0 {
        assert!(
            group
                .iter()
                .all(|case| case.state_checksum == group[0].state_checksum)
        );
    }
    let peak = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse().ok())
        });
    let report = Report {
        schema: 1,
        rust_package: env!("CARGO_PKG_VERSION").into(),
        samples,
        process_peak_rss_kib: peak,
    };
    std::fs::write(output, serde_json::to_string_pretty(&report)? + "\n")?;
    Ok(())
}
