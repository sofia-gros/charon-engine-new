use charon_tracing::{charon_lighting::*, *};
fn scene() -> Scene {
    Scene::new(
        Geometry {
            width: 12,
            height: 8,
            origin: [-16., 24.],
            cell_size: 1.,
        },
        Settings {
            sky: [0.; 3],
            ambient: [0.; 3],
            max_distance: 16.,
            ao_strength: 0.,
            ..Settings::default()
        },
    )
    .unwrap()
}
#[test]
fn walls_occlude_water_transmits_and_digging_removes_occlusion() {
    let mut s = scene();
    let mut layer = OpticalGrid::new(s.geometry()).unwrap();
    for y in 0..8 {
        layer.set(6, y, Optical::OPAQUE).unwrap();
    }
    s.set_layer(1, layer.clone()).unwrap();
    let a = [-13.5, 28.5];
    let b = [-6.5, 28.5];
    assert_eq!(
        visibility(&TraceScene::new(s.prepare()).unwrap(), a, b, 64).unwrap(),
        [0.; 3]
    );
    layer
        .set(
            6,
            4,
            Optical {
                opacity: 0.1,
                transmission: [0.6, 0.8, 1.],
                ..Optical::AIR
            },
        )
        .unwrap();
    s.set_layer(1, layer).unwrap();
    let trans = visibility(&TraceScene::new(s.prepare()).unwrap(), a, b, 64).unwrap();
    assert!(trans[0] > 0. && trans[0] < trans[1] && trans[1] < trans[2]);
    s.remove_layer(1);
    assert_eq!(
        visibility(&TraceScene::new(s.prepare()).unwrap(), a, b, 64).unwrap(),
        [1.; 3]
    );
    assert!(
        visibility(
            &TraceScene::new(s.prepare()).unwrap(),
            [f32::NAN, 0.],
            b,
            64
        )
        .is_err()
    );
    assert!(visibility(&TraceScene::new(s.prepare()).unwrap(), [-999., 0.], b, 64).is_err());
}
#[test]
fn accumulation_converges_caps_and_resets_for_same_revision_different_world() {
    let mut s = scene();
    s.set_light(
        1,
        PointLight {
            position: [-13.5, 28.5],
            color: [1., 0.5, 0.1],
            intensity: 1.,
        },
    )
    .unwrap();
    let input = TraceScene::new(s.prepare()).unwrap();
    let mut acc = Accumulator::default();
    let q = Quality {
        samples_per_update: 4,
        max_samples: 8,
        ..Quality::for_mode(Mode::Path)
    };
    let a = acc.update(&input, q).unwrap();
    assert_eq!(acc.samples(), 4);
    assert!(
        a.rgba
            .iter()
            .flatten()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    );
    acc.update(&input, q).unwrap();
    assert_eq!(acc.samples(), 8);
    let b = acc.update(&input, q).unwrap();
    let c = acc.update(&input, q).unwrap();
    assert_eq!(b.rgba, c.rgba);
    let mut other = input.clone();
    other.input.sources.fill([0.; 3]);
    let dark = acc.update(&other, q).unwrap();
    assert_eq!(acc.samples(), 4);
    assert!(dark.rgba.iter().all(|p| p[..3] == [0.; 3]));
    let mut other = input.clone();
    other.reflectance[0] = [0.1; 3];
    acc.update(&other, q).unwrap();
    assert_eq!(acc.samples(), 4);
}
#[test]
fn mobile_modes_and_budgets_are_enforced() {
    assert!(Mode::Rtx.validate(Platform::Mobile).is_err());
    assert!(Mode::Path.validate(Platform::Mobile).is_err());
    assert!(Mode::Legacy.validate(Platform::Mobile).is_ok());
    let q = Quality::for_mode(Mode::LitePath);
    assert!(q.validate(Mode::LitePath, Platform::Mobile).is_ok());
    assert!(
        Quality { bounces: 2, ..q }
            .validate(Mode::LitePath, Platform::Mobile)
            .is_err()
    );
}
#[test]
fn exhausted_rays_are_not_reported_as_clear() {
    let s = TraceScene::new(scene().prepare()).unwrap();
    assert_eq!(
        visibility(&s, [-15.5, 24.5], [-5.5, 24.5], 1).unwrap(),
        [0.; 3]
    );
}
#[test]
fn tilemap_and_particle_layers_both_block_the_same_rays() {
    use charon_tilemap::{TileDefinition, Tilemap};
    let mut s = scene();
    let mut map = Tilemap::new(12, 8, [1, 1]).unwrap();
    let stone = map
        .register(TileDefinition {
            name: "wall".into(),
            solid: true,
            atlas_index: 0,
            tint: [1.; 4],
        })
        .unwrap();
    map.set(6, 4, Some(stone)).unwrap();
    let grid = charon_tilemap_lighting::rasterize(
        &map,
        s.geometry(),
        s.geometry().origin,
        &Default::default(),
    )
    .unwrap();
    s.set_layer(1, grid).unwrap();
    let a = [-13.5, 28.5];
    let b = [-6.5, 28.5];
    assert_eq!(
        visibility(&TraceScene::new(s.prepare()).unwrap(), a, b, 64).unwrap(),
        [0.; 3]
    );
    s.remove_layer(1);
    // Snapshot optical data is built from actual falling-sand cell storage.
    use charon_physics::sim::{
        material::{Cell, CellData},
        simulation::Simulation,
    };
    let mut physics = Simulation::new(12, 8, 7);
    physics.apply(charon_physics::sim::simulation::Request {
        id: 1,
        expected_revision: None,
        operation: charon_physics::sim::simulation::Operation::Brush {
            x: 6,
            y: 4,
            radius: 0,
            cell: CellData::sand(1),
        },
    });
    let grid = charon_sand_lighting::rasterize(
        &physics.snapshot(),
        s.geometry(),
        s.geometry().origin,
        1.,
        |c| {
            if c.kind == Cell::Air {
                Optical::AIR
            } else {
                Optical::OPAQUE
            }
        },
    )
    .unwrap();
    s.set_layer(2, grid).unwrap();
    assert_eq!(
        visibility(&TraceScene::new(s.prepare()).unwrap(), a, b, 64).unwrap(),
        [0.; 3]
    );
    s.remove_layer(2);
    assert_eq!(
        visibility(&TraceScene::new(s.prepare()).unwrap(), a, b, 64).unwrap(),
        [1.; 3]
    );
}

#[test]
fn moving_subcell_lights_have_sample_independent_direct_illumination() {
    let mut input = TraceScene::new(scene().prepare()).unwrap();
    let q = Quality {
        samples_per_update: 1,
        max_samples: 16,
        bounces: 0,
        ..Quality::for_mode(Mode::Path)
    };
    let mut acc = Accumulator::default();
    for x in [-13.5, -13.4, -13.3] {
        input.point_lights = vec![PointLight {
            position: [x, 28.5],
            color: [1., 0.5, 0.1],
            intensity: 1.,
        }];
        let first = acc.update(&input, q).unwrap();
        assert_eq!(acc.samples(), 1);
        for _ in 1..16 {
            assert_eq!(first.rgba, acc.update(&input, q).unwrap().rgba);
        }
        assert!(first.rgba.iter().any(|p| p[0] > 0.));
        input.input.revision += 1;
        acc.update(&input, q).unwrap();
        assert_eq!(
            acc.samples(),
            16,
            "revision-only notification must not destroy history"
        );
    }
}
