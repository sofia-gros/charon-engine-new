#[path = "support/repose_measurement.rs"]
mod measurement;
use charon_physics::sim::{
    grid::Grid,
    material::{Cell, CellData},
    physics,
    rules::MaterialTable,
};
use rand::{SeedableRng, rngs::StdRng};

#[test]
fn sand_repose_on_flat_sloped_and_stepped_ground_conserves_mass() {
    for ground in [
        measurement::Ground::Flat,
        measurement::Ground::Slope,
        measurement::Ground::Step,
    ] {
        for seed in [42, 123] {
            let m = measurement::measure(32.0, ground, seed);
            assert!(m.settled, "{ground:?} seed={seed} ticks={}", m.ticks);
            assert_eq!(m.sand_before, m.sand_after);
            for angle in [m.left_degrees, m.right_degrees] {
                assert!(
                    (angle - f64::from(m.target)).abs() <= 4.0,
                    "{ground:?} seed={seed} measured={angle}"
                );
            }
        }
    }
}

#[test]
fn steeper_friction_setting_changes_the_measured_slope() {
    let m = measurement::measure(45.0, measurement::Ground::Flat, 42);
    assert!(m.settled);
    assert!(m.left_degrees > 41.0 && m.right_degrees > 41.0);
    assert_eq!(m.sand_before, m.sand_after);
}

#[test]
fn density_and_spread_are_driven_by_material_table() {
    let mut g = Grid::new(24, 24);
    let mut rules = MaterialTable::default();
    rules.set_density(Cell::Oil, 1500.0).unwrap();
    rules.set_spread(Cell::Water, 0).unwrap();
    g.set_materials(rules);
    for y in 1..23 {
        for x in 0..24 {
            g.set(x, y, CellData::water(0));
        }
    }
    g.set(12, 2, CellData::oil(0));
    let mut rng = StdRng::seed_from_u64(42);
    for _ in 0..16 {
        physics::step(&mut g, &mut rng);
    }
    let oil = g.cells.iter().position(|c| c.kind == Cell::Oil).unwrap();
    assert!(oil / 24 > 8, "denser oil must sink in water");
    let mut g = Grid::new(24, 24);
    let mut rules = MaterialTable::default();
    rules.set_spread(Cell::Water, 0).unwrap();
    g.set_materials(rules);
    for x in 0..24 {
        g.set(x, 23, CellData::stone(0));
    }
    g.set(12, 22, CellData::water(0));
    for _ in 0..30 {
        physics::step(&mut g, &mut rng);
    }
    assert_eq!(g.get(12, 22).kind, Cell::Water);
    assert_eq!(std::mem::size_of::<CellData>(), 2);
}

#[test]
fn invalid_properties_are_rejected_without_changing_table() {
    let mut rules = MaterialTable::default();
    for angle in [f32::NAN, f32::INFINITY, -1.0, 90.0] {
        assert!(rules.set_repose(Cell::Sand, angle).is_err());
    }
    assert_eq!(rules.get(Cell::Sand).repose_degrees(), 32.0);
    assert!(rules.set_repose(Cell::Water, 32.0).is_err());
    assert!(rules.set_density(Cell::Oil, 0.0).is_err());
    assert!(rules.set_spread(Cell::Water, 33).is_err());
    rules.set_friction(Cell::Sand, 0.625).unwrap();
    assert!((rules.get(Cell::Sand).static_friction() - 0.625).abs() < 0.0001);
}
