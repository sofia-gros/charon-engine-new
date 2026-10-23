mod picture;
use charon_dialogue::{Command, Dialogue, Page};
use charon_render::{
    append_performance_overlay, DebugPerformance, ImageView, Result, WindowRenderer,
};
use charon_runtime::Runtime;
use std::{
    fs::File,
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::WindowId,
};

struct App {
    runtime: Runtime<Dialogue>,
    view: Option<WindowRenderer>,
    page: Option<Page>,
    next_draw: Instant,
    started: Instant,
    seconds: Option<u64>,
    record: Option<File>,
    error: Option<String>,
    report: Instant,
    draws: u64,
    last_tick: u64,
    last_sim_sample: Instant,
    performance: Option<DebugPerformance>,
}
impl App {
    fn command(&self, command: Command) {
        if let Err(error) = self.runtime.submit(command) {
            eprintln!("Input not applied: {error}");
        }
    }
    fn draw(&mut self) -> Result<()> {
        self.next_draw = Instant::now() + Duration::from_secs_f64(1.0 / 144.0);
        if self.runtime.is_finished() {
            return Err("dialogue thread stopped unexpectedly".into());
        }
        let Some(snapshot) = self.runtime.latest() else {
            return Ok(());
        };
        let sampled_at = Instant::now();
        let Some(view) = self.view.as_mut() else {
            return Ok(());
        };
        let changed = self.page != Some(snapshot.frame.page);
        let pixels = changed.then(|| picture::draw(&snapshot.frame));
        let image = pixels.as_ref().map(|rgba| ImageView {
            width: 640,
            height: 360,
            rgba,
        });
        let mut overlays = Vec::new();
        append_performance_overlay(&mut overlays, self.performance, view.software_adapter);
        if view.present_with_overlays(image, &overlays)?.is_none() {
            return Ok(());
        }
        self.draws += 1;
        let elapsed = self.report.elapsed().as_secs_f64();
        let report = elapsed >= 1.0;
        if report {
            self.performance = Some(DebugPerformance {
                draw_fps: self.draws as f64 / elapsed,
                simulation_hz: snapshot.tick.saturating_sub(self.last_tick) as f64
                    / sampled_at
                        .duration_since(self.last_sim_sample)
                        .as_secs_f64(),
            });
            self.last_tick = snapshot.tick;
            self.last_sim_sample = sampled_at;
            self.draws = 0;
            self.report = Instant::now();
        }
        if changed || report {
            self.page = Some(snapshot.frame.page);
            let row = format!(
                "{:.3},{},{:?},{:.2},{:.2}",
                self.started.elapsed().as_secs_f64(),
                snapshot.tick,
                snapshot.frame.page,
                self.performance.map_or(0.0, |m| m.draw_fps),
                self.performance.map_or(0.0, |m| m.simulation_hz)
            );
            println!("{row}");
            if let Some(file) = &mut self.record {
                writeln!(file, "{row}")?;
                file.flush()?;
            }
        }
        Ok(())
    }
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.view.is_none() {
            match WindowRenderer::new(
                event_loop,
                "Charon | Dialogue · Space continue · 1/2 choose · R restart",
                640,
                360,
                false,
            ) {
                Ok(view) => self.view = Some(view),
                Err(error) => {
                    self.error = Some(error.to_string());
                    event_loop.exit();
                }
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(view) = &mut self.view {
                    view.resize(size);
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.command(Command::Advance),
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Space) => self.command(Command::Advance),
                    PhysicalKey::Code(KeyCode::Digit1) => self.command(Command::Choose(0)),
                    PhysicalKey::Code(KeyCode::Digit2) => self.command(Command::Choose(1)),
                    PhysicalKey::Code(KeyCode::KeyR) => self.command(Command::Reset),
                    PhysicalKey::Code(KeyCode::Escape) => event_loop.exit(),
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.draw() {
                    self.error = Some(error.to_string());
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self
            .seconds
            .is_some_and(|s| self.started.elapsed() >= Duration::from_secs(s))
        {
            event_loop.exit();
            return;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_draw));
        if Instant::now() >= self.next_draw {
            if let Some(view) = &self.view {
                view.window.request_redraw();
            }
        }
    }
}
fn main() -> Result<()> {
    let mut seconds = None;
    let mut path: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seconds" => seconds = Some(args.next().ok_or("missing seconds")?.parse()?),
            "--record" => path = Some(args.next().ok_or("missing record path")?.into()),
            _ => return Err(format!("unknown option: {arg}").into()),
        }
    }
    let mut record = path
        .map(|p| {
            if let Some(parent) = p.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            File::create(p)
        })
        .transpose()?;
    if let Some(file) = &mut record {
        writeln!(file, "elapsed_s,tick,page,draw_fps,sim_hz")?;
    }
    let now = Instant::now();
    let mut app = App {
        runtime: Runtime::start(Dialogue::default())?,
        view: None,
        page: None,
        next_draw: now,
        started: now,
        seconds,
        record,
        error: None,
        report: now,
        draws: 0,
        last_tick: 0,
        last_sim_sample: now,
        performance: None,
    };
    EventLoop::new()?.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(error.into());
    }
    Ok(())
}
