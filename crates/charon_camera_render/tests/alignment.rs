use charon_camera::*;
use charon_render::*;
#[test]
fn tile_sprite_particle_and_light_domain_use_the_same_transform_and_ui_is_unchanged() {
    let c = Camera2D::new(
        [160., 96.],
        4.,
        Rect {
            x: 20.,
            y: 72.,
            width: 800.,
            height: 500.,
        },
    )
    .unwrap();
    let mut frame = DrawFrame::new([960, 696]);
    for (pass, rect) in [
        (PassId(1), [104., 80., 8., 8.]),
        (PassId(2), [104., 80., 8., 8.]),
        (PassId(0), [20., 20., 8., 8.]),
    ] {
        frame.quads.push(Quad {
            rect,
            uv: [0., 0., 1., 1.],
            tint: [1.; 4],
            texture: TextureId(0),
            layer: 0,
            pass,
        });
    }
    charon_camera_render::apply(&mut frame, c, &[PassId(1), PassId(2)]).unwrap();
    assert_eq!(frame.quads[0].rect, frame.quads[1].rect);
    assert_eq!(frame.quads[2].rect, [20., 20., 8., 8.]);
    assert_eq!(frame.clips[&PassId(1)], [20, 72, 800, 500]);
    let point = c.world_to_screen([108., 84.]).unwrap();
    let domain = c
        .project_rect(Rect {
            x: 0.,
            y: 0.,
            width: 320.,
            height: 192.,
        })
        .unwrap();
    let uv = [
        (point[0] - domain.x) / domain.width,
        (point[1] - domain.y) / domain.height,
    ];
    assert!((uv[0] - 108. / 320.).abs() < 0.0001);
    assert!((uv[1] - 84. / 192.).abs() < 0.0001);
}
#[test]
fn failed_transform_leaves_frame_unchanged() {
    let c = Camera2D::new(
        [0.; 2],
        1.,
        Rect {
            x: 0.,
            y: 0.,
            width: 100.,
            height: 100.,
        },
    )
    .unwrap();
    let mut f = DrawFrame::new([100, 100]);
    f.quads.push(Quad {
        rect: [1., 2., -1., 3.],
        uv: [0., 0., 1., 1.],
        tint: [1.; 4],
        texture: TextureId(0),
        layer: 0,
        pass: PassId(1),
    });
    let before = f.quads[0].rect;
    assert!(charon_camera_render::apply(&mut f, c, &[PassId(1)]).is_err());
    assert_eq!(f.quads[0].rect, before);
    assert!(f.clips.is_empty());
}
