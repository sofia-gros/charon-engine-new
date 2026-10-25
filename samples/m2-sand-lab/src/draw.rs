use crate::runtime::{HEIGHT, Status, WIDTH};
use charon_sand::Snapshot;
use sandengine::sim::material::Cell;
pub const WINDOW_W: usize = 1024;
pub const WINDOW_H: usize = 760;
pub const LEFT: i32 = 32;
pub const TOP: i32 = 88;
pub const SCALE: i32 = 3;
pub struct Canvas {
    pub pixels: Vec<u32>,
}
impl Canvas {
    pub fn new() -> Self {
        Self {
            pixels: vec![0; WINDOW_W * WINDOW_H],
        }
    }
    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        for py in y.max(0)..(y + h).min(WINDOW_H as i32) {
            for px in x.max(0)..(x + w).min(WINDOW_W as i32) {
                self.pixels[py as usize * WINDOW_W + px as usize] = color;
            }
        }
    }
    fn text(&mut self, x: i32, y: i32, text: &str, scale: i32, color: u32) {
        for (i, c) in text.chars().enumerate() {
            for (row, bits) in glyph(c).into_iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        self.rect(
                            x + i as i32 * 6 * scale + col * scale,
                            y + row as i32 * scale,
                            scale,
                            scale,
                            color,
                        );
                    }
                }
            }
        }
    }
    pub fn render(
        &mut self,
        snapshot: &Snapshot,
        status: &Status,
        hz: u32,
        material: Cell,
        radius: u16,
    ) {
        self.pixels.fill(0x0b1020);
        self.rect(0, 0, WINDOW_W as i32, 4, 0x56e5b2);
        self.text(32, 24, "CHARON / M2 SAND LAB", 3, 0xe8efff);
        self.text(
            520,
            24,
            &format!("{} / BRUSH {}", material.name(), radius).to_uppercase(),
            2,
            0xffd778,
        );
        self.text(
            32,
            61,
            &format!(
                "GAME 144 / RENDER 60 / COMPUTE {} HZ / TICK {}",
                hz, snapshot.ticks
            ),
            2,
            0x92a5ca,
        );
        self.rect(
            LEFT,
            TOP,
            WIDTH as i32 * SCALE,
            HEIGHT as i32 * SCALE,
            0x111827,
        );
        for (i, cell) in snapshot.cells.iter().enumerate() {
            if cell.kind == Cell::Air {
                continue;
            }
            let [r, g, b, _] = cell.to_rgba();
            let color = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
            self.rect(
                LEFT + (i % WIDTH) as i32 * SCALE,
                TOP + (i / WIDTH) as i32 * SCALE,
                SCALE,
                SCALE,
                color,
            );
        }
        for &i in &snapshot.colliders {
            self.rect(
                LEFT + (i % WIDTH) as i32 * SCALE,
                TOP + (i / WIDTH) as i32 * SCALE,
                SCALE,
                SCALE,
                0x6784a9,
            );
        }
        self.text(
            32,
            704,
            "1 SAND 2 WATER 3 STONE 4 GRAVEL 5 FIRE 6 OIL 7 ACID 8 STEAM",
            2,
            0xc4d0e9,
        );
        self.text(
            32,
            729,
            "LMB PAINT / RMB ERASE / WHEEL SIZE / P PAUSE / R RESET / G GATE / B BREAK",
            1,
            0x56e5b2,
        );
        let result = if snapshot.paused {
            "PAUSED"
        } else if status.queue_full {
            "COMMAND QUEUE FULL - RETRYING"
        } else {
            &status.last_result
        };
        self.text(
            32,
            746,
            &format!(
                "{} / ACTIVE {} / GAME STEPS {}",
                result, snapshot.active_chunks, status.game_steps
            ),
            1,
            0x92a5ca,
        );
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
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 2, 18, 12],
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
        '+' => [0, 4, 4, 31, 4, 4, 0],
        '/' => [1, 1, 2, 4, 8, 16, 16],
        '>' => [16, 8, 4, 2, 4, 8, 16],
        '!' => [4, 4, 4, 4, 4, 0, 4],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        _ => [0; 7],
    }
}
