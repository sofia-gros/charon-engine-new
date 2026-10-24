//! Game content belongs here; the sand plugin supplies reusable motion rules.
use charon_sand::{Material, Materials, Motion};

pub const AIR: u8 = 0;
pub const SAND: u8 = 1;
pub const WALL: u8 = 2;
pub const ASH: u8 = 3;
pub const WATER: u8 = 4;
pub const OIL: u8 = 5;
pub const RUBBLE: u8 = 6;
pub const BLOCK_ITEM: u8 = 7;
pub const DIRT: u8 = 8;
pub const STONE: u8 = 9;
pub const ORE: u8 = 10;
pub const DIRT_RUBBLE: u8 = 11;
pub const STONE_RUBBLE: u8 = 12;
pub const ORE_RUBBLE: u8 = 13;
pub fn mining_products(id: u8) -> Option<(u8, u8)> {
    match id {
        WALL => Some((RUBBLE, BLOCK_ITEM)),
        DIRT => Some((DIRT_RUBBLE, BLOCK_ITEM)),
        STONE => Some((STONE_RUBBLE, BLOCK_ITEM)),
        ORE => Some((ORE_RUBBLE, BLOCK_ITEM)),
        _ => None,
    }
}

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

/// Palette used by the recorded sea scene, before mining materials were added.
pub fn fluid_palette() -> Materials {
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

pub fn palette() -> Materials {
    let old = fluid_palette();
    let mut entries: Vec<_> = (0..6).map(|id| old.get(id).unwrap().clone()).collect();
    for (key, color, motion, actors) in [
        (
            "game.wall_rubble",
            [82, 100, 121, 255],
            Motion::Powder,
            true,
        ),
        (
            "game.block_item",
            [90, 240, 160, 255],
            Motion::Powder,
            false,
        ),
        ("game.dirt", [122, 83, 55, 255], Motion::Static, true),
        ("game.stone", [97, 103, 119, 255], Motion::Static, true),
        ("game.ore", [174, 121, 63, 255], Motion::Static, true),
        ("game.dirt_rubble", [122, 83, 55, 255], Motion::Powder, true),
        (
            "game.stone_rubble",
            [97, 103, 119, 255],
            Motion::Powder,
            true,
        ),
        ("game.ore_rubble", [174, 121, 63, 255], Motion::Powder, true),
    ] {
        entries.push(Material {
            key: key.into(),
            color,
            motion,
            density: if motion == Motion::Static {
                u16::MAX
            } else {
                2400
            },
            blocks_actors: actors,
            blocks_projectiles: actors,
        });
    }
    Materials::new(entries, AIR, WALL).expect("valid game fluids")
}
