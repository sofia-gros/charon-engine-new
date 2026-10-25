#![cfg(feature = "lighting")]
use charon_engine::{lighting::*, *};
fn lighting() -> Lighting {
    let config = LightingConfig::new(
        Geometry {
            width: 12,
            height: 8,
            origin: [-16., 24.],
            cell_size: 4.,
        },
        Settings {
            sky: [0.; 3],
            ambient: [0.; 3],
            max_distance: 24.,
            ..Settings::default()
        },
        30,
    )
    .unwrap();
    let mut l = Lighting::new(config).unwrap();
    l.set_light(
        1,
        PointLight {
            position: [2., 38.],
            color: [1., 0.5, 0.25],
            intensity: 1.,
        },
    )
    .unwrap();
    l
}
#[test]
fn world_sampling_matches_low_level_and_invalidates_on_edits() {
    let mut l = lighting();
    let input = l.prepare();
    let expected = low_level::solve_cpu(&input).unwrap();
    assert!(l.sample_world([2., 38.]).is_none());
    l.solve_cpu().unwrap();
    assert_eq!(l.sample_world([2., 38.]), expected.sample([2., 38.]));
    assert!(l.sample_world([-17., 30.]).is_none());
    assert!(l.sample_world([f32::NAN, 30.]).is_none());
    assert!(!l.solve_cpu().unwrap());
    l.remove_light(1);
    assert!(l.field().is_none());
    assert!(l.sample_world([2., 38.]).is_none());
    l.solve_cpu().unwrap();
    assert_eq!(l.sample_world([2., 38.]).unwrap()[0], 0.);
}
#[test]
fn backend_rejects_stale_and_malformed_fields_without_overwriting_current_result() {
    let mut l = lighting();
    let old = low_level::solve_cpu(&l.prepare()).unwrap();
    l.remove_light(1);
    assert!(!l.accept(old).unwrap());
    l.solve_cpu().unwrap();
    let before = l.sample_world([2., 38.]);
    let mut bad = l.field().unwrap().clone();
    bad.rgba[0][0] = f32::NAN;
    assert!(l.accept(bad).is_err());
    let mut bad = l.field().unwrap().clone();
    bad.geometry.origin = [0.; 2];
    assert!(l.accept(bad).is_err());
    let mut bad = l.field().unwrap().clone();
    bad.rgba.pop();
    assert!(l.accept(bad).is_err());
    assert_eq!(l.sample_world([2., 38.]), before);
}
#[test]
fn optics_and_low_level_settings_are_shared_by_both_api_levels() {
    let mut l = lighting();
    let mut layer = OpticalGrid::new(l.scene().geometry()).unwrap();
    layer
        .raster_rect([4., 24., 4., 32.], Optical::OPAQUE)
        .unwrap();
    l.set_layer(9, layer).unwrap();
    l.solve_cpu().unwrap();
    let before = l.sample_world([14., 38.]).unwrap()[0];
    l.remove_layer(9);
    l.solve_cpu().unwrap();
    assert!(l.sample_world([14., 38.]).unwrap()[0] > before);
    let mut settings = l.scene().settings();
    settings.ao_strength = 0.;
    l.scene_mut().set_settings(settings).unwrap();
    assert!(l.field().is_none());
}
#[test]
fn quality_config_validates_bounds_resolution_and_rate() {
    let low = LightingConfig::for_bounds([-10., 20., 321., 193.], Quality::Low).unwrap();
    let high = LightingConfig::for_bounds([-10., 20., 321., 193.], Quality::High).unwrap();
    assert_eq!(low.geometry().origin, [-10., 20.]);
    assert_eq!(low.geometry().width, 81);
    assert_eq!(high.geometry().width, 161);
    assert_eq!(high.update_hz(), 60);
    assert!(LightingConfig::for_bounds([0., 0., f32::INFINITY, 10.], Quality::Low).is_err());
    assert!(LightingConfig::for_bounds([0., 0., 0., 10.], Quality::High).is_err());
    assert!(LightingConfig::new(low.geometry(), low.settings(), 0).is_err());
    let mut l = lighting();
    assert!(l.set_update_hz(241).is_err());
    assert_eq!(l.update_hz(), 30);
}
#[test]
fn cpu_plugin_respects_light_rate_and_cleanup() {
    let mut app = AppBuilder::new(Startup::new(Rates::new(60, 60, 30).unwrap(), 8).unwrap())
        .unwrap()
        .plugin(CpuLightingPlugin::new(lighting()))
        .unwrap()
        .build();
    app.start().unwrap();
    app.tick().unwrap();
    app.world_mut()
        .get_mut::<Lighting>()
        .unwrap()
        .remove_light(1);
    app.tick().unwrap();
    assert_eq!(
        app.world()
            .get::<Lighting>()
            .unwrap()
            .sample_world([2., 38.])
            .unwrap()[0],
        0.
    );
    app.shutdown().unwrap();
    assert!(app.world().get::<Lighting>().is_none());
}

#[test]
fn replacing_the_low_level_scene_at_the_same_revision_cannot_reuse_cached_light() {
    let mut l = lighting();
    l.solve_cpu().unwrap();
    assert!(l.sample_world([2., 38.]).is_some());
    let mut replacement = Scene::new(l.scene().geometry(), l.scene().settings()).unwrap();
    replacement
        .set_light(
            2,
            PointLight {
                position: [20., 40.],
                color: [0., 1., 0.],
                intensity: 1.,
            },
        )
        .unwrap();
    assert_eq!(replacement.revision(), l.scene().revision());
    *l.scene_mut() = replacement;
    assert!(l.field().is_none());
    assert!(l.sample_world([2., 38.]).is_none());
    l.solve_cpu().unwrap();
    assert_eq!(l.sample_world([20., 40.]).unwrap()[0], 0.);
}
