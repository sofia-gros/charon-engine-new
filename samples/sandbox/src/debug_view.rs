//! Sample-only ASCII debug display; normal game text/layout remains separate work.
use crate::game::Frame;
use charon_render::{
    append_debug_text as text, append_performance_overlay, DebugPerformance, OverlayRect,
};
fn rect(x: f32, y: f32, width: f32, height: f32, color: [f32; 4]) -> OverlayRect {
    OverlayRect {
        x,
        y,
        width,
        height,
        color,
    }
}
pub fn overlays(
    frame: &Frame,
    inspect_enabled: bool,
    metrics: Option<DebugPerformance>,
    software: bool,
    camera: charon_render::Camera2D,
    tool_label: &str,
    preview: Option<[f32; 4]>,
) -> Vec<OverlayRect> {
    let mut result = Vec::new();
    if let Some(body) = frame.player.filter(|body| body.embedded) {
        result.push(rect(
            body.x(),
            body.y(),
            body.width(),
            body.height(),
            if body.embedded {
                [1., 0.2, 0.2, 1.]
            } else {
                [0.15, 0.8, 1., 1.]
            },
        ));
        result.push(rect(
            body.x() + 4.,
            body.y() + 2.,
            2.,
            2.,
            [0.05, 0.1, 0.2, 1.],
        ));
    }
    let accent = if frame.paused {
        [1., 0.75, 0.2, 1.]
    } else {
        [0.3, 1., 0.5, 1.]
    };
    if let Some(cell) = frame.inspection.filter(|cell| cell.material.is_some()) {
        let (x, y) = (cell.x as f32, cell.y as f32);
        result.extend([
            rect(x, y, 1., 0.2, accent),
            rect(x, y + 0.8, 1., 0.2, accent),
            rect(x, y, 0.2, 1., accent),
            rect(x + 0.8, y, 0.2, 1., accent),
        ]);
    }
    for &[x, y] in &frame.shots {
        result.push(rect(x - 2., y, 3., 0.7, [1., 0.95, 0.3, 1.]));
    }
    for &[x, y] in &frame.sparks {
        result.push(rect(x, y, 0.8, 0.8, [1., 0.5, 0.15, 1.]));
    }
    if let Some([x, y, w, h]) = preview {
        let color = [0.5, 1., 0.65, 1.];
        let line = 0.2;
        result.extend([
            rect(x, y, w, line, color),
            rect(x, y + h - line, w, line, color),
            rect(x, y, line, h, color),
            rect(x + w - line, y, line, h, color),
        ]);
    }
    // World geometry follows the camera; the HUD appended below remains fixed.
    for rect in &mut result {
        let viewport = [frame.terrain.width as f32, frame.terrain.height as f32];
        let point = camera.world_to_view([rect.x, rect.y], viewport);
        rect.x = point[0];
        rect.y = point[1];
        rect.width *= viewport[0] / camera.size()[0];
        rect.height *= viewport[1] / camera.size()[1];
    }
    append_performance_overlay(&mut result, metrics, software);
    {
        result.push(rect(
            2.,
            23.,
            270.,
            if frame.paused || inspect_enabled {
                21.
            } else {
                11.
            },
            [0.03, 0.06, 0.1, 1.],
        ));
        text(
            &mut result,
            4.,
            25.,
            &format!(
                "{} TICK {:06}",
                if frame.paused { "PAUSED" } else { "RUNNING" },
                frame.simulation_tick
            ),
            accent,
        );
        let detail = frame.inspection.map_or_else(
            || "1 SAND 2 ASH P PAUSE I INSPECT".into(),
            |cell| {
                let material = cell
                    .material
                    .and_then(|id| frame.terrain.materials.get(id))
                    .map_or("OUTSIDE", |m| m.key.as_str());
                let awake = match cell.awake {
                    Some(true) => "AWAKE",
                    Some(false) => "SLEEP",
                    None => "",
                };
                format!("CELL {} {} {} {}", cell.x, cell.y, material, awake)
            },
        );
        if frame.paused || inspect_enabled {
            text(&mut result, 4., 35., &detail, [0.9, 0.95, 1., 1.]);
        }
    }
    result.push(rect(2., 45., 270., 11., [0.03, 0.06, 0.1, 1.]));
    text(&mut result, 4., 47., tool_label, [0.7, 1., 0.8, 1.]);
    if let Some(error) = frame.image_error {
        result.push(rect(2., 57., 270., 11., [0.1, 0.03, 0.03, 1.]));
        text(&mut result, 4., 59., error, [1., 0.4, 0.4, 1.]);
    }
    result
}
