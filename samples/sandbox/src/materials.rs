//! Game content belongs here; the sand plugin supplies reusable motion rules.
use charon_sand::{Material, Materials, Motion};

pub const AIR: u8 = 0;
pub const SAND: u8 = 1;
pub const WALL: u8 = 2;
pub const ASH: u8 = 3;

pub fn palette() -> Materials {
    Materials::new(
        vec![
            Material {
                key: "air".into(),
                color: [16, 23, 36, 255],
                motion: Motion::Empty,
                blocks_actors: false,
                blocks_projectiles: false,
            },
            Material {
                key: "sand".into(),
                color: [232, 181, 72, 255],
                motion: Motion::Powder,
                blocks_actors: true,
                blocks_projectiles: true,
            },
            Material {
                key: "wall".into(),
                color: [82, 100, 121, 255],
                motion: Motion::Static,
                blocks_actors: true,
                blocks_projectiles: true,
            },
            // A second powder that falls and accumulates, but actors can walk through.
            // Projectile blocking is a separate policy, exercised by the next milestone.
            Material {
                key: "demo.ash".into(),
                color: [177, 133, 204, 255],
                motion: Motion::Powder,
                blocks_actors: false,
                blocks_projectiles: true,
            },
        ],
        AIR,
        WALL,
    )
    .expect("valid game material definitions")
}
