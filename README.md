# Charon simulation core

A headless Rust library for a 2D falling-sand cellular automaton, derived from
[Sandengine](https://github.com/Ijtihed/sandengine). The crate remains named
`sandengine` so existing physics imports continue to work.

The library contains grid storage, 32×32 chunk activity tracking, materials,
movement and reactions, and a movable block that displaces particles. It has no
window, GPU, GUI, input handling, replay history, or frame loop. A host application
owns rendering, input, tick scheduling, and any snapshot storage.

## Usage

Requires a current stable Rust toolchain supporting edition 2024.

```toml
[dependencies]
sandengine = { git = "https://github.com/sofia-gros/charon-engine-new" }
rand = "0.10.1"
```

```rust
use rand::{SeedableRng, rngs::StdRng};
use sandengine::sim::{grid::Grid, material::CellData, physics};

let mut grid = Grid::new(365, 365);
let mut rng = StdRng::seed_from_u64(42);
grid.set(182, 10, CellData::sand(0));

for _ in 0..120 {
    physics::step(&mut grid, &mut rng);
}
```

Run a complete example without a display:

```sh
cargo run --release --example headless
```

## Core modules

| Module | Responsibility |
| --- | --- |
| `sim::grid` | Cells, double buffering, and active chunk tracking |
| `sim::material` | Two-byte `CellData`, material densities, and color helpers |
| `sim::physics` | Gravity, displacement, liquid/gas movement, and reactions |
| `sim::block` | Movable rectangular block and particle displacement |

Ten cell kinds are retained: air, sand, water, stone, fire, steam, gravel,
block, oil, and acid. Material color helpers return plain values and require no
rendering library. Production dependencies are `rand` and `glam`; Criterion is
used only for development benchmarks.

The original scenario generators live in `tests/support/scenarios.rs` as
test fixtures. They are excluded from the library API.

## Current limits and next steps

This extraction preserves the original simulation rules. Sand still forms slopes
near 45°; a natural angle of repose, per-cell velocity, temperature transport,
viscosity, and surface tension are not implemented. These require separate physics
changes and validation.

`physics::step` still copies the entire cell buffer each tick, even when chunks
are inactive. Each cell occupies two bytes, so a 365×365 grid uses about 520 KiB
for its two cell buffers, excluding scheduler data and allocation overhead. No
snapshot history is allocated by the core.

Future work can introduce configurable material behavior and separate arrays for
velocity or temperature while preserving compact cell storage. No performance
claims from the original graphical application have been revalidated here.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --release
cargo bench
```

CI runs formatting, Clippy, tests, and a release build on Linux, macOS, and
Windows without requiring graphics system packages.

## License

MIT. Original portions are Copyright (c) 2026 Ijtihed; modifications are
Copyright (c) 2026 sofia-gros. The original copyright and permission notice are
retained in [LICENSE](LICENSE).
