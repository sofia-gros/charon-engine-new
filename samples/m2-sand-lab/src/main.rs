mod draw;
mod runtime;
use charon_physics::sim::material::Cell;
use runtime::{Controls, Lab, WIDTH};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--smoke-test") {
        return smoke_test();
    }
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        return snapshot(args.get(i + 1).ok_or("--snapshot requires a path")?);
    }
    let hz = if let Some(i) = args.iter().position(|a| a == "--compute-hz") {
        args.get(i + 1)
            .ok_or("--compute-hz requires a rate")?
            .parse::<u32>()?
    } else {
        45
    };
    if !(30..=60).contains(&hz) {
        return Err("compute rate must be 30..=60 Hz".into());
    }
    #[cfg(feature = "window")]
    return window(hz);
    #[cfg(not(feature = "window"))]
    Err("window feature disabled; use --smoke-test".into())
}

#[cfg(feature = "window")]
fn window(hz: u32) -> Result<(), Box<dyn std::error::Error>> {
    use charon_physics::sim::material::CellData;
    use minifb::{Key, KeyRepeat, MouseButton, MouseMode, Window, WindowOptions};
    use runtime::Brush;
    let mut lab = Lab::start(hz)?;
    let mut window = Window::new(
        "Charon M2 / Falling Sand Lab",
        draw::WINDOW_W,
        draw::WINDOW_H,
        WindowOptions::default(),
    )?;
    window.set_target_fps(60);
    let mut canvas = draw::Canvas::new();
    let mut cell = CellData::sand(17);
    let mut radius = 4u16;
    while window.is_open() && !window.is_key_down(Key::Escape) {
        lab.check()?;
        let active = window.is_active();
        if active {
            for (key, material) in [
                (Key::Key1, CellData::sand(17)),
                (Key::Key2, CellData::water(31)),
                (Key::Key3, CellData::stone(0)),
                (Key::Key4, CellData::gravel(0)),
                (Key::Key5, CellData::fire(100)),
                (Key::Key6, CellData::oil(0)),
                (Key::Key7, CellData::acid(120)),
                (Key::Key8, CellData::steam(80)),
            ] {
                if window.is_key_pressed(key, KeyRepeat::No) {
                    cell = material;
                }
            }
            for (key, name) in [(Key::Key9, "Soil"), (Key::C, "Cloud"), (Key::N, "Nitrogen")] {
                if window.is_key_pressed(key, KeyRepeat::No)
                    && let Some(view) = lab
                        .client
                        .snapshot()
                        .palette
                        .iter()
                        .find(|view| view.name == name)
                {
                    cell = CellData::new(view.kind, 17);
                }
            }
            if let Some((_, scroll)) = window.get_scroll_wheel() {
                radius = (radius as f32 + scroll).clamp(1.0, 12.0) as u16;
            }
        }
        let brush = if active
            && (window.get_mouse_down(MouseButton::Left)
                || window.get_mouse_down(MouseButton::Right))
        {
            window
                .get_mouse_pos(MouseMode::Discard)
                .and_then(|(mx, my)| {
                    let (px, py) = (mx as i32 - draw::LEFT, my as i32 - draw::TOP);
                    (px >= 0
                        && py >= 0
                        && px < WIDTH as i32 * draw::SCALE
                        && py < runtime::HEIGHT as i32 * draw::SCALE)
                        .then_some(Brush {
                            x: px / draw::SCALE,
                            y: py / draw::SCALE,
                            radius,
                            cell: if window.get_mouse_down(MouseButton::Right) {
                                CellData::AIR
                            } else {
                                cell
                            },
                        })
                })
        } else {
            None
        };
        lab.input(Controls {
            brush,
            reset: active && window.is_key_pressed(Key::R, KeyRepeat::No),
            pause: active && window.is_key_pressed(Key::P, KeyRepeat::No),
            gate: active && window.is_key_pressed(Key::G, KeyRepeat::No),
            release: active && window.is_key_pressed(Key::B, KeyRepeat::No),
            angle: (active && window.is_key_pressed(Key::A, KeyRepeat::No)).then_some(cell.kind),
        })?;
        let latest = lab.client.snapshot();
        window.set_title(&format!(
            "Charon M2 / Falling Sand Lab | {}",
            latest.palette[usize::from(cell.kind.id())].name
        ));
        canvas.render(&latest, &lab.status(), hz, cell.kind, radius);
        window.update_with_buffer(&canvas.pixels, draw::WINDOW_W, draw::WINDOW_H)?;
    }
    lab.shutdown()
}

fn smoke_test() -> Result<(), Box<dyn std::error::Error>> {
    for hz in [30, 60] {
        let mut lab = Lab::start(hz)?;
        let before = lab
            .client
            .snapshot()
            .cells
            .iter()
            .filter(|c| c.kind == Cell::Sand)
            .count();
        lab.input(Controls {
            release: true,
            ..Controls::default()
        })?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            lab.check()?;
            let snapshot = lab.client.snapshot();
            if !snapshot.colliders.contains(&(105 * WIDTH + 160)) {
                let after = snapshot
                    .cells
                    .iter()
                    .filter(|c| c.kind == Cell::Sand)
                    .count();
                if after != before + 190 {
                    return Err("barrier conversion lost or duplicated sand".into());
                }
                break;
            }
            if Instant::now() > deadline {
                return Err("timed out waiting for sand worker".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut canvas = draw::Canvas::new();
        canvas.render(&lab.client.snapshot(), &lab.status(), hz, Cell::Sand, 4);
        lab.shutdown()?;
        println!(
            "M2 {hz} Hz: commands, collider release, mass conservation, snapshot and shutdown OK"
        );
    }
    Ok(())
}

fn snapshot(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let mut sim = runtime::initial_sim();
    for _ in 0..100 {
        sim.step();
    }
    let mut canvas = draw::Canvas::new();
    canvas.render(
        &sim.snapshot(),
        &runtime::Status::default(),
        45,
        Cell::Sand,
        4,
    );
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(file, "P6\n{} {}\n255\n", draw::WINDOW_W, draw::WINDOW_H)?;
    for pixel in canvas.pixels {
        file.write_all(&[(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8])?;
    }
    file.flush()?;
    Ok(())
}
