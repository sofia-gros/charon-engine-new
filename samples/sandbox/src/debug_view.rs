//! Sample-only ASCII debug display; normal game text/layout remains separate work.
use crate::game::Frame;
use charon_render::OverlayRect;
use charon_sand::{AIR, SAND, WALL};
fn rect(x: f32, y: f32, width: f32, height: f32, color: [f32; 4]) -> OverlayRect {
    OverlayRect {
        x,
        y,
        width,
        height,
        color,
    }
}
fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 14],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        _ => [0; 7],
    }
}
fn text(rects: &mut Vec<OverlayRect>, x: f32, y: f32, value: &str, color: [f32; 4]) {
    for (i, c) in value.chars().enumerate() {
        for (row, bits) in glyph(c).into_iter().enumerate() {
            let mut col = 0;
            while col < 5 {
                if bits & (1 << (4 - col)) == 0 {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < 5 && bits & (1 << (4 - col)) != 0 {
                    col += 1;
                }
                rects.push(rect(
                    x + (i * 6 + start) as f32,
                    y + row as f32,
                    (col - start) as f32,
                    1.0,
                    color,
                ));
            }
        }
    }
}
pub fn overlays(frame: &Frame, inspect_enabled: bool) -> Vec<OverlayRect> {
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
    if frame.paused || inspect_enabled {
        result.push(rect(2., 2., 220., 21., [0.03, 0.06, 0.1, 1.]));
        text(
            &mut result,
            4.,
            4.,
            &format!(
                "{} TICK {:06}",
                if frame.paused { "PAUSED" } else { "RUNNING" },
                frame.simulation_tick
            ),
            accent,
        );
        let detail = frame.inspection.map_or_else(
            || "P PAUSE N STEP I INSPECT".into(),
            |cell| {
                let material = match cell.material {
                    Some(AIR) => "AIR",
                    Some(SAND) => "SAND",
                    Some(WALL) => "WALL",
                    _ => "OUTSIDE",
                };
                let awake = match cell.awake {
                    Some(true) => "AWAKE",
                    Some(false) => "SLEEP",
                    None => "",
                };
                format!("CELL {} {} {} {}", cell.x, cell.y, material, awake)
            },
        );
        text(&mut result, 4., 14., &detail, [0.9, 0.95, 1., 1.]);
    }
    result
}
