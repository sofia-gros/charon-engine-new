use charon_bridge::*;
use charon_physics::sim::{
    grid::Grid,
    material::{Cell, CellData},
    simulation::Simulation,
};
use charon_tilemap::*;
fn setup() -> Bridge {
    let mut map = Tilemap::new(8, 8, [4, 4]).unwrap();
    for name in ["sand", "water"] {
        map.register(TileDefinition {
            name: name.into(),
            solid: true,
            atlas_index: 0,
            tint: [1.; 4],
        })
        .unwrap();
    }
    map.set(3, 2, Some(TileId(0))).unwrap();
    map.set(5, 2, Some(TileId(1))).unwrap();
    let mut bridge = Bridge::new(map, Simulation::new(32, 32, 7)).unwrap();
    bridge
        .set_conversion(TileId(0), Some(CellData::sand(0)))
        .unwrap();
    bridge
        .set_conversion(TileId(1), Some(CellData::water(0)))
        .unwrap();
    bridge
}
fn cmd(id: u64, edit: Edit) -> Command {
    Command {
        id,
        expected_revision: None,
        edit,
    }
}
fn count(bridge: &Bridge, kind: Cell) -> usize {
    bridge
        .sand()
        .grid()
        .cells
        .iter()
        .filter(|c| c.kind == kind)
        .count()
}
#[test]
fn release_is_exactly_once_and_conserves_mass() {
    let mut b = setup();
    let release = cmd(1, Edit::Break { x: 3, y: 2 });
    assert_eq!(b.apply(release.clone()).changed_cells, 16);
    assert_eq!(b.apply(release).result, ResultKind::Duplicate);
    assert_eq!(b.apply(cmd(2, Edit::Break { x: 3, y: 2 })).changed_cells, 0);
    assert_eq!(count(&b, Cell::Sand), 16);
    b.apply(cmd(3, Edit::Break { x: 5, y: 2 }));
    assert_eq!(count(&b, Cell::Water), 16);
    for _ in 0..300 {
        b.step();
    }
    assert_eq!(count(&b, Cell::Sand), 16);
    assert_eq!(count(&b, Cell::Water), 16);
    assert!(b.sand().snapshot().colliders.is_empty());
}
#[test]
fn occupied_placement_and_stale_edit_change_neither_layer() {
    let mut b = setup();
    b.apply(cmd(1, Edit::Break { x: 3, y: 2 }));
    let before = b.snapshot();
    let r = b.apply(cmd(
        2,
        Edit::Place {
            x: 3,
            y: 2,
            tile: TileId(0),
        },
    ));
    assert_eq!(r.result, ResultKind::Occupied);
    assert!(b.tiles().get(3, 2).is_none());
    assert_eq!(before.sand, b.sand().snapshot());
    let r = b.apply(Command {
        id: 3,
        expected_revision: Some(0),
        edit: Edit::Break { x: 5, y: 2 },
    });
    assert_eq!(r.result, ResultKind::Stale);
    assert!(b.tiles().get(5, 2).is_some());
}
#[test]
fn tile_blocks_physics_and_reports_contact_and_occupancy() {
    let mut b = setup();
    b.apply(cmd(
        1,
        Edit::Paint {
            x: 13,
            y: 7,
            radius: 0,
            cell: CellData::sand(0),
        },
    ));
    assert!(b.sand().contacts().any(|c| c.collider.1 == 8));
    for _ in 0..30 {
        b.step();
    }
    assert_eq!(count(&b, Cell::Sand), 1);
    assert!(
        b.occupancy(3, 2)
            .unwrap()
            .iter()
            .all(|c| c.kind == Cell::Air)
    );
    b.apply(cmd(2, Edit::Break { x: 3, y: 2 }));
    assert_eq!(count(&b, Cell::Sand), 17);
}
#[test]
fn game_rules_and_invalid_edits_cannot_delete_particles() {
    let mut b = setup();
    b.set_placement_rule(|_, _| false);
    assert_eq!(
        b.apply(cmd(
            1,
            Edit::Place {
                x: 1,
                y: 1,
                tile: TileId(0)
            }
        ))
        .result,
        ResultKind::PlacementDenied
    );
    assert_eq!(
        b.apply(cmd(
            2,
            Edit::Place {
                x: 99,
                y: 0,
                tile: TileId(0)
            }
        ))
        .result,
        ResultKind::Invalid
    );
    assert_eq!(
        b.apply(cmd(
            3,
            Edit::Place {
                x: 1,
                y: 1,
                tile: TileId(600)
            }
        ))
        .result,
        ResultKind::Invalid
    );
    b.set_conversion(TileId(0), None).unwrap();
    b.apply(cmd(4, Edit::Break { x: 3, y: 2 }));
    assert_eq!(count(&b, Cell::Sand), 0);
    assert!(b.set_conversion(TileId(1), Some(CellData::AIR)).is_err());
}
#[test]
fn dimensions_and_initial_occupancy_are_validated() {
    let mut map = Tilemap::new(2, 2, [4, 4]).unwrap();
    let id = map
        .register(TileDefinition {
            name: "solid".into(),
            solid: true,
            atlas_index: 0,
            tint: [1.; 4],
        })
        .unwrap();
    map.set(0, 0, Some(id)).unwrap();
    assert!(Bridge::new(map.clone(), Simulation::new(9, 8, 0)).is_err());
    let mut grid = Grid::new(8, 8);
    grid.set(1, 1, CellData::sand(0));
    assert!(Bridge::new(map, Simulation::from_grid(grid, 0)).is_err());
}
#[test]
fn compute_plugin_backpressure_preserves_requests_and_shutdown() {
    use charon_core::App;
    let (plugin, client) = BridgePlugin::new(setup(), 1).unwrap();
    let mut app = App::new();
    app.add_plugin(plugin).unwrap();
    app.start().unwrap();
    client.try_send(cmd(1, Edit::Break { x: 3, y: 2 })).unwrap();
    app.tick().unwrap();
    client.try_send(cmd(2, Edit::Break { x: 5, y: 2 })).unwrap();
    app.tick().unwrap();
    assert_eq!(client.try_receipt().unwrap().id, 1);
    app.tick().unwrap();
    assert_eq!(client.try_receipt().unwrap().id, 2);
    assert_eq!(
        client
            .snapshot()
            .sand
            .cells
            .iter()
            .filter(|c| c.kind == Cell::Sand)
            .count(),
        16
    );
    app.shutdown().unwrap();
    assert!(client.try_send(cmd(3, Edit::Break { x: 1, y: 1 })).is_err());
}
