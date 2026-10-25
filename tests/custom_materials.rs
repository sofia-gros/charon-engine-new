use charon_physics::sim::{
    grid::Grid,
    material::{Cell, CellData},
    physics::{self, ParticleContext},
    rules::{MaterialDefinition, MaterialRegistry, Movement},
    simulation::{Operation, Outcome, Request, Simulation},
};
use rand::{SeedableRng, rngs::StdRng};

#[test]
fn register_cloud_nitrogen_and_custom_granular_without_enum_changes() {
    let mut registry = MaterialRegistry::default();
    let cloud = registry
        .register(MaterialDefinition::new(
            "Cloud",
            Movement::Gas,
            0.7,
            [210, 220, 230, 255],
        ))
        .unwrap();
    let nitrogen = registry
        .register(MaterialDefinition::new(
            "Nitrogen",
            Movement::Gas,
            1.165,
            [120, 170, 220, 255],
        ))
        .unwrap();
    let soil = registry
        .register(
            MaterialDefinition::new("Soil", Movement::Granular, 1600.0, [120, 80, 40, 255])
                .with_repose(38.0)
                .unwrap(),
        )
        .unwrap();
    assert_ne!(cloud, nitrogen);
    assert_eq!(registry.name(cloud), "Cloud");
    assert_eq!(registry.find("Soil"), Some(soil));
    registry.set_repose(soil, 60.0).unwrap();
    assert_eq!(registry.get(soil).repose_degrees(), 60.0);
    registry.set_repose(soil, 0.0).unwrap();
    let mut g = Grid::new(32, 32);
    g.set_materials(registry);
    g.set(8, 25, CellData::new(cloud, 0));
    g.set(20, 25, CellData::new(nitrogen, 0));
    g.set(16, 2, CellData::new(soil, 0));
    let mut rng = StdRng::seed_from_u64(42);
    for _ in 0..12 {
        physics::step(&mut g, &mut rng);
    }
    for kind in [cloud, nitrogen] {
        let index = g.cells.iter().position(|cell| cell.kind == kind).unwrap();
        assert!(index / 32 < 25, "lighter-than-air gas should rise");
    }
    assert!(g.cells.iter().position(|cell| cell.kind == soil).unwrap() / 32 > 2);
    assert_eq!(
        g.materials.color(CellData::new(cloud, 0)),
        [210, 220, 230, 255]
    );
    assert_eq!(std::mem::size_of::<CellData>(), 2);
}

fn condense(context: &mut ParticleContext<'_>, _rng: &mut dyn rand::Rng) {
    if context.cell().extra == 0 {
        assert!(context.replace(CellData::water(0)));
    } else {
        let cell = context.cell();
        context.replace(CellData::new(cell.kind, cell.extra - 1));
    }
}
#[test]
fn custom_callback_can_implement_a_lifetime_and_phase_change() {
    let mut registry = MaterialRegistry::default();
    let custom = registry
        .register(
            MaterialDefinition::new("Condensing cloud", Movement::Gas, 0.7, [220, 220, 220, 255])
                .with_update(condense),
        )
        .unwrap();
    let mut g = Grid::new(16, 16);
    g.set_materials(registry);
    g.set(8, 8, CellData::new(custom, 2));
    let mut rng = StdRng::seed_from_u64(42);
    for _ in 0..3 {
        physics::step(&mut g, &mut rng);
    }
    assert_eq!(g.cells.iter().filter(|c| c.kind == Cell::Water).count(), 1);
    assert!(!g.cells.iter().any(|c| c.kind == custom));
}

#[test]
fn duplicate_names_invalid_density_and_capacity_are_explicit_errors() {
    let mut registry = MaterialRegistry::default();
    assert!(
        registry
            .register(MaterialDefinition::new("Water", Movement::Gas, 1.0, [0; 4]))
            .is_err()
    );
    assert!(
        registry
            .register(MaterialDefinition::new("", Movement::Gas, 1.0, [0; 4]))
            .is_err()
    );
    assert!(
        registry
            .register(MaterialDefinition::new(
                "bad",
                Movement::Gas,
                f32::NAN,
                [0; 4]
            ))
            .is_err()
    );
    for i in 10..256 {
        registry
            .register(MaterialDefinition::new(
                format!("m{i}"),
                Movement::Solid,
                1.0,
                [0; 4],
            ))
            .unwrap();
    }
    assert!(
        registry
            .register(MaterialDefinition::new(
                "overflow",
                Movement::Gas,
                1.0,
                [0; 4]
            ))
            .is_err()
    );
}
#[test]
fn simulation_refuses_foreign_unregistered_ids_and_changes_repose_transactionally() {
    let mut foreign = MaterialRegistry::default();
    let custom = foreign
        .register(MaterialDefinition::new(
            "Foreign",
            Movement::Gas,
            0.7,
            [0; 4],
        ))
        .unwrap();
    let mut sim = Simulation::new(32, 32, 42);
    let result = sim.apply(Request {
        id: 1,
        expected_revision: None,
        operation: Operation::Brush {
            x: 3,
            y: 3,
            radius: 0,
            cell: CellData::new(custom, 0),
        },
    });
    assert_eq!(result.outcome, Outcome::InvalidSettings);
    let result = sim.apply(Request {
        id: 2,
        expected_revision: None,
        operation: Operation::SetRepose {
            kind: Cell::Sand,
            degrees: 60.0,
        },
    });
    assert_eq!(result.outcome, Outcome::Applied);
    assert_eq!(sim.snapshot().sand_repose_degrees, 60.0);
    let result = sim.apply(Request {
        id: 3,
        expected_revision: None,
        operation: Operation::SetRepose {
            kind: Cell::Sand,
            degrees: 32.5,
        },
    });
    assert_eq!(result.outcome, Outcome::Applied);
    assert_eq!(sim.snapshot().sand_repose_degrees, 32.5);
    let result = sim.apply(Request {
        id: 4,
        expected_revision: None,
        operation: Operation::SetRepose {
            kind: Cell::Sand,
            degrees: f32::NAN,
        },
    });
    assert_eq!(result.outcome, Outcome::InvalidSettings);
    assert_eq!(sim.snapshot().sand_repose_degrees, 32.5);
}

