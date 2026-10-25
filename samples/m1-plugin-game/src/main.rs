mod game;
mod threaded;
mod view;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--smoke-test") {
        game::smoke_test()?;
        return threaded::smoke_test();
    }
    if let Some(index) = args.iter().position(|arg| arg == "--snapshot") {
        let path = args.get(index + 1).ok_or("--snapshot needs a .ppm path")?;
        return snapshot(path, !args.iter().any(|arg| arg == "--no-effects"));
    }
    if args.iter().any(|arg| arg == "--help") {
        println!(
            "Charon M1 Gem Run\n--compute-hz N  compute rate 30..60 (default 45)\n--no-effects  omit the Pulse Plugin\n--snapshot PATH  save a PPM frame without a window\n--smoke-test  scripted check, no window\nWASD/arrows move, P pause, R restart, Esc quit"
        );
        return Ok(());
    }
    let compute_hz = if let Some(index) = args.iter().position(|arg| arg == "--compute-hz") {
        args.get(index + 1)
            .ok_or("--compute-hz needs a rate")?
            .parse::<u32>()?
    } else {
        45
    };
    if !(30..=60).contains(&compute_hz) {
        return Err("compute rate must be 30..=60 Hz".into());
    }
    #[cfg(feature = "window")]
    return run_window(!args.iter().any(|arg| arg == "--no-effects"), compute_hz);
    #[cfg(not(feature = "window"))]
    Err("window feature disabled; use --smoke-test or rebuild with default features".into())
}

#[cfg(feature = "window")]
fn run_window(effects: bool, compute_hz: u32) -> Result<(), Box<dyn std::error::Error>> {
    use game::Input;
    use minifb::{Key, KeyRepeat, Window, WindowOptions};
    use std::time::Instant;
    use view::{Canvas, HEIGHT, WIDTH};

    let mut app = threaded::ThreadedGame::start(effects, compute_hz)?;
    let mut window = Window::new(
        "Charon M1 / Gem Run",
        WIDTH,
        HEIGHT,
        WindowOptions {
            resize: true,
            ..WindowOptions::default()
        },
    )?;
    window.set_target_fps(60);
    let mut previous = Instant::now();
    while window.is_open() && !window.is_key_down(Key::Escape) {
        let now = Instant::now();
        let elapsed = now.duration_since(previous);
        previous = now;
        let active = window.is_active();
        let input = Input {
            dx: i32::from(active && (window.is_key_down(Key::D) || window.is_key_down(Key::Right)))
                - i32::from(
                    active && (window.is_key_down(Key::A) || window.is_key_down(Key::Left)),
                ),
            dy: i32::from(active && (window.is_key_down(Key::S) || window.is_key_down(Key::Down)))
                - i32::from(active && (window.is_key_down(Key::W) || window.is_key_down(Key::Up))),
            reset: active && window.is_key_pressed(Key::R, KeyRepeat::No),
            pause: active && window.is_key_pressed(Key::P, KeyRepeat::No),
        };
        app.set_input(input)?;
        app.frame(elapsed)?;
        let canvas = app.render.world().get::<Canvas>().ok_or("canvas missing")?;
        window.update_with_buffer(&canvas.pixels, WIDTH, HEIGHT)?;
    }
    app.shutdown()?;
    Ok(())
}

// Software-frame export also works with --no-default-features.
fn snapshot(path: &str, effects: bool) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let mut app = game::make_app(effects)?;
    app.frame(std::time::Duration::ZERO)?;
    let canvas = game::required::<view::Canvas>(app.world())?;
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(file, "P6\n{} {}\n255\n", view::WIDTH, view::HEIGHT)?;
    for &pixel in &canvas.pixels {
        file.write_all(&[(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8])?;
    }
    file.flush()?;
    app.shutdown()?;
    println!("Saved {path}");
    Ok(())
}
