mod game;
mod picture;
mod story;
use charon_dialogue::Command;
use charon_render::{
    append_performance_overlay, DebugPerformance, ImageView, Result, Sprite, WindowRenderer,
};
use charon_runtime::Runtime;
use game::Command as GameCommand;
use picture::Action;
use std::{
    fs::File,
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};
use story::Page;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::WindowId,
};

struct App {
    runtime: Runtime<game::Game>,
    ui: picture::Ui,
    cursor: Option<[f32; 2]>,
    shift: bool,
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
        if let Err(error) = self.runtime.submit(GameCommand::Dialogue(command)) {
            eprintln!("Input not applied: {error}");
        }
    }
    fn game_command(&self, command: GameCommand) {
        if let Err(error) = self.runtime.submit(command) {
            eprintln!("Input not applied: {error}");
        }
    }
    fn action(&self, action: Action) {
        self.command(match action {
            Action::Continue => Command::Advance,
            Action::Choose(index) => Command::Choose(index),
            Action::Reset => Command::Reset,
        });
    }
    fn pointer(&mut self) {
        let point = self.view.as_ref().and_then(|view| {
            let size = view.window.inner_size();
            self.cursor.and_then(|p| {
                charon_ui::logical_pointer(p, [size.width as f32, size.height as f32], [640., 360.])
            })
        });
        self.ui.pointer_move(point);
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
        let size = view.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let changed = self.page != Some(snapshot.frame.dialogue.page);
        self.ui.set_page(&snapshot.frame.dialogue)?;
        self.ui.set_status(&snapshot.frame);
        self.ui.pointer_move(self.cursor.and_then(|p| {
            charon_ui::logical_pointer(p, [size.width as f32, size.height as f32], [640., 360.])
        }));
        let ui_changed = self.ui.render(view, &snapshot.frame.dialogue)?;
        let pixels = changed.then(|| picture::background(snapshot.frame.dialogue.page));
        let image = pixels.as_ref().map(|rgba| ImageView {
            width: 640,
            height: 360,
            rgba,
        });
        let mut overlays = Vec::new();
        append_performance_overlay(&mut overlays, self.performance, view.software_adapter);
        let ui = [Sprite {
            texture: self.ui.texture(),
            source: [0, 0, 640, 360],
            destination: [0., 0., 640., 360.],
            flip_x: false,
        }];
        let Some(render) = view.present_with_ui(image, &ui, &overlays)? else {
            return Ok(());
        };
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
            self.page = Some(snapshot.frame.dialogue.page);
            let row = format!(
                "{:.3},{},{:?},{:.2},{:.2},{},{},{},{},{},{},{:?},{},{},{},{},{}",
                self.started.elapsed().as_secs_f64(),
                snapshot.tick,
                snapshot.frame.dialogue.page,
                self.performance.map_or(0.0, |m| m.draw_fps),
                self.performance.map_or(0.0, |m| m.simulation_hz),
                self.ui.redraws,
                self.ui.cached_glyphs(),
                u8::from(ui_changed),
                render.ui_upload_bytes,
                size.width,
                size.height,
                self.ui.buttons.focused(),
                snapshot.frame.saves,
                snapshot.frame.loads,
                snapshot.frame.volume,
                snapshot.frame.muted,
                match snapshot.frame.audio {
                    charon_audio::DeviceState::Ready => "Ready",
                    charon_audio::DeviceState::Starting => "Starting",
                    _ => "Unavailable",
                }
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
                self.pointer();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some([position.x as f32, position.y as f32]);
                self.pointer();
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                self.pointer();
            }
            WindowEvent::Focused(false) => {
                self.ui.cancel();
                self.shift = false;
            }
            WindowEvent::ModifiersChanged(modifiers) => self.shift = modifiers.state().shift_key(),
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                self.pointer();
                if state == ElementState::Pressed {
                    self.ui.press();
                } else if let Some(action) = self.ui.release() {
                    self.action(action);
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Space | KeyCode::Enter) => {
                        if let Some(action) = self.ui.buttons.focused() {
                            self.action(action);
                        }
                    }
                    PhysicalKey::Code(KeyCode::Tab) => self.ui.navigate(self.shift),
                    PhysicalKey::Code(KeyCode::ArrowLeft | KeyCode::ArrowUp) => {
                        self.ui.navigate(true)
                    }
                    PhysicalKey::Code(KeyCode::ArrowRight | KeyCode::ArrowDown) => {
                        self.ui.navigate(false)
                    }
                    PhysicalKey::Code(KeyCode::Digit1) => self.command(Command::Choose(0)),
                    PhysicalKey::Code(KeyCode::Digit2) => self.command(Command::Choose(1)),
                    PhysicalKey::Code(KeyCode::F5) => self.game_command(GameCommand::Save),
                    PhysicalKey::Code(KeyCode::F9) => self.game_command(GameCommand::Load),
                    PhysicalKey::Code(KeyCode::KeyM) => self.game_command(GameCommand::Mute),
                    PhysicalKey::Code(KeyCode::KeyP) => self.game_command(GameCommand::PauseMusic),
                    PhysicalKey::Code(KeyCode::Equal | KeyCode::NumpadAdd) => {
                        self.game_command(GameCommand::Volume(10))
                    }
                    PhysicalKey::Code(KeyCode::Minus | KeyCode::NumpadSubtract) => {
                        self.game_command(GameCommand::Volume(-10))
                    }
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
    let mut save_file = None;
    let mut audio = true;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seconds" => seconds = Some(args.next().ok_or("missing seconds")?.parse()?),
            "--save-file" => {
                save_file = Some(PathBuf::from(args.next().ok_or("missing save path")?))
            }
            "--no-audio" => audio = false,
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
        writeln!(file, "elapsed_s,tick,page,draw_fps,sim_hz,ui_redraws,cached_glyphs,ui_texture_changed,ui_vertex_bytes,window_width,window_height,focused,saves,loads,volume,muted,audio")?;
    }
    let now = Instant::now();
    let mut app = App {
        runtime: Runtime::start(game::Game::new(
            save_file.map_or_else(default_save_file, Ok)?,
            audio,
        )?)?,
        ui: picture::Ui::new()?,
        cursor: None,
        shift: false,
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

fn default_save_file() -> Result<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
    };
    Ok(base
        .ok_or("no user data directory; specify --save-file")?
        .join("charon/dialogue/slot.json"))
}
