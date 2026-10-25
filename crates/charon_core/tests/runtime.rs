use charon_core::{App, CoreError, Phase, Plugin, PluginResult, State, Step, World};
use std::time::Duration;

#[derive(Default)]
struct Log(Vec<String>);

struct Probe {
    name: &'static str,
    deps: &'static [&'static str],
    after: &'static [&'static str],
    before: &'static [&'static str],
    fail: Option<Phase>,
    fail_shutdown: bool,
}

impl Probe {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            deps: &[],
            after: &[],
            before: &[],
            fail: None,
            fail_shutdown: false,
        }
    }

    fn record(&self, world: &mut World, phase: Phase) -> PluginResult {
        world
            .get_mut::<Log>()
            .unwrap()
            .0
            .push(format!("{}:{phase:?}", self.name));
        if self.fail == Some(phase) || (phase == Phase::Shutdown && self.fail_shutdown) {
            Err(format!("{} failure", self.name))
        } else {
            Ok(())
        }
    }
}

impl Plugin for Probe {
    fn name(&self) -> &'static str {
        self.name
    }
    fn dependencies(&self) -> &[&'static str] {
        self.deps
    }
    fn after(&self) -> &[&'static str] {
        self.after
    }
    fn before(&self) -> &[&'static str] {
        self.before
    }
    fn init(&mut self, world: &mut World) -> PluginResult {
        self.record(world, Phase::Init)
    }
    fn fixed_update(&mut self, world: &mut World, _: Step) -> PluginResult {
        self.record(world, Phase::Fixed)
    }
    fn frame_update(&mut self, world: &mut World, _: Step) -> PluginResult {
        self.record(world, Phase::Frame)
    }
    fn shutdown(&mut self, world: &mut World) -> PluginResult {
        self.record(world, Phase::Shutdown)
    }
}

fn app() -> App {
    let mut app = App::with_timing(Duration::from_millis(10), 3).unwrap();
    app.world_mut().insert(Log::default());
    app
}

fn log(app: &App) -> Vec<&str> {
    app.world()
        .get::<Log>()
        .unwrap()
        .0
        .iter()
        .map(String::as_str)
        .collect()
}

#[test]
fn empty_app_and_invalid_lifecycle() {
    let mut app = app();
    assert!(matches!(
        app.tick(),
        Err(CoreError::InvalidState(State::Configuring))
    ));
    app.start().unwrap();
    assert!(app.plugin_order().is_empty());
    app.advance(Duration::from_secs(1)).unwrap();
    assert!(app.add_plugin(Probe::new("late")).is_err());
    assert!(app.start().is_err());
    app.shutdown().unwrap();
    app.shutdown().unwrap();
    assert_eq!(app.state(), State::Stopped);
    assert!(app.frame(Duration::ZERO).is_err());
    assert!(app.start().is_err());
}

#[test]
fn dependency_order_and_reverse_cleanup() {
    let mut app = app();
    let mut b = Probe::new("b");
    b.deps = &["a", "a"];
    app.add_plugin(b).unwrap();
    app.add_plugin(Probe::new("a")).unwrap();
    app.start().unwrap();
    assert_eq!(app.plugin_order(), ["a", "b"]);
    app.tick().unwrap();
    app.frame(Duration::ZERO).unwrap();
    app.shutdown().unwrap();
    assert_eq!(
        log(&app),
        [
            "a:Init",
            "b:Init",
            "a:Fixed",
            "b:Fixed",
            "a:Frame",
            "b:Frame",
            "b:Shutdown",
            "a:Shutdown"
        ]
    );
}

#[test]
fn stable_order_and_optional_constraints() {
    let mut app = app();
    let mut a = Probe::new("a");
    a.after = &["c", "absent"];
    let mut b = Probe::new("b");
    b.before = &["c", "absent"];
    app.add_plugin(a).unwrap();
    app.add_plugin(b).unwrap();
    app.add_plugin(Probe::new("c")).unwrap();
    app.add_plugin(Probe::new("d")).unwrap();
    app.start().unwrap();
    assert_eq!(app.plugin_order(), ["b", "c", "a", "d"]);
}

