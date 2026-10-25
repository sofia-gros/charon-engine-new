//! Sample-only software view. This is not the future engine Rendering Plugin.
use crate::game::{COLS, Game, Glow, ROWS, Score, Status, required, trace, wall};
use charon_core::{Plugin, PluginResult, Step, World};

pub const WIDTH: usize = 960;
pub const HEIGHT: usize = 704;
const TILE: i32 = 24;
const LEFT: i32 = 48;
const TOP: i32 = 112;

pub struct Canvas {
    pub pixels: Vec<u32>,
}
impl Default for Canvas {
    fn default() -> Self {
        Self {
            pixels: vec![0; WIDTH * HEIGHT],
        }
    }
}

impl Canvas {
    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        for py in y.max(0)..(y + h).min(HEIGHT as i32) {
            for px in x.max(0)..(x + w).min(WIDTH as i32) {
                self.pixels[py as usize * WIDTH + px as usize] = color;
            }
        }
    }

    fn text(&mut self, x: i32, y: i32, text: &str, scale: i32, color: u32) {
        for (i, letter) in text.chars().enumerate() {
            for (row, bits) in glyph(letter).iter().enumerate() {
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

    fn tile(&mut self, position: (i32, i32), inset: i32, color: u32) {
        self.rect(
            LEFT + position.0 * TILE + inset,
            TOP + position.1 * TILE + inset,
            TILE - inset * 2,
            TILE - inset * 2,
            color,
        );
    }
}

pub struct ViewPlugin {
    sources: &'static [&'static str],
}
impl ViewPlugin {
    pub fn local() -> Self {
        Self {
            sources: &["game", "score"],
        }
    }
    pub fn snapshot() -> Self {
        Self {
            sources: &["snapshot"],
        }
    }
}
impl Plugin for ViewPlugin {
    fn name(&self) -> &'static str {
        "view"
    }
    fn dependencies(&self) -> &[&'static str] {
        self.sources
    }
    fn init(&mut self, world: &mut World) -> PluginResult {
        world.insert(Canvas::default());
        trace(world, "view:init");
        Ok(())
    }
    fn frame_update(&mut self, world: &mut World, step: Step) -> PluginResult {
        let mut canvas = world.remove::<Canvas>().ok_or("canvas missing")?;
        render(world, &mut canvas, step)?;
        world.insert(canvas);
        Ok(())
    }
    fn shutdown(&mut self, world: &mut World) -> PluginResult {
        world.remove::<Canvas>();
        trace(world, "view:shutdown");
        Ok(())
    }
}

fn render(world: &World, c: &mut Canvas, step: Step) -> PluginResult {
    let game = required::<Game>(world)?;
    let score = required::<Score>(world)?.0;
    let glow = world.get::<Glow>();
    c.pixels.fill(0x0b1020);
    c.rect(0, 0, WIDTH as i32, 4, 0x56e5b2);
    c.text(48, 24, "CHARON / GEM RUN", 3, 0xe8efff);
    c.text(48, 62, "M1 PLUGIN LAB", 2, 0x56e5b2);
    c.text(
        500,
        28,
        &format!("GEMS {score}/6   HP {}", game.health),
        2,
        0xe8efff,
    );
    c.text(
        500,
        62,
        &format!(
            "TIME {:02}   TICK {}",
            game.remaining.ceil() as u32,
            game.ticks
        ),
        2,
        0x92a5ca,
    );

    for y in 0..ROWS {
        for x in 0..COLS {
            c.tile(
                (x, y),
                1,
                if wall(x, y) {
                    0x344766
                } else if (x + y) % 2 == 0 {
                    0x141d30
                } else {
                    0x182236
                },
            );
            if wall(x, y) {
                c.rect(
                    LEFT + x * TILE + 2,
                    TOP + y * TILE + 2,
                    TILE - 4,
                    2,
                    0x456186,
                );
            }
        }
    }
    for &gem in &game.gems {
        if let Some(glow) = glow {
            let value = 30 + glow.0;
            c.tile(gem, 1, (value << 16) | ((value + 25) << 8) | 75);
        }
        c.tile(gem, 6, 0xffd778);
        c.tile(gem, 9, 0xfff0b4);
    }
    c.tile(game.hazard(), 3, 0xf56584);
    c.tile(game.hazard(), 8, 0x992e56);
    c.tile(game.player, 3, 0x56e5b2);
    c.tile(game.player, 8, 0xd6fff0);

    c.text(
        48,
        612,
        "WASD / ARROWS MOVE   P PAUSE   R RESTART   ESC QUIT",
        2,
        0xc4d0e9,
    );
    let plugins = if world.get::<crate::threaded::Metrics>().is_some() {
        "GAME + SCORE / SNAPSHOT / VIEW"
    } else if glow.is_some() {
        "GAME > SCORE > PULSE > VIEW"
    } else {
        "GAME > SCORE > VIEW / PULSE OFF"
    };
    c.text(48, 641, plugins, 2, 0x56e5b2);
    c.text(
        48,
        674,
        &format!("TARGET GAME 144 HZ / RENDER 60 HZ / FRAME {}", step.index),
        1,
        0x92a5ca,
    );
    if let Some(stats) = world.get::<crate::threaded::Metrics>() {
        c.text(
            48,
            689,
            &format!(
                "COMPUTE TARGET {} HZ / STEPS {} / GAME STEPS {}",
                stats.hz, stats.steps, stats.game_steps
            ),
            1,
            0x92a5ca,
        );
    }
    if game.paused || game.status != Status::Playing {
        c.rect(230, 275, 500, 140, 0x0b1020);
        c.rect(230, 275, 500, 3, 0x56e5b2);
        let message = match game.status {
            Status::Won => "ALL GEMS FOUND!",
            Status::Lost => "TRY AGAIN!",
            Status::Playing => "PAUSED",
        };
        c.text(270, 305, message, 3, 0xffd778);
        let controls = if game.status == Status::Playing {
            "P RESUME / R RESTART"
        } else {
            "R RESTART / ESC QUIT"
        };
        c.text(270, 365, controls, 2, 0xc4d0e9);
    }
    Ok(())
}

// Built-in 5x7 glyphs: no external font, image, or asset runtime required.
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
