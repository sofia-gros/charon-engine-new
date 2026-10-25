use charon_camera::*;
fn camera() -> Camera2D {
    Camera2D::new(
        [17., -13.],
        2.5,
        Rect {
            x: 31.,
            y: 72.,
            width: 800.,
            height: 500.,
        },
    )
    .unwrap()
}
fn close(a: [f32; 2], b: [f32; 2]) {
    for i in 0..2 {
        assert!((a[i] - b[i]).abs() < 0.0001, "{a:?} != {b:?}");
    }
}
#[test]
fn inverse_projection_survives_pan_zoom_and_offset_viewport() {
    let mut c = camera();
    c.pan([123., -34.]).unwrap();
    c.set_zoom(7.2).unwrap();
    for p in [[0., 0.], [108., 84.], [-20., 300.]] {
        close(c.screen_to_world(c.world_to_screen(p).unwrap()).unwrap(), p);
    }
    assert!(c.pick([0., 0.]).is_none());
    assert!(c.pick([400., 200.]).is_some());
}
#[test]
fn zoom_anchor_keeps_world_under_pointer_and_resize_keeps_center() {
    let mut c = camera();
    let anchor = [250., 300.];
    let before = c.screen_to_world(anchor).unwrap();
    c.zoom_at(5., anchor).unwrap();
    close(c.screen_to_world(anchor).unwrap(), before);
    let center = c.center();
    c.set_viewport(Rect {
        x: 0.,
        y: 72.,
        width: 640.,
        height: 360.,
    })
    .unwrap();
    close(c.center(), center);
}
#[test]
fn fit_and_zoom_limits_reject_invalid_changes_atomically() {
    let mut c = camera();
    c.set_zoom_limits(0.5, 10.).unwrap();
    c.set_zoom(100.).unwrap();
    assert_eq!(c.zoom(), 10.);
    let before = c;
    assert!(c.zoom_at(f32::NAN, [0., 0.]).is_err());
    assert_eq!(c, before);
    assert!(
        c.set_viewport(Rect {
            x: 0.,
            y: 0.,
            width: 0.,
            height: 1.
        })
        .is_err()
    );
    c.fit(Rect {
        x: 0.,
        y: 0.,
        width: 320.,
        height: 200.,
    })
    .unwrap();
    assert_eq!(c.center(), [160., 100.]);
}
