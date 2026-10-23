//! Small ASCII diagnostics, independent of simulation plugins.
use crate::OverlayRect;
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
        '.' => [0, 0, 0, 0, 0, 6, 6],
        ':' => [0, 6, 6, 0, 6, 6, 0],
        '_' => [0, 0, 0, 0, 0, 0, 31],
        _ => [0; 7],
    }
}
pub fn append_debug_text(
    rects: &mut Vec<OverlayRect>,
    x: f32,
    y: f32,
    value: &str,
    color: [f32; 4],
) {
    for (i, c) in value.chars().enumerate() {
        for (row, bits) in glyph(c.to_ascii_uppercase()).into_iter().enumerate() {
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

/// CPU-side present-submission and simulation rates measured by the sample.
#[derive(Clone, Copy)]
pub struct DebugPerformance {
    pub draw_fps: f64,
    pub simulation_hz: f64,
}
pub fn append_performance_overlay(
    rects: &mut Vec<OverlayRect>,
    metrics: Option<DebugPerformance>,
    software: bool,
) {
    rects.push(rect(2., 2., 270., 21., [0.03, 0.06, 0.1, 1.]));
    let rates = metrics.map_or_else(
        || "DRAW WARMUP SIM WARMUP".into(),
        |m| format!("DRAW {:.0} FPS SIM {:.0} HZ", m.draw_fps, m.simulation_hz),
    );
    append_debug_text(rects, 4., 4., &rates, [0.9, 0.95, 1., 1.]);
    let environment = format!(
        "{} {} {}",
        std::env::consts::OS.to_ascii_uppercase(),
        if software { "SOFTWARE GPU" } else { "GPU" },
        if cfg!(debug_assertions) {
            "DEBUG"
        } else {
            "RELEASE"
        }
    );
    append_debug_text(rects, 4., 14., &environment, [0.65, 0.75, 0.9, 1.]);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn material_keys_with_lowercase_letters_are_visible() {
        let mut lowercase = vec![];
        let mut uppercase = vec![];
        append_debug_text(&mut lowercase, 0., 0., "demo.ash", [1.; 4]);
        append_debug_text(&mut uppercase, 0., 0., "DEMO.ASH", [1.; 4]);
        assert!(
            !lowercase.is_empty(),
            "material inspection label disappeared"
        );
        assert!(lowercase == uppercase);
    }
}
