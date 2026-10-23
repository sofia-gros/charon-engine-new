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
) -> Vec<OverlayRect> {
    let mut result = Vec::new();
    if let Some(body) = frame.player {
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
    result
}