#[test]
fn graph_errors_run_no_callbacks_and_missing_dependency_can_be_added() {
    let mut app = app();
    let mut a = Probe::new("a");
    a.deps = &["missing"];
    app.add_plugin(a).unwrap();
    assert!(matches!(
        app.add_plugin(Probe::new("a")),
        Err(CoreError::DuplicatePlugin("a"))
    ));
    assert!(matches!(
        app.add_plugin(Probe::new(" ")),
        Err(CoreError::InvalidName)
    ));
    assert_eq!(
        app.start(),
        Err(CoreError::MissingDependency {
            plugin: "a",
            dependency: "missing"
        })
    );
    assert!(log(&app).is_empty());
    assert_eq!(app.state(), State::Configuring);
    app.add_plugin(Probe::new("missing")).unwrap();
    app.start().unwrap();
    assert_eq!(app.plugin_order(), ["missing", "a"]);
}

#[test]
fn detects_dependency_and_order_cycles_before_init() {
    for self_cycle in [false, true] {
        let mut app = app();
        let mut a = Probe::new("a");
        a.deps = &["b"];
        let mut b = Probe::new("b");
        b.after = if self_cycle { &["b"] } else { &["a"] };
        app.add_plugin(a).unwrap();
        app.add_plugin(b).unwrap();
        assert!(matches!(app.start(), Err(CoreError::DependencyCycle(_))));
        assert!(log(&app).is_empty());
    }
}

#[test]
fn init_failure_cleans_partial_plugin_and_dependencies_once() {
    let mut app = app();
    app.add_plugin(Probe::new("a")).unwrap();
    let mut b = Probe::new("b");
    b.fail = Some(Phase::Init);
    app.add_plugin(b).unwrap();
    app.add_plugin(Probe::new("never_started")).unwrap();
    let error = app.start().unwrap_err();
    assert!(
        matches!(error, CoreError::Callback { failure, cleanup_errors } if failure.plugin == "b" && failure.phase == Phase::Init && cleanup_errors.is_empty())
    );
    assert_eq!(app.state(), State::Failed);
    app.shutdown().unwrap();
    assert_eq!(log(&app), ["a:Init", "b:Init", "b:Shutdown", "a:Shutdown"]);
}

#[test]
fn update_failures_preserve_primary_error_and_cleanup_errors() {
    for phase in [Phase::Fixed, Phase::Frame] {
        let mut app = app();
        let mut a = Probe::new("a");
        a.fail_shutdown = true;
        app.add_plugin(a).unwrap();
        let mut b = Probe::new("b");
        b.fail = Some(phase);
        app.add_plugin(b).unwrap();
        app.add_plugin(Probe::new("c")).unwrap();
        app.start().unwrap();
        let result = if phase == Phase::Fixed {
            app.tick()
        } else {
            app.frame(Duration::ZERO)
        };
        match result.unwrap_err() {
            CoreError::Callback {
                failure,
                cleanup_errors,
            } => {
                assert_eq!(failure.plugin, "b");
                assert_eq!(failure.phase, phase);
                assert_eq!(cleanup_errors.len(), 1);
                assert_eq!(cleanup_errors[0].plugin, "a");
            }
            other => panic!("unexpected: {other}"),
        }
        assert_eq!(app.state(), State::Failed);
        assert!(log(&app).ends_with(&["c:Shutdown", "b:Shutdown", "a:Shutdown"]));
        assert!(!log(&app).contains(&format!("c:{phase:?}").as_str()));
        let count = log(&app).len();
        app.shutdown().unwrap();
        assert_eq!(log(&app).len(), count);
    }
}

#[test]
fn shutdown_continues_after_errors() {
    let mut app = app();
    for name in ["a", "b"] {
        let mut p = Probe::new(name);
        p.fail_shutdown = true;
        app.add_plugin(p).unwrap();
    }
    app.start().unwrap();
    assert!(matches!(app.shutdown(), Err(CoreError::Shutdown(errors)) if errors.len() == 2));
    assert_eq!(log(&app), ["a:Init", "b:Init", "b:Shutdown", "a:Shutdown"]);
    app.shutdown().unwrap();
}

