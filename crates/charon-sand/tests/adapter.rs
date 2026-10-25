use charon_core::App;
use charon_sand::{FallingSandPlugin, Operation, Outcome, Rect, Request, SandState, Simulation};
use sandengine::sim::material::{Cell, CellData};

fn request(id: u64, operation: Operation) -> Request {
    Request {
        id,
        expected_revision: None,
        operation,
    }
}
fn brush(x: i32, y: i32, cell: CellData) -> Operation {
    Operation::Brush {
        x,
        y,
        radius: 0,
        cell,
    }
}

#[test]
fn direct_and_plugin_simulation_produce_identical_snapshots() {
    let mut direct = Simulation::new(65, 65, 42);
    let (plugin, client) = FallingSandPlugin::new(Simulation::new(65, 65, 42), 8).unwrap();
    let mut app = App::new();
    app.add_plugin(plugin).unwrap();
    app.start().unwrap();
    let commands = [
        Operation::Collider {
            rect: Rect {
                x: 0,
                y: 45,
                w: 65,
                h: 1,
            },
            solid: true,
        },
        brush(31, 1, CellData::sand(7)),
        brush(33, 2, CellData::water(3)),
    ];
    for (i, op) in commands.into_iter().enumerate() {
        let cmd = request(i as u64, op);
        direct.apply(cmd.clone());
        client.try_send(cmd).unwrap();
    }
    for _ in 0..160 {
        direct.step();
        app.tick().unwrap();
    }
    assert_eq!(*client.snapshot(), direct.snapshot());
    assert_eq!(
        client
            .snapshot()
            .cells
            .iter()
            .filter(|c| c.kind == Cell::Sand)
            .count(),
        1
    );
    app.shutdown().unwrap();
    assert!(matches!(
        client.try_send(request(99, Operation::Reset)),
        Err(std::sync::mpsc::TrySendError::Disconnected(_))
    ));
}

#[test]
fn external_solids_block_particles_and_removal_wakes_sleeping_chunks() {
    let mut sim = Simulation::new(65, 65, 1);
    sim.apply(request(
        1,
        Operation::Collider {
            rect: Rect {
                x: 0,
                y: 32,
                w: 65,
                h: 1,
            },
            solid: true,
        },
    ));
    sim.apply(request(
        2,
        Operation::Brush {
            x: 32,
            y: 20,
            radius: 8,
            cell: CellData::sand(0),
        },
    ));
    for _ in 0..200 {
        sim.step();
    }
    assert!(
        sim.grid().cells[33 * 65..]
            .iter()
            .all(|c| c.kind == Cell::Air)
    );
    let count = sim.grid().particle_count();
    assert!(count > 0);
    assert!(sim.contacts().any(|c| c.material == Cell::Sand));
    assert!(
        sim.snapshot().cells[32 * 65..33 * 65]
            .iter()
            .all(|c| c.kind == Cell::Air)
    );
    sim.apply(request(
        3,
        Operation::Collider {
            rect: Rect {
                x: 0,
                y: 32,
                w: 65,
                h: 1,
            },
            solid: false,
        },
    ));
    for _ in 0..160 {
        sim.step();
    }
    assert_eq!(sim.grid().particle_count(), count);
    assert!(
        sim.grid().cells[33 * 65..]
            .iter()
            .any(|c| c.kind == Cell::Sand)
    );
}

#[test]
fn colliders_reject_occupied_regions_atomically_and_release_is_idempotent() {
    let mut sim = Simulation::new(16, 16, 0);
    sim.apply(request(1, brush(2, 2, CellData::sand(0))));
    let fail = sim.apply(request(
        2,
        Operation::Collider {
            rect: Rect {
                x: 0,
                y: 2,
                w: 8,
                h: 1,
            },
            solid: true,
        },
    ));
    assert_eq!(fail.outcome, Outcome::Occupied);
    assert!(sim.snapshot().colliders.is_empty());
    let rect = Rect {
        x: 4,
        y: 4,
        w: 4,
        h: 2,
    };
    sim.apply(request(3, Operation::Collider { rect, solid: true }));
    let cmd = request(
        4,
        Operation::Release {
            rect,
            cell: CellData::water(0),
        },
    );
    assert_eq!(sim.apply(cmd.clone()).changed, 8);
    assert_eq!(sim.apply(cmd).outcome, Outcome::DuplicateOrOutOfOrder);
    assert_eq!(
        sim.grid()
            .cells
            .iter()
            .filter(|c| c.kind == Cell::Water)
            .count(),
        8
    );
    assert!(sim.snapshot().colliders.is_empty());
}

