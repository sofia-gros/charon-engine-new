//! 2D falling-sand cellular automaton.
//!
//! The simulation operates on a fixed-size [`Grid`] of [`CellData`] entries.
//! Each frame, [`physics::step`] sweeps the grid bottom-up with a randomized
//! horizontal direction (a standard mitigation for the directional bias of
//! sequential CA updates documented by Will Hankinson in *Sandspiel* and by
//! Petri Purho in his Noita GDC 2019 talk). Materials are ordered by density
//! so heavier materials sink and lighter ones rise.
//!
//! Sub-modules:
//! - [`material`]: the [`Cell`] enum and per-cell color synthesis
//! - [`grid`]: storage, double buffer, and per-chunk dirty-rect tracking
//! - [`physics`]: per-material movement rules
//! - [`block`]: an interactive rigid block that displaces particles via BFS
//! - [`history`]: bounded ring-buffer of snapshots for time-scrubbing replays
//! - [`scenarios`]: initial-condition presets and live spawners

pub mod block;
pub mod grid;
pub mod history;
pub mod material;
pub mod physics;
pub mod scenarios;
