//! Criterion micro-benchmarks for the physics step.
//!
//! `cargo bench` produces `target/criterion/<name>/report/index.html`.
//! Numbers below are illustrative; what you get will depend on your CPU.
//!
//! On a 1280×720 grid (≈920k cells) we measure cells-per-second by running
//! `physics::step` and dividing by the elapsed time. The chunked dirty-rect
//! scheduler (see `grid.rs`) means that wall-clock cost scales with the
//! number of *active* chunks, not the total grid area, so the "saturated"
//! and "settled" benchmarks differ by ~10×.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use rand::SeedableRng;
use rand::rngs::StdRng;
use sandengine::sim::block::Block;
use sandengine::sim::grid::Grid;
use sandengine::sim::material::CellData;
use sandengine::sim::{physics, scenarios};

fn bench_saturated(c: &mut Criterion) {
    let mut g = Grid::new(640, 360);
    let mut b = Block::new(0.0, 0.0, 1.0, 1.0);
    let mut rng = StdRng::seed_from_u64(1);
    let _ = (&mut b,);
    // Falling-circle scenario: lots of moving sand.
    scenarios::apply(1, &mut g, &mut b, &mut rng);

    let mut group = c.benchmark_group("physics_step_640x360");
    group.throughput(Throughput::Elements((g.w * g.h) as u64));
    group.bench_function("falling_circle", |bencher| {
        bencher.iter(|| {
            physics::step(&mut g, &mut rng);
        });
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
