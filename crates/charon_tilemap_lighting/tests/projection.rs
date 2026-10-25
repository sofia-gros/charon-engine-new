use charon_lighting::*;
use charon_tilemap::*;
use charon_tilemap_lighting::*;
use std::collections::HashMap;
#[test]
fn edits_property_changes_and_world_origin_invalidate_cached_optics() {
    let g = Geometry {
        width: 8,
        height: 8,
        origin: [8., 8.],
        cell_size: 4.,
    };
    let mut map = Tilemap::new(4, 4, [8, 8]).unwrap();
    let id = map
        .register(TileDefinition {
            name: "ore".into(),
            solid: true,
            atlas_index: 0,
            tint: [1.; 4],
        })
        .unwrap();
    map.set(1, 1, Some(id)).unwrap();
    let mut input = TilemapInput::new(g, [0., 0.]).unwrap();
    let mut properties = HashMap::new();
    let field = input.sync(&map, &[], &properties).unwrap();
    assert_eq!(field.cells()[0], Optical::OPAQUE);
    properties.insert(
        id,
        Optical {
            emission_color: [1.; 3],
            emission_strength: 1.,
            ..Optical::OPAQUE
        },
    );
    assert_eq!(
        input.sync(&map, &[], &properties).unwrap().cells()[0].emission_strength,
        1.
    );
    map.set(1, 1, None).unwrap();
    let edits: Vec<_> = map.drain_changes().collect();
    assert_eq!(
        input.sync(&map, &edits, &properties).unwrap().cells()[0],
        Optical::AIR
    );
}
#[test]
fn coarse_resolution_preserves_thin_opaque_coverage() {
    let g = Geometry {
        width: 2,
        height: 2,
        origin: [0., 0.],
        cell_size: 8.,
    };
    let mut map = Tilemap::new(16, 16, [1, 1]).unwrap();
    let id = map
        .register(TileDefinition {
            name: "wall".into(),
            solid: true,
            atlas_index: 0,
            tint: [1.; 4],
        })
        .unwrap();
    map.set(3, 3, Some(id)).unwrap();
    assert_eq!(
        rasterize(&map, g, [0., 0.], &HashMap::new())
            .unwrap()
            .cells()[0]
            .opacity,
        1.
    );
}
