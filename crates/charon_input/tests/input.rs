use charon_input::*;
fn press(b: Button, pressed: bool) -> InputEvent {
    InputEvent::Button { button: b, pressed }
}
#[test]
fn tap_repeat_and_tick_edges_are_distinct() {
    let mut s = InputState::default();
    let key = Key(65);
    s.begin_tick();
    s.apply(press(key.into(), true)).unwrap();
    s.apply(press(key.into(), true)).unwrap();
    assert!(s.held(key) && s.pressed(key));
    s.apply(press(key.into(), false)).unwrap();
    assert!(!s.held(key) && s.pressed(key) && s.released(key));
    s.begin_tick();
    assert!(!s.pressed(key) && !s.released(key));
}
#[test]
fn focus_loss_releases_mouse_and_keys_and_rejects_background_input() {
    let mut s = InputState::default();
    s.apply(press(Key(65).into(), true)).unwrap();
    s.apply(press(Button::Mouse(0), true)).unwrap();
    s.apply(InputEvent::Focus(false)).unwrap();
    assert!(!s.held(Key(65)) && !s.held(Button::Mouse(0)));
    assert!(s.released(Key(65)));
    s.apply(press(Key(65).into(), true)).unwrap();
    assert!(!s.held(Key(65)));
    assert!(s.pointer.is_none());
}
#[test]
fn wheel_is_accumulated_then_cleared_and_invalid_values_are_rejected() {
    let mut s = InputState::default();
    s.apply(InputEvent::Wheel([1., 2.])).unwrap();
    s.apply(InputEvent::Wheel([2., 3.])).unwrap();
    assert_eq!(s.wheel, [3., 5.]);
    s.begin_tick();
    assert_eq!(s.wheel, [0.; 2]);
    assert!(s.apply(InputEvent::Pointer([f32::NAN, 0.])).is_err());
}
#[test]
fn scene_barrier_suppresses_held_buttons_and_os_repeat_until_release() {
    let mut b = InputBarrier::default();
    let down = press(Key(65).into(), true);
    assert!(b.route(down).is_some());
    b.scene_changed();
    assert!(b.route(down).is_none());
    assert!(b.route(press(Key(65).into(), false)).is_none());
    assert!(b.route(down).is_some());
    b.scene_changed();
    b.route(InputEvent::Focus(false));
    assert!(b.route(down).is_some());
}
#[cfg(feature = "plugin")]
#[test]
fn plugin_backpressure_and_shutdown_are_explicit() {
    use charon_core::App;
    let (p, c) = InputPlugin::new(1).unwrap();
    let mut a = App::new();
    a.add_plugin(p).unwrap();
    a.start().unwrap();
    c.try_send(press(Key(65).into(), true)).unwrap();
    assert!(c.try_send(InputEvent::Focus(false)).is_err());
    a.tick().unwrap();
    assert!(c.snapshot().held(Key(65)));
    a.shutdown().unwrap();
    assert!(!c.snapshot().held(Key(65)));
    assert!(c.try_send(InputEvent::Focus(true)).is_err());
}
#[test]
fn repeat_does_not_rearm_keys_after_focus_or_scene_reset() {
    let mut s = InputState::default();
    let key = Key(65);
    s.apply(press(key.into(), true)).unwrap();
    s.begin_tick();
    s.apply(InputEvent::Repeat(key.into())).unwrap();
    assert!(s.held(key) && s.repeated(key) && !s.pressed(key));
    s.apply(InputEvent::Focus(false)).unwrap();
    s.apply(InputEvent::Focus(true)).unwrap();
    s.apply(InputEvent::Repeat(key.into())).unwrap();
    assert!(!s.held(key) && !s.pressed(key) && !s.repeated(key));
}
#[cfg(feature = "plugin")]
#[test]
fn disconnect_releases_pressed_input() {
    use charon_core::App;
    let (p, c) = InputPlugin::new(4).unwrap();
    let mut a = App::new();
    a.add_plugin(p).unwrap();
    a.start().unwrap();
    c.try_send(press(Key(65).into(), true)).unwrap();
    a.tick().unwrap();
    assert!(a.world().get::<InputState>().unwrap().held(Key(65)));
    drop(c);
    a.tick().unwrap();
    assert!(!a.world().get::<InputState>().unwrap().held(Key(65)));
    assert!(!a.world().get::<InputState>().unwrap().focused);
}
