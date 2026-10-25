use charon_scene::*;
use std::{cell::RefCell, rc::Rc, time::Duration};
struct S {
    name: &'static str,
    log: Rc<RefCell<Vec<String>>>,
    fail_start: bool,
    fail_stop: bool,
}
impl Scene for S {
    fn start(&mut self, _: SceneId) -> SceneResult {
        self.log.borrow_mut().push(format!("{} start", self.name));
        if self.fail_start {
            Err("start failed".into())
        } else {
            Ok(())
        }
    }
    fn stop(&mut self) -> SceneResult {
        self.log.borrow_mut().push(format!("{} stop", self.name));
        if self.fail_stop {
            Err("stop failed".into())
        } else {
            Ok(())
        }
    }
    fn destroy(&mut self) -> SceneResult {
        self.log.borrow_mut().push(format!("{} destroy", self.name));
        Ok(())
    }
}
fn register(
    m: &mut SceneManager,
    name: &'static str,
    log: Rc<RefCell<Vec<String>>>,
    fail_start: bool,
    fail_stop: bool,
) {
    m.register(name, move |_| {
        Ok(Box::new(S {
            name,
            log: log.clone(),
            fail_start,
            fail_stop,
        }))
    })
    .unwrap();
}
#[test]
fn switch_disposes_old_once_and_rejects_stale_or_foreign_requests() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut m = SceneManager::new();
    register(&mut m, "a", log.clone(), false, false);
    register(&mut m, "b", log.clone(), false, false);
    m.switch_to("a").unwrap();
    let old = m.active_id().unwrap();
    m.request_switch(old, "b").unwrap();
    m.advance(Duration::ZERO).unwrap();
    assert_eq!(m.active_name(), Some("b"));
    assert!(m.request_switch(old, "a").is_err());
    let mut other = SceneManager::new();
    register(&mut other, "a", log.clone(), false, false);
    other.switch_to("a").unwrap();
    assert!(m.request_switch(other.active_id().unwrap(), "a").is_err());
    m.shutdown().unwrap();
    m.shutdown().unwrap();
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|s| s.as_str() == "a stop")
            .count(),
        1
    );
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|s| s.as_str() == "b destroy")
            .count(),
        1
    );
}
#[test]
fn candidate_start_failure_keeps_old_scene_and_cleans_partial_candidate() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut m = SceneManager::new();
    register(&mut m, "old", log.clone(), false, false);
    register(&mut m, "bad", log.clone(), true, false);
    m.switch_to("old").unwrap();
    let old = m.active_id();
    assert!(m.switch_to("bad").is_err());
    assert_eq!(m.active_id(), old);
    assert!(log.borrow().contains(&"bad stop".into()));
    assert!(log.borrow().contains(&"bad destroy".into()));
    assert!(!log.borrow().contains(&"old stop".into()));
}
#[test]
fn cleanup_error_does_not_skip_destroy_or_new_scene() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut m = SceneManager::new();
    register(&mut m, "a", log.clone(), false, true);
    register(&mut m, "b", log.clone(), false, false);
    m.switch_to("a").unwrap();
    m.switch_to("b").unwrap();
    assert_eq!(m.active_name(), Some("b"));
    assert!(log.borrow().contains(&"a destroy".into()));
    assert!(
        m.drain_events()
            .any(|e| e.phase == "stop" && e.error.is_some())
    );
}
#[test]
fn unknown_target_does_not_tear_down_running_scene() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut m = SceneManager::new();
    register(&mut m, "a", log.clone(), false, false);
    m.switch_to("a").unwrap();
    let old = m.active_id();
    assert!(m.switch_to("missing").is_err());
    assert_eq!(m.active_id(), old);
    assert_eq!(log.borrow().len(), 1);
}
#[cfg(feature = "plugin")]
#[test]
fn core_scope_releases_plugin_resources_before_scene_destroy() {
    use charon_core::{App, Plugin, PluginResult, World};
    use charon_scene::integration::*;
    struct Resource(Rc<RefCell<u32>>);
    impl Drop for Resource {
        fn drop(&mut self) {
            *self.0.borrow_mut() += 1;
        }
    }
    struct P(Rc<RefCell<u32>>);
    impl Plugin for P {
        fn name(&self) -> &'static str {
            "owned"
        }
        fn init(&mut self, w: &mut World) -> PluginResult {
            w.insert(Resource(self.0.clone()));
            Ok(())
        }
        fn shutdown(&mut self, w: &mut World) -> PluginResult {
            w.remove::<Resource>();
            Ok(())
        }
    }
    struct Logic;
    impl SceneLogic for Logic {
        fn destroy(&mut self, w: &mut World) -> SceneResult {
            assert!(w.get::<Resource>().is_none());
            Ok(())
        }
    }
    let drops = Rc::new(RefCell::new(0));
    let count = drops.clone();
    let mut m = SceneManager::new();
    m.register("a", move |_| {
        let mut app = App::new();
        app.add_plugin(P(count.clone())).unwrap();
        Ok(Box::new(ScopedScene::new(app, Logic)))
    })
    .unwrap();
    m.switch_to("a").unwrap();
    m.switch_to("a").unwrap();
    assert_eq!(*drops.borrow(), 1);
    m.shutdown().unwrap();
    assert_eq!(*drops.borrow(), 2);
}
#[test]
fn update_switch_is_applied_once_at_boundary_without_old_catchup_in_new_scene() {
    struct A(Rc<RefCell<u32>>);
    impl Scene for A {
        fn fixed_update(&mut self, _: Step) -> Result<Directive, String> {
            *self.0.borrow_mut() += 1;
            Ok(Directive::Switch("b".into()))
        }
    }
    struct B(Rc<RefCell<u32>>);
    impl Scene for B {
        fn fixed_update(&mut self, _: Step) -> Result<Directive, String> {
            *self.0.borrow_mut() += 1;
            Ok(Directive::Continue)
        }
    }
    let old = Rc::new(RefCell::new(0));
    let new = Rc::new(RefCell::new(0));
    let mut m = SceneManager::new();
    let o = old.clone();
    m.register("a", move |_| Ok(Box::new(A(o.clone()))))
        .unwrap();
    let n = new.clone();
    m.register("b", move |_| Ok(Box::new(B(n.clone()))))
        .unwrap();
    m.switch_to("a").unwrap();
    m.advance(Duration::from_secs(1)).unwrap();
    assert_eq!(*old.borrow(), 1);
    assert_eq!(*new.borrow(), 0);
    assert_eq!(m.active_name(), Some("b"));
    m.advance(Duration::from_millis(8)).unwrap();
    assert_eq!(*new.borrow(), 1);
}
#[test]
fn failed_factory_keeps_current_scene_and_reports_create_error() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut m = SceneManager::new();
    register(&mut m, "old", log, false, false);
    m.register("bad", |_| Err("factory failed".into())).unwrap();
    m.switch_to("old").unwrap();
    let id = m.active_id();
    assert!(m.switch_to("bad").is_err());
    assert_eq!(m.active_id(), id);
    assert!(
        m.drain_events()
            .any(|e| e.phase == "create" && e.error.is_some())
    );
}
