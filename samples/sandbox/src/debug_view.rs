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
    let hud_start = result.len();
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
    text(
        &mut result,
        4.,
        47.,
        if frame.cup.is_some_and(|c| !c.movable) {
            "H DAM K CUP P PAUSE N STEP"
        } else if frame.cup.is_some() {
            "K CUP A/D MOVE O ADD OIL P PAUSE"
        } else {
            tool_label
        },
        [0.7, 1., 0.8, 1.],
    );
    result.push(rect(2., 57., 270., 11., [0.03, 0.06, 0.1, 1.]));
    let counts = frame.cup.map_or_else(
        || {
            format!(
                "SAND {} LIQUID {} ACTIVE {}",
                frame.terrain.sand, frame.terrain.liquid, frame.terrain.active_tiles
            )
        },
        |c| format!("WATER {} OIL {} MOVE {}", c.water, c.oil, c.moves),
    );
    text(&mut result, 4., 59., &counts, [0.75, 0.9, 1., 1.]);
    if let Some(cup) = frame.cup {
        result.push(rect(2., 81., 300., 11., [0.03, 0.06, 0.1, 1.]));
        text(
            &mut result,
            4.,
            83.,
            &format!(
                "{} MAC/VOF DIV {:.1e} SUB {} TILT {:+.1}",
                cup.compute_backend,
                cup.fluid.max_divergence,
                cup.fluid.substeps,
                cup.surface_delta
            ),
            [0.75, 0.9, 1., 1.],
        );
    }
    if let Some(gpu) = frame.cup.and_then(|c| c.fluid.gpu) {
        let label = match (gpu.velocity_ms, gpu.pressure_ms) {
            (Some(v), Some(p)) => format!("GPU MS VELOCITY {v:.2} PRESSURE {p:.2}"),
            _ => "GPU TIMER N/A".into(),
        };
        result.push(rect(2., 69., 300., 11., [0.03, 0.06, 0.1, 1.]));
        text(&mut result, 4., 71., &label, [0.75, 0.9, 1., 1.]);
    }
    if let Some(error) = frame.image_error {
        result.push(rect(2., 69., 270., 11., [0.1, 0.03, 0.03, 1.]));
        text(&mut result, 4., 71., error, [1., 0.4, 0.4, 1.]);
    }
    // HUD uses a stable 480x270 design canvas; simulation-cell density and
    // camera zoom must not change the size of screen-space text.
    let scale = [
        frame.terrain.width as f32 / 480.,
        frame.terrain.height as f32 / 270.,
    ];
    for rect in &mut result[hud_start..] {
        rect.x *= scale[0];
        rect.width *= scale[0];
        rect.y *= scale[1];
        rect.height *= scale[1];
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_runtime::Simulation;
    #[test]
    fn hud_keeps_its_screen_size_when_simulation_density_changes() {
        let render = |w, h| {
            let mut game = crate::game::Game::new(w, h, 1, false).unwrap();
            let mut frame = game.step(vec![]);
            frame.terrain.sand = 0;
            frame.terrain.liquid = 0;
            frame.terrain.active_tiles = 0;
            let camera = charon_render::Camera2D::new([0., 0.], [w as f32, h as f32]).unwrap();
            overlays(&frame, false, None, true, camera, "TOOLS", None)
                .into_iter()
                .map(|r| {
                    [
                        r.x * 1920. / w as f32,
                        r.y * 1080. / h as f32,
                        r.width * 1920. / w as f32,
                        r.height * 1080. / h as f32,
                    ]
                })
                .collect::<Vec<_>>()
        };
        let regular = render(480, 270);
        let small = render(128, 100);
        assert!(!regular.is_empty());
        assert_eq!(regular.len(), small.len());
        for (a, b) in regular.iter().zip(small) {
            for (a, b) in a.iter().zip(b) {
                assert!((a - b).abs() < 1e-3);
            }
        }
    }
}