#[test]
fn stale_revision_brush_bounds_pause_and_reset() {
    let mut sim = Simulation::new(8, 8, 7);
    let stale = Request {
        id: 1,
        expected_revision: Some(42),
        operation: brush(1, 1, CellData::water(0)),
    };
    assert_eq!(sim.apply(stale).outcome, Outcome::StaleRevision);
    assert_eq!(sim.grid().particle_count(), 0);
    sim.apply(request(
        2,
        Operation::Brush {
            x: -1,
            y: 0,
            radius: 2,
            cell: CellData::sand(0),
        },
    ));
    assert!(sim.grid().particle_count() > 0);
    sim.apply(request(3, Operation::SetPaused(true)));
    let paused = sim.snapshot();
    sim.step();
    assert_eq!(paused, sim.snapshot());
    sim.apply(request(4, Operation::Reset));
    assert_eq!(sim.grid().particle_count(), 0);
    assert_eq!(sim.ticks(), 0);
    assert!(!sim.snapshot().paused);
    assert_eq!(
        sim.apply(request(3, Operation::Reset)).outcome,
        Outcome::DuplicateOrOutOfOrder
    );
    assert_eq!(std::mem::size_of::<CellData>(), 2);
}

#[test]
fn bounded_receipts_pause_commands_but_do_not_block_simulation() {
    let (plugin, client) = FallingSandPlugin::new(Simulation::new(8, 8, 0), 1).unwrap();
    let mut app = App::new();
    app.add_plugin(plugin).unwrap();
    app.start().unwrap();
    client
        .try_send(request(1, brush(1, 0, CellData::sand(0))))
        .unwrap();
    app.tick().unwrap(); // receipt 1 fills result queue
    client
        .try_send(request(2, brush(3, 0, CellData::water(0))))
        .unwrap();
    app.tick().unwrap(); // receipt 2 retained as pending
    client
        .try_send(request(3, Operation::SetPaused(true)))
        .unwrap();
    assert!(matches!(
        client.try_send(request(4, Operation::Reset)),
        Err(std::sync::mpsc::TrySendError::Full(_))
    ));
    app.tick().unwrap();
    assert_eq!(client.snapshot().ticks, 3);
    assert!(!client.snapshot().paused);
    assert_eq!(client.try_receipt().unwrap().id, 1);
    app.tick().unwrap(); // flush receipt2, process pause (receipt3 pending)
    assert!(client.snapshot().paused);
    assert_eq!(client.try_receipt().unwrap().id, 2);
    app.tick().unwrap();
    assert_eq!(client.try_receipt().unwrap().id, 3);
    app.shutdown().unwrap();
}

#[test]
fn failed_init_does_not_remove_resources_owned_by_the_host() {
    let mut app = App::new();
    app.world_mut().insert(SandState(Simulation::new(4, 4, 3)));
    let (plugin, _client) = FallingSandPlugin::new(Simulation::new(8, 8, 0), 1).unwrap();
    app.add_plugin(plugin).unwrap();
    assert!(app.start().is_err());
    assert_eq!(app.world().get::<SandState>().unwrap().0.grid().w, 4);
}

#[test]
fn contact_notifications_are_opt_in_and_bounded() {
    use charon_sand::{Contact, ContactOverflow};
    let mut sim = Simulation::new(16, 16, 42);
    sim.apply(request(
        1,
        Operation::Collider {
            rect: Rect {
                x: 0,
                y: 8,
                w: 16,
                h: 1,
            },
            solid: true,
        },
    ));
    sim.apply(request(
        2,
        Operation::Brush {
            x: 8,
            y: 7,
            radius: 3,
            cell: CellData::sand(0),
        },
    ));
    let (plugin, _client) = FallingSandPlugin::new(sim, 8).unwrap();
    let mut app = App::new();
    app.add_plugin(plugin.with_contact_events(2)).unwrap();
    app.start().unwrap();
    app.tick().unwrap();
    assert_eq!(app.world().events::<Contact>().len(), 2);
    assert_eq!(app.world().events::<ContactOverflow>().len(), 1);
}
