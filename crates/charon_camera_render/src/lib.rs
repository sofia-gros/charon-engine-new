//! Optional transform/viewport adapter for generic packets, not world structures.
use charon_camera::{Camera2D, Rect};
use charon_render::{DrawFrame, PassId};
pub fn apply(frame: &mut DrawFrame, camera: Camera2D, passes: &[PassId]) -> Result<(), String> {
    // Prepare before mutating so an invalid packet cannot partially transform a frame.
    let transformed: Result<Vec<_>, String> = frame
        .quads
        .iter()
        .filter(|q| passes.contains(&q.pass))
        .map(|q| {
            camera
                .project_rect(Rect {
                    x: q.rect[0],
                    y: q.rect[1],
                    width: q.rect[2],
                    height: q.rect[3],
                })
                .map(Rect::array)
        })
        .collect();
    let transformed = transformed?;
    let v = camera.viewport();
    let x = v.x.floor().clamp(0., frame.size[0] as f32) as u32;
    let y = v.y.floor().clamp(0., frame.size[1] as f32) as u32;
    let right = (v.x + v.width).ceil().clamp(0., frame.size[0] as f32) as u32;
    let bottom = (v.y + v.height).ceil().clamp(0., frame.size[1] as f32) as u32;
    for (quad, rect) in frame
        .quads
        .iter_mut()
        .filter(|q| passes.contains(&q.pass))
        .zip(transformed)
    {
        quad.rect = rect;
    }
    for &id in passes {
        frame.clips.insert(
            id,
            [x, y, right.saturating_sub(x), bottom.saturating_sub(y)],
        );
    }
    Ok(())
}
