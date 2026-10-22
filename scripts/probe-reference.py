#!/usr/bin/env python3
"""Demonstrate competing moves in the pinned rust-sandbox rule, without Bevy."""

import argparse
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path,
                        default=Path("/workspace/library-files/charon-references"))
    args = parser.parse_args()
    # Verify inputs before extracting code. No reference file is modified.
    subprocess.run(["python3", str(Path(__file__).with_name("references.py")),
                    "--root", str(args.root)], check=True)
    source = args.root / "rust-sandbox/rust/src"
    grid = (source / "grid.rs").read_text()
    grid_types = grid[grid.index("pub struct Grid {"):grid.index("/*", grid.index("pub struct Grid {"))]
    grid_types = grid_types.replace("#[derive(Component)]", "")
    blood = (source / "blood.rs").read_text().split("\n/*", 1)[0]
    blood = blood.replace("use bevy::prelude::*;", "use crate::bevy::prelude::*;")
    constants = (source / "utils/constants.rs").read_text()
    boundaries = (source / "utils/boundaries.rs").read_text()
    # Bevy math/component stand-ins only; movement and grid methods are extracted
    # unchanged. This is an isolated rule probe, not a run of the complete game.
    harness = r'''
#![allow(dead_code)]
mod bevy { pub mod prelude {
    #[derive(Default)] pub struct Position { pub x: f32, pub y: f32 }
    #[derive(Default)] pub struct Transform { pub translation: Position }
} }
mod grain {
    #[derive(Clone, Copy)] pub enum GrainType { Bone, Blood }
    pub enum Direction { Left, Right }
}
'''
    harness += "mod utils { pub mod constants {" + constants + "} pub mod boundaries {" + boundaries + "} }\n"
    harness += "mod grid { use crate::grain::GrainType; use crate::utils::constants::*;" + grid_types + "}\n"
    harness += "mod blood {" + blood + "}\n"
    harness += r'''
fn main() {
    use grain::{Direction, GrainType};
    use grid::{Grid, GridPosition};
    let mut grid = Grid::new();
    for x in [2, 4] {
        grid.set(x, 1, GrainType::Blood);
        grid.set(x, 2, GrainType::Bone);
    }
    let position = |x| GridPosition { current_x: x, current_y: 1, prev_x: None, prev_y: None };
    let (mut a, mut b) = (position(2), position(4));
    let (mut ta, mut tb) = (bevy::prelude::Transform::default(), bevy::prelude::Transform::default());
    blood::handle_blood_grain(&mut ta, &mut a, &grid, &mut Direction::Right, 10);
    blood::handle_blood_grain(&mut tb, &mut b, &grid, &mut Direction::Left, 10);
    assert_eq!((a.current_x, a.current_y), (3, 1));
    assert_eq!((b.current_x, b.current_y), (3, 1));
    println!("Observed: both grains selected (3, 1); this rule has no destination reservation.");
    println!("Isolated source-rule probe only; full Bevy runtime and FPS were not tested.");
}
'''
    with tempfile.TemporaryDirectory(prefix="charon-rule-probe-") as temporary:
        root = Path(temporary)
        rust = root / "probe.rs"
        binary = root / "probe"
        rust.write_text(harness)
        subprocess.run(["rustc", "--edition=2021", str(rust), "-o", str(binary)], check=True)
        subprocess.run([str(binary)], check=True)


if __name__ == "__main__":
    main()