static CALLBACK_TICKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn delayed_change(context: &mut ParticleContext<'_>, _rng: &mut dyn rand::Rng) {
    if CALLBACK_TICKS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 2 {
        context.replace(CellData::water(0));
    }
}
#[test]
fn custom_callbacks_keep_ticking_without_having_to_modify_color_state() {
    CALLBACK_TICKS.store(0, std::sync::atomic::Ordering::SeqCst);
    let mut registry = MaterialRegistry::default();
    let kind = registry
        .register(
            MaterialDefinition::new("Timer", Movement::Solid, 10.0, [0; 4])
                .with_update(delayed_change),
        )
        .unwrap();
    let mut grid = Grid::new(8, 8);
    grid.set_materials(registry);
    grid.set(3, 3, CellData::new(kind, 0));
    let mut rng = StdRng::seed_from_u64(42);
    for _ in 0..3 {
        physics::step(&mut grid, &mut rng);
    }
    assert_eq!(CALLBACK_TICKS.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert_eq!(grid.get(3, 3).kind, Cell::Water);
}

fn collision_probe(context: &mut ParticleContext<'_>, _rng: &mut dyn rand::Rng) {
    assert_eq!(context.neighbor(-1, 0).unwrap().kind, Cell::Block);
    assert!(!context.try_move(-1, 0));
    assert!(!context.transform_neighbor(-1, 0, CellData::AIR));
    context.replace(CellData::sand(0));
}
#[test]
fn custom_reaction_cannot_erase_or_cross_external_solids() {
    let mut registry = MaterialRegistry::default();
    let kind = registry
        .register(
            MaterialDefinition::new("Probe", Movement::Gas, 0.7, [0; 4])
                .with_update(collision_probe),
        )
        .unwrap();
    let mut sim = Simulation::new(8, 8, 42).with_materials(registry);
    sim.apply(Request {
        id: 1,
        expected_revision: None,
        operation: Operation::Collider {
            rect: charon_physics::sim::simulation::Rect {
                x: 2,
                y: 3,
                w: 1,
                h: 1,
            },
            solid: true,
        },
    });
    sim.apply(Request {
        id: 2,
        expected_revision: None,
        operation: Operation::Brush {
            x: 3,
            y: 3,
            radius: 0,
            cell: CellData::new(kind, 0),
        },
    });
    sim.step();
    assert!(sim.is_solid(2, 3));
    assert_eq!(sim.grid().get(2, 3).kind, Cell::Air);
    assert_eq!(sim.grid().get(3, 3).kind, Cell::Sand);
}

fn fixed_cloud(context: &mut ParticleContext<'_>, _rng: &mut dyn rand::Rng) {
    context.sleep();
}
#[test]
fn gas_above_a_pile_does_not_hide_its_granular_surface() {
    let mut registry = MaterialRegistry::default();
    let cloud = registry
        .register(
            MaterialDefinition::new("Fixed cloud", Movement::Gas, 0.7, [200; 4])
                .with_update(fixed_cloud),
        )
        .unwrap();
    let mut g = Grid::new(65, 65);
    g.set_materials(registry);
    for x in 0..65 {
        g.set(x, 64, CellData::stone(0));
        g.set(x, 0, CellData::new(cloud, 0));
        let height = (16 - (x as i32 - 32).abs()).max(0) as usize;
        for y in 64 - height..64 {
            g.set(x, y, CellData::sand(0));
        }
    }
    let before = g.cells.iter().filter(|c| c.kind == Cell::Sand).count();
    let mut rng = StdRng::seed_from_u64(42);
    for _ in 0..600 {
        physics::step(&mut g, &mut rng);
    }
    let height = (0..65)
        .map(|x| (0..64).filter(|&y| g.get(x, y).kind == Cell::Sand).count())
        .max()
        .unwrap();
    assert!(
        height < 15,
        "32-degree relaxation must still occur underneath gas"
    );
    assert_eq!(
        g.cells.iter().filter(|c| c.kind == Cell::Sand).count(),
        before
    );
}

#[test]
fn registered_liquid_uses_its_own_id_for_lateral_spreading() {
    let mut registry = MaterialRegistry::default();
    let fluid = registry
        .register(MaterialDefinition::new(
            "Custom water",
            Movement::Liquid,
            1000.0,
            [0; 4],
        ))
        .unwrap();
    registry.set_spread(fluid, 5).unwrap();
    let mut a = Grid::new(32, 32);
    let mut b = Grid::new(32, 32);
    b.set_materials(registry);
    for (g, kind) in [(&mut a, Cell::Water), (&mut b, fluid)] {
        for x in 0..32 {
            g.set(x, 31, CellData::stone(0));
        }
        for y in 10..31 {
            for x in 2..12 {
                g.set(x, y, CellData::new(kind, 0));
            }
        }
    }
    let mut ar = StdRng::seed_from_u64(42);
    let mut br = StdRng::seed_from_u64(42);
    for _ in 0..100 {
        physics::step(&mut a, &mut ar);
        physics::step(&mut b, &mut br);
        for (left, right) in a.cells.iter().zip(&b.cells) {
            assert_eq!(
                left.kind,
                if right.kind == fluid {
                    Cell::Water
                } else {
                    right.kind
                }
            );
            assert_eq!(left.extra, right.extra);
        }
    }
}
