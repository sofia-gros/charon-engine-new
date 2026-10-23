//! Game content belongs here; the sand plugin supplies reusable motion rules.
use charon_sand::{Material, Materials, Motion};

pub const AIR: u8 = 0;
pub const SAND: u8 = 1;
pub const WALL: u8 = 2;
pub const ASH: u8 = 3;
pub const WATER: u8 = 4;
pub const OIL: u8 = 5;

pub fn legacy_palette() -> Materials {
    Materials::new(
        vec![
            Material {
                key: "air".into(),
                color: [16, 23, 36, 255],
                motion: Motion::Empty,
                density: 0,
                blocks_actors: false,
                blocks_projectiles: false,
            },
            Material {
                key: "sand".into(),
                color: [232, 181, 72, 255],
                motion: Motion::Powder,
                density: 1600,
                blocks_actors: true,
                blocks_projectiles: true,
            },
            Material {
                key: "wall".into(),
                color: [82, 100, 121, 255],
                motion: Motion::Static,
                density: u16::MAX,
                blocks_actors: true,
                blocks_projectiles: true,
            },
            // A second powder that falls and accumulates, but actors can walk through.
            // Projectile blocking is a separate policy, exercised by the next milestone.
            Material {
                key: "demo.ash".into(),
                color: [177, 133, 204, 255],
                motion: Motion::Powder,
                density: 1600,
                blocks_actors: false,
                blocks_projectiles: true,
            },
        ],
        AIR,
        WALL,
    )
    .expect("valid game material definitions")
}

/// Current game palette. Keep the old definitions for explicit v1/v2 replay.
pub fn palette() -> Materials {
    let old = legacy_palette();
    let mut entries: Vec<_> = (0..4).map(|id| old.get(id).unwrap().clone()).collect();
    for (key, color, density) in [
        ("demo.water", [35, 139, 224, 255], 1000),
        ("demo.oil", [201, 179, 61, 255], 800),
    ] {
        entries.push(Material {
            key: key.into(),
            color,
            motion: Motion::Liquid,
            density,
            blocks_actors: false,
            blocks_projectiles: false,
        });
    }
    Materials::new(entries, AIR, WALL).expect("valid game fluids")
}
