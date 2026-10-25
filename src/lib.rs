//! `charon_physics` — a 2D falling-sand cellular automaton.
//!
//! The crate is structured so that the simulation core ([`sim`]) is
//! independent of the windowing / GPU stack. You can use it headlessly:
//!
//! ```
//! use charon_physics::sim::grid::Grid;
//! use charon_physics::sim::material::CellData;
//! use charon_physics::sim::physics;
//! use rand::SeedableRng;
//!
//! let mut grid = Grid::new(64, 64);
//! grid.set(32, 0, CellData::sand(0));
//! let mut rng = rand::rngs::StdRng::seed_from_u64(0);
//! for _ in 0..30 { physics::step(&mut grid, &mut rng); }
//! ```
//!
//! Rendering, input, replay storage, and frame timing belong to the host application.

pub mod sim;
