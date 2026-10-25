use charon_lighting::*;
fn geometry() -> Geometry {
    Geometry {
        width: 16,
        height: 16,
        origin: [0., 0.],
        cell_size: 1.,
    }
}
fn settings() -> Settings {
    Settings {
        sky: [0.; 3],
        ambient: [0.; 3],
        falloff_per_unit: 0.8,
        max_distance: 32.,
        ao_strength: 0.,
    }
}
fn source(x: f32, y: f32) -> PointLight {
    PointLight {
        position: [x, y],
        color: [1.; 3],
        intensity: 1.,
    }
}
#[test]
fn opaque_wall_receives_light_but_never_transmits_it() {
    let g = geometry();
    let mut scene = Scene::new(g, settings()).unwrap();
    let mut wall = OpticalGrid::new(g).unwrap();
    wall.raster_rect([7., 0., 1., 16.], Optical::OPAQUE)
        .unwrap();
    scene.set_layer(1, wall).unwrap();
    scene.set_light(1, source(4., 8.)).unwrap();
    let field = solve_cpu(&scene.prepare()).unwrap();
    assert!(field.sample([7., 8.]).unwrap()[0] > 0.);
    assert_eq!(field.sample([9., 8.]).unwrap()[0], 0.);
}
#[test]
fn surface_and_sealed_cave_have_distinct_sky_illumination() {
    let g = geometry();
    let mut s = settings();
    s.sky = [1.; 3];
    let mut scene = Scene::new(g, s).unwrap();
    let mut roof = OpticalGrid::new(g).unwrap();
    roof.raster_rect([0., 5., 16., 1.], Optical::OPAQUE)
        .unwrap();
    scene.set_layer(0, roof).unwrap();
    let f = solve_cpu(&scene.prepare()).unwrap();
    assert_eq!(f.sample([8., 4.]).unwrap()[0], 1.);
    assert_eq!(f.sample([8., 5.]).unwrap()[0], 1.);
    assert_eq!(f.sample([8., 6.]).unwrap()[0], 0.);
}
#[test]
fn closing_passage_and_removing_or_moving_sources_clear_old_light() {
    let g = geometry();
    let mut scene = Scene::new(g, settings()).unwrap();
    scene.set_light(1, source(3., 8.)).unwrap();
    let mut wall = OpticalGrid::new(g).unwrap();
    wall.raster_rect([7., 0., 1., 7.], Optical::OPAQUE).unwrap();
    wall.raster_rect([7., 9., 1., 7.], Optical::OPAQUE).unwrap();
    scene.set_layer(1, wall).unwrap();
    assert!(
        solve_cpu(&scene.prepare())
            .unwrap()
            .sample([9., 8.])
            .unwrap()[0]
            > 0.
    );
    let mut closed = OpticalGrid::new(g).unwrap();
    closed
        .raster_rect([7., 0., 1., 16.], Optical::OPAQUE)
        .unwrap();
    scene.set_layer(1, closed).unwrap();
    assert_eq!(
        solve_cpu(&scene.prepare())
            .unwrap()
            .sample([9., 8.])
            .unwrap()[0],
        0.
    );
    scene.set_light(1, source(-10., -10.)).unwrap();
    assert!(
        solve_cpu(&scene.prepare())
            .unwrap()
            .rgba
            .iter()
            .all(|v| v[..3] == [0.; 3])
    );
    scene.remove_light(1);
    assert!(
        solve_cpu(&scene.prepare())
            .unwrap()
            .rgba
            .iter()
            .all(|v| v[..3] == [0.; 3])
    );
}
#[test]
fn emitting_opaque_ore_illuminates_neighbors_and_removal_clears() {
    let g = geometry();
    let mut scene = Scene::new(g, settings()).unwrap();
    let mut grid = OpticalGrid::new(g).unwrap();
    let ore = Optical {
        emission_color: [0.2, 0.6, 1.],
        emission_strength: 1.,
        ..Optical::OPAQUE
    };
    grid.set(8, 8, ore).unwrap();
    scene.set_layer(1, grid).unwrap();
    let f = solve_cpu(&scene.prepare()).unwrap();
    assert_eq!(f.sample([8., 8.]).unwrap()[2], 1.);
    assert!(f.sample([9., 8.]).unwrap()[2] > 0.);
    assert!(f.sample([9., 8.]).unwrap()[2] > f.sample([9., 8.]).unwrap()[0]);
    scene.remove_layer(1);
    assert_eq!(
        solve_cpu(&scene.prepare())
            .unwrap()
            .sample([9., 8.])
            .unwrap()[2],
        0.
    );
}
#[test]
fn transmission_color_and_distance_change_light() {
    let g = geometry();
    let mut scene = Scene::new(g, settings()).unwrap();
    let mut grid = OpticalGrid::new(g).unwrap();
    grid.raster_rect(
        [7., 0., 1., 16.],
        Optical {
            opacity: 0.1,
            transmission: [0.2, 0.5, 1.],
            ..Optical::AIR
        },
    )
    .unwrap();
    scene.set_layer(1, grid).unwrap();
    scene.set_light(1, source(4., 8.)).unwrap();
    let f = solve_cpu(&scene.prepare()).unwrap();
    let rgb = f.sample([9., 8.]).unwrap();
    assert!(rgb[2] > rgb[0]);
    assert!(f.sample([5., 8.]).unwrap()[2] > rgb[2]);
}
#[test]
fn ao_darkens_contacts_independently_of_lighting() {
    let g = geometry();
    let mut grid = OpticalGrid::new(g).unwrap();
    grid.raster_rect([4., 4., 1., 3.], Optical::OPAQUE).unwrap();
    grid.raster_rect([4., 6., 3., 1.], Optical::OPAQUE).unwrap();
    let ao = ambient_occlusion(g, grid.cells(), 0.8).unwrap();
    assert!(ao[g.index([5., 5.]).unwrap()] < ao[g.index([10., 10.]).unwrap()]);
    assert!(
        ambient_occlusion(g, grid.cells(), 0.)
            .unwrap()
            .iter()
            .all(|&v| v == 1.)
    );
}
#[test]
fn layers_require_shared_coordinates_and_combine_independently_of_order() {
    let g = geometry();
    let mut a = OpticalGrid::new(g).unwrap();
    a.set(
        3,
        3,
        Optical {
            opacity: 0.5,
            ..Optical::AIR
        },
    )
    .unwrap();
    let mut b = OpticalGrid::new(g).unwrap();
    b.set(
        3,
        3,
        Optical {
            opacity: 0.5,
            transmission: [0.5; 3],
            ..Optical::AIR
        },
    )
    .unwrap();
    let mut s = Scene::new(g, settings()).unwrap();
    s.set_layer(1, a.clone()).unwrap();
    s.set_layer(2, b.clone()).unwrap();
    let mut t = Scene::new(g, settings()).unwrap();
    t.set_layer(1, b).unwrap();
    t.set_layer(2, a).unwrap();
    assert_eq!(s.prepare().optics, t.prepare().optics);
    assert_eq!(s.prepare().optics[g.index([3., 3.]).unwrap()].opacity, 0.75);
    assert!(
        s.set_layer(
            3,
            OpticalGrid::new(Geometry {
                origin: [1., 0.],
                ..g
            })
            .unwrap()
        )
        .is_err()
    );
}
#[test]
fn lighting_invalidation_is_independent_of_physics_and_includes_halo() {
    let g = geometry();
    let mut s = settings();
    s.max_distance = 2.;
    let mut scene = Scene::new(g, s).unwrap();
    scene.acknowledge(0);
    let mut grid = OpticalGrid::new(g).unwrap();
    grid.set(8, 8, Optical::OPAQUE).unwrap();
    assert!(scene.set_layer(1, grid.clone()).unwrap());
    let revision = scene.revision();
    let r = scene.dirty_region().unwrap();
    assert_eq!(
        r,
        Region {
            min: [5, 5],
            max: [12, 12]
        }
    );
    scene.acknowledge(revision - 1);
    assert!(scene.dirty_region().is_some());
    scene.acknowledge(revision);
    assert!(scene.dirty_region().is_none());
    assert!(!scene.set_layer(1, grid).unwrap());
    assert_eq!(scene.revision(), revision);
}
#[test]
fn viewport_can_sample_a_larger_world_with_offscreen_light() {
    let g = Geometry {
        width: 32,
        height: 16,
        origin: [-16., 0.],
        cell_size: 1.,
    };
    let mut scene = Scene::new(g, settings()).unwrap();
    scene.set_light(1, source(-2., 8.)).unwrap();
    let f = solve_cpu(&scene.prepare()).unwrap();
    assert!(f.sample([0., 8.]).unwrap()[0] > 0.);
    assert!(f.sample([-17., 8.]).is_none());
    assert!(g.index([f32::MAX, f32::MAX]).is_none());
}
#[test]
fn invalid_inputs_are_rejected_without_state_changes() {
    let g = geometry();
    let mut scene = Scene::new(g, settings()).unwrap();
    assert!(
        scene
            .set_light(
                1,
                PointLight {
                    position: [f32::NAN, 0.],
                    ..source(0., 0.)
                }
            )
            .is_err()
    );
    assert_eq!(scene.revision(), 0);
    assert!(
        scene
            .set_settings(Settings {
                max_distance: 10000.,
                ..settings()
            })
            .is_err()
    );
    let mut grid = OpticalGrid::new(g).unwrap();
    assert!(
        grid.set(
            0,
            0,
            Optical {
                opacity: f32::NAN,
                ..Optical::AIR
            }
        )
        .is_err()
    );
    assert!(Geometry { cell_size: 0., ..g }.validate().is_err());
    let mut prepared = scene.prepare();
    prepared.sources.clear();
    assert!(solve_cpu(&prepared).is_err());
}
#[cfg(feature = "plugin")]
#[test]
fn optional_plugin_updates_on_revision_and_cleans_up() {
    use charon_core::App;
    let mut app = App::new();
    app.add_plugin(LightingPlugin::new(
        Scene::new(geometry(), settings()).unwrap(),
    ))
    .unwrap();
    app.start().unwrap();
    app.tick().unwrap();
    assert_eq!(
        app.world()
            .get::<LightingState>()
            .unwrap()
            .field
            .as_ref()
            .unwrap()
            .revision,
        0
    );
    app.world_mut()
        .get_mut::<LightingState>()
        .unwrap()
        .scene
        .set_light(1, source(8., 8.))
        .unwrap();
    app.tick().unwrap();
    assert_eq!(
        app.world()
            .get::<LightingState>()
            .unwrap()
            .field
            .as_ref()
            .unwrap()
            .sample([8., 8.])
            .unwrap()[0],
        1.
    );
    app.shutdown().unwrap();
    assert!(app.world().get::<LightingState>().is_none());
}
