use charon_engine::core::{PluginResult, Step};
use charon_engine::*;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
#[derive(Clone)]
struct Counter(Arc<Mutex<Vec<(&'static str, std::thread::ThreadId)>>>);
impl Plugin for Counter {
    fn name(&self) -> &'static str {
        "counter"
    }
    fn init(&mut self, _: &mut World) -> PluginResult {
        self.0
            .lock()
            .unwrap()
            .push(("init", std::thread::current().id()));
        Ok(())
    }
    fn fixed_update(&mut self, w: &mut World, _: Step) -> PluginResult {
        *w.get_mut::<u32>().ok_or("missing counter")? += 1;
        Ok(())
    }
    fn shutdown(&mut self, _: &mut World) -> PluginResult {
        self.0
            .lock()
            .unwrap()
            .push(("stop", std::thread::current().id()));
        Ok(())
    }
}
#[test]
fn builder_and_direct_api_execute_identically_and_expose_the_same_world() {
    let config = Startup::default();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut high = AppBuilder::new(config)
        .unwrap()
        .resource(0_u32)
        .plugin(Counter(events.clone()))
        .unwrap()
        .build();
    let mut low =
        App::with_timing(config.rates().delta(Lane::Game), config.max_catch_up()).unwrap();
    low.world_mut().insert(0_u32);
    low.add_plugin(Counter(events.clone())).unwrap();
    for app in [&mut low, &mut high] {
        app.start().unwrap();
        for _ in 0..10 {
            app.tick().unwrap();
        }
        app.shutdown().unwrap();
    }
    assert_eq!(high.world().get::<u32>(), low.world().get::<u32>());
    assert_eq!(high.world().get::<u32>(), Some(&10));
    assert_eq!(events.lock().unwrap().len(), 4);
}
#[test]
fn timing_is_validated_and_does_not_couple_lanes() {
    assert!(Rates::new(0, 60, 60).is_err());
    assert!(Rates::new(144, 1001, 60).is_err());
    assert!(Startup::new(Rates::default(), 0).is_err());
    let rates = Rates::new(144, 60, 30).unwrap();
    let startup = Startup::new(rates, 4).unwrap();
    assert_ne!(rates.delta(Lane::Game), rates.delta(Lane::Render));
    assert_eq!(
        startup.app(Lane::Compute).unwrap().build().fixed_delta(),
        Duration::from_secs_f64(1. / 30.)
    );
    assert!(startup.spawn(Lane::Render, "bad", Ok, ()).is_err());
}
#[test]
fn worker_factory_runs_and_cleans_up_in_its_owner_thread() {
    let host = std::thread::current().id();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let mut worker = Startup::default()
        .spawn(
            Lane::Game,
            "api-test",
            move |app| app.resource(0_u32).plugin(Counter(capture)),
            (),
        )
        .unwrap();
    worker.stop();
    worker.join().unwrap();
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].0, "init");
    assert_eq!(events[1].0, "stop");
    assert_ne!(events[0].1, host);
    assert_eq!(events[0].1, events[1].1);
}
#[test]
fn duplicate_and_missing_plugins_keep_core_diagnostics() {
    let counter = || Counter(Arc::new(Mutex::new(Vec::new())));
    let result = AppBuilder::new(Startup::default())
        .unwrap()
        .plugin(counter())
        .unwrap()
        .plugin(counter());
    assert!(matches!(result, Err(CoreError::DuplicatePlugin("counter"))));
    struct Needs;
    impl Plugin for Needs {
        fn name(&self) -> &'static str {
            "needs"
        }
        fn dependencies(&self) -> &[&'static str] {
            &["absent"]
        }
    }
    let mut app = AppBuilder::new(Startup::default())
        .unwrap()
        .plugin(Needs)
        .unwrap()
        .build();
    assert!(matches!(
        app.start(),
        Err(CoreError::MissingDependency {
            plugin: "needs",
            dependency: "absent"
        })
    ));
}
#[cfg(feature = "interaction")]
#[test]
fn interaction_bundle_matches_individual_plugins_and_closes_input() {
    use bundles::Interaction2D;
    let camera = camera::Camera2D::new(
        [0.; 2],
        1.,
        camera::Rect {
            x: 0.,
            y: 0.,
            width: 100.,
            height: 100.,
        },
    )
    .unwrap();
    let bundle = Interaction2D::new(camera, 8).unwrap();
    let client = bundle.client();
    let mut app = AppBuilder::new(Startup::default())
        .unwrap()
        .bundle(bundle)
        .unwrap()
        .build();
    app.start().unwrap();
    assert_eq!(app.world().get::<camera::Camera2D>(), Some(&camera));
    assert!(app.world().get::<assets::Assets<assets::Image>>().is_some());
    client
        .try_send(input::InputEvent::Button {
            button: input::keys::ENTER.into(),
            pressed: true,
        })
        .unwrap();
    app.tick().unwrap();
    assert!(
        app.world()
            .get::<input::InputState>()
            .unwrap()
            .pressed(input::keys::ENTER)
    );
    app.shutdown().unwrap();
    assert!(client.try_send(input::InputEvent::Focus(true)).is_err());
    assert!(app.world().get::<camera::Camera2D>().is_none());
    assert!(app.world().get::<assets::Assets<assets::Image>>().is_none());
}
#[cfg(feature = "scenes")]
#[test]
fn scene_catalog_uses_matching_timing_and_restarts_with_fresh_resources() {
    use scenes::{
        Directive, SceneId,
        integration::{SceneLogic, ScopedScene},
    };
    struct Logic;
    impl SceneLogic for Logic {
        fn fixed_update(&mut self, w: &mut World, _: scenes::Step) -> Result<Directive, String> {
            *w.get_mut::<u32>().unwrap() += 1;
            Ok(Directive::Continue)
        }
    }
    let startup = Startup::new(Rates::new(120, 60, 30).unwrap(), 3).unwrap();
    let mut manager = SceneCatalog::new(startup)
        .unwrap()
        .scene("game", |_: SceneId, app| {
            Ok(Box::new(ScopedScene::new(
                app.resource(0_u32).build(),
                Logic,
            )))
        })
        .unwrap()
        .build("game")
        .unwrap();
    manager.advance(startup.rates().delta(Lane::Game)).unwrap();
    assert_eq!(
        manager
            .active_as::<ScopedScene>()
            .unwrap()
            .world()
            .get::<u32>(),
        Some(&1)
    );
    let old = manager.active_id();
    manager.switch_to("game").unwrap();
    assert_ne!(old, manager.active_id());
    assert_eq!(
        manager
            .active_as::<ScopedScene>()
            .unwrap()
            .world()
            .get::<u32>(),
        Some(&0)
    );
    manager.shutdown().unwrap();
}
