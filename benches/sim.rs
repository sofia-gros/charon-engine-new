//! Criterion micro-benchmarks for the physics step.
//!
//! `cargo bench` produces `target/criterion/<name>/report/index.html`.
//! Numbers below are illustrative; what you get will depend on your CPU.
//!
//! On a 640×360 grid (≈230k cells) we measure cells-per-second by running
//! `physics::step` and dividing by the elapsed time. The chunked dirty-rect
//! scheduler (see `grid.rs`) means that wall-clock cost scales with the
//! number of active chunks, but each tick still copies the full cell buffer.

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use rand::SeedableRng;
use rand::rngs::StdRng;
use sandengine::sim::grid::Grid;
use sandengine::sim::material::CellData;
use sandengine::sim::physics;

fn bench_saturated(c: &mut Criterion) {
    let mut group = c.benchmark_group("physics_step_640x360");
    group.throughput(Throughput::Elements(640 * 360));
    group.bench_function("falling_sand", |bencher| {
        // Reset outside the timed step so sand never settles during sampling.
        bencher.iter_batched_ref(
            || {
                let mut grid = Grid::new(640, 360);
                for y in 0..120 {
                    for x in 160..480 {
                        grid.set(x, y, CellData::sand(0));
                    }
                }
                (grid, StdRng::seed_from_u64(1))
            },
            |(grid, rng)| physics::step(grid, rng),
            BatchSize::LargeInput,
        );
    });
    group.finish();
}

fn bench_settled(c: &mut Criterion) {
    let mut g = Grid::new(640, 360);
    let mut rng = StdRng::seed_from_u64(2);
    // Fill with stone — completely inert, so dirty chunks should drop to 0.
    for y in 0..g.h {
        for x in 0..g.w {
            g.set(x, y, CellData::stone(0));
        }
    }
    // Warm up a few ticks so the dirty propagation settles.
    for _ in 0..8 {
        physics::step(&mut g, &mut rng);
    }

    let mut group = c.benchmark_group("physics_step_640x360");
    group.throughput(Throughput::Elements((g.w * g.h) as u64));
    group.bench_function("settled_stone", |bencher| {
        bencher.iter(|| {
            physics::step(&mut g, &mut rng);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_saturated, bench_settled);
criterion_main!(benches);
