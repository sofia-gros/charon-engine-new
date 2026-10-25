//! `sandengine` — a 2D falling-sand cellular automaton.
//!
//! The crate is structured so that the simulation core ([`sim`]) is
//! independent of the windowing / GPU stack. You can use it headlessly:
//!
//! ```
//! use sandengine::sim::grid::Grid;
//! use sandengine::sim::material::CellData;
//! use sandengine::sim::physics;
//! use rand::SeedableRng;
//!
//! let mut grid = Grid::new(64, 64);
//! grid.set(32, 0, CellData::sand(0));
//! let mut rng = rand::rngs::StdRng::seed_from_u64(0);
//! for _ in 0..30 { physics::step(&mut grid, &mut rng); }
//! ```
//!
//! See the README for a survey of the literature and design references.

pub mod sim;