#[test]
fn drop_cleans_running_plugins() {
    use std::cell::Cell;
    use std::rc::Rc;
    struct OnDrop(Rc<Cell<u32>>);
    impl Plugin for OnDrop {
        fn name(&self) -> &'static str {
            "drop"
        }
        fn shutdown(&mut self, _: &mut World) -> PluginResult {
            self.0.set(self.0.get() + 1);
            Ok(())
        }
    }
    let count = Rc::new(Cell::new(0));
    {
        let mut app = App::new();
        app.add_plugin(OnDrop(count.clone())).unwrap();
        app.start().unwrap();
    }
    assert_eq!(count.get(), 1);
}

#[derive(Default)]
struct Steps(Vec<(Phase, Step)>);
struct Clock;
impl Plugin for Clock {
    fn name(&self) -> &'static str {
        "clock"
    }
    fn fixed_update(&mut self, w: &mut World, s: Step) -> PluginResult {
        w.get_mut::<Steps>().unwrap().0.push((Phase::Fixed, s));
        Ok(())
    }
    fn frame_update(&mut self, w: &mut World, s: Step) -> PluginResult {
        w.get_mut::<Steps>().unwrap().0.push((Phase::Frame, s));
        Ok(())
    }
}

#[test]
fn accumulator_runs_fixed_before_frame_and_bounds_catchup() {
    let mut app = app();
    app.world_mut().insert(Steps::default());
    app.add_plugin(Clock).unwrap();
    app.start().unwrap();
    let first = app.advance(Duration::from_millis(25)).unwrap();
    assert_eq!(first.fixed_steps, 2);
    assert_eq!(first.dropped_time, Duration::ZERO);
    assert!((first.interpolation - 0.5).abs() < 1e-9);
    let second = app.advance(Duration::from_millis(105)).unwrap();
    assert_eq!(second.fixed_steps, 3);
    assert_eq!(second.dropped_time, Duration::from_millis(80));
    assert_eq!(second.interpolation, 0.0);
    let steps = &app.world().get::<Steps>().unwrap().0;
    assert_eq!(
        steps.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
        [
            Phase::Fixed,
            Phase::Fixed,
            Phase::Frame,
            Phase::Fixed,
            Phase::Fixed,
            Phase::Fixed,
            Phase::Frame
        ]
    );
    let fixed: Vec<_> = steps
        .iter()
        .filter(|(p, _)| *p == Phase::Fixed)
        .map(|(_, s)| *s)
        .collect();
    assert_eq!(
        fixed.iter().map(|s| s.index).collect::<Vec<_>>(),
        [0, 1, 2, 3, 4]
    );
    assert!(fixed.iter().all(|s| s.delta == Duration::from_millis(10)));
    assert_eq!(steps.last().unwrap().1.index, 1);
    assert_eq!(steps.last().unwrap().1.delta, Duration::from_millis(105));
}

#[test]
fn timing_rejects_zero_and_accepts_zero_elapsed() {
    assert!(matches!(
        App::with_timing(Duration::ZERO, 1),
        Err(CoreError::InvalidTiming)
    ));
    assert!(matches!(
        App::with_timing(Duration::from_nanos(1), 0),
        Err(CoreError::InvalidTiming)
    ));
    let mut app = app();
    app.start().unwrap();
    assert_eq!(app.advance(Duration::ZERO).unwrap().fixed_steps, 0);
}

#[test]
fn typed_resources_and_broadcast_events() {
    let mut w = World::new();
    assert!(w.get::<u32>().is_none());
    assert_eq!(w.insert(10u32), None);
    assert_eq!(w.insert(20u32), Some(10));
    *w.get_mut::<u32>().unwrap() += 1;
    w.insert("different type");
    assert_eq!(w.remove::<u32>(), Some(21));
    assert!(w.get::<u32>().is_none());
    assert_eq!(w.get::<&str>(), Some(&"different type"));
    w.send(1u8);
    w.send(2u8);
    w.send("separate event");
    assert_eq!(w.events::<u8>(), [1, 2]);
    assert_eq!(w.events::<u8>(), [1, 2]); // another reader sees the same events
    w.clear_events::<u8>();
    assert!(w.events::<u8>().is_empty());
    assert_eq!(w.events::<&str>(), ["separate event"]);
}
