//! Deliberately small ASCII presentation for the composition example; not a text engine.
use charon_dialogue::{Frame, Page};
const W: usize = 640;
const H: usize = 360;
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
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        ':' => [0, 4, 4, 0, 4, 4, 0],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        _ => [0; 7],
    }
}
fn rect(image: &mut [u8], x: usize, y: usize, w: usize, h: usize, color: [u8; 4]) {
    for py in y..(y + h).min(H) {
        for px in x..(x + w).min(W) {
            image[(py * W + px) * 4..(py * W + px + 1) * 4].copy_from_slice(&color);
        }
    }
}
fn text(image: &mut [u8], x: usize, y: usize, content: &str, color: [u8; 4]) {
    let (mut px, mut py) = (x, y);
    for c in content.chars() {
        if c == '\n' {
            px = x;
            py += 24;
            continue;
        }
        for (row, bits) in glyph(c).into_iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    rect(image, px + col * 2, py + row * 2, 2, 2, color);
                }
            }
        }
        px += 12;
    }
}
pub fn draw(frame: &Frame) -> Vec<u8> {
    let mut image = vec![0; W * H * 4];
    for y in 0..H {
        rect(
            &mut image,
            0,
            y,
            W,
            1,
            [
                14 + (y / 20) as u8,
                20 + (y / 16) as u8,
                40 + (y / 10) as u8,
                255,
            ],
        );
    }
    let accent = match frame.page {
        Page::River => [95, 199, 211, 255],
        Page::Hill => [167, 204, 128, 255],
        _ => [231, 186, 110, 255],
    };
    rect(&mut image, 36, 38, 4, 34, accent);
    text(&mut image, 54, 40, "CHARON", accent);
    text(
        &mut image,
        54,
        62,
        "A CROSSROADS STORY",
        [190, 199, 214, 255],
    );
    // Simple scenery is intentionally independent of cell simulation.
    for x in 0..W {
        let hill = 140 + ((x as isize - 400).unsigned_abs() / 7).min(70);
        rect(&mut image, x, hill, 1, 90, [35, 53, 72, 255]);
    }
    rect(&mut image, 30, 205, 580, 130, [17, 25, 40, 255]);
    rect(&mut image, 30, 205, 580, 2, accent);
    text(&mut image, 48, 224, frame.text, [229, 234, 242, 255]);
    text(&mut image, 48, 286, frame.prompt, accent);
    image
}
