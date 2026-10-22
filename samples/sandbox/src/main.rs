mod gpu;

use charon_runtime::Runtime;
use charon_sand::{Command, SandWorld, AIR, SAND, WALL};
use gpu::{Gpu, Result};
use std::{
    fs::File,
    io::Write,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

struct Options {
    workers: usize,
    width: usize,
    height: usize,
    smoke: bool,
    software: bool,
    ticks: u64,
    output: PathBuf,
    record: Option<PathBuf>,
    seconds: Option<u64>,
}
impl Options {
    fn parse() -> Result<Self> {
        let mut options = Self {
            workers: std::thread::available_parallelism()
                .map_or(2, usize::from)
                .min(4),
            width: 480,
            height: 270,
            smoke: false,
            software: false,
            ticks: 40,
            output: "artifacts/local/smoke.png".into(),
            record: None,
            seconds: None,
        };
        let mut args = std::env::args().skip(1);
        while let Some(argument) = args.next() {
            let mut value = || args.next().ok_or("missing option value");
            match argument.as_str() {
                "--workers" => options.workers = value()?.parse()?,
                "--grid" => {
                    let grid = value()?;
                    let (width, height) =
                        grid.split_once('x').ok_or("grid must be WIDTHxHEIGHT")?;
                    options.width = width.parse()?;
                    options.height = height.parse()?;
                }
                "--smoke" => options.smoke = true,
                "--software" => options.software = true,
                "--ticks" => options.ticks = value()?.parse()?,
                "--output" => options.output = value()?.into(),
                "--record" => options.record = Some(value()?.into()),
                "--seconds" => options.seconds = Some(value()?.parse()?),
                "--help" => {
                    println!("Charon sandbox\n--workers 1..32 --grid WIDTHxHEIGHT --record FILE --seconds N\n--smoke --software --ticks N --output PNG\nMouse: left sand, right erase, middle wall. R clears; Esc exits.");
                    std::process::exit(0);
                }
                _ => return Err(format!("unknown option: {argument}").into()),
            }
        }
        if options.ticks == 0 || options.ticks > 3600 {
            return Err("ticks must be 1..3600".into());
        }
        Ok(options)
    }
}
struct View {
    window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
}
impl View {
    fn new(event_loop: &ActiveEventLoop, options: &Options) -> Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Charon | LMB sand · RMB erase · MMB wall · R clear")
                    .with_inner_size(PhysicalSize::new(1920, 1080)),
            )?,
        );
        let instance = gpu::window_instance(event_loop.owned_display_handle());
        let surface = instance.create_surface(window.clone())?;
        let adapter =
            pollster::block_on(gpu::adapter(&instance, Some(&surface), options.software))?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface has no supported format")?;
        config.format = surface
            .get_capabilities(&adapter)
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(config.format);
        config.present_mode = wgpu::PresentMode::AutoVsync;
        let gpu = pollster::block_on(Gpu::new(
            &adapter,
            config.format,
            options.width as u32,
            options.height as u32,
        ))?;
        surface.configure(&gpu.device, &config);
        println!(
            "surface={}x{} present={:?} workers={} grid={}x{}",
            config.width,
            config.height,
            config.present_mode,
            options.workers,
            options.width,
            options.height
        );
        println!("Draw FPS measures submitted frames; GPU time and physical presentation are not measured.");
        Ok(Self {
            window,
            instance,
            surface,
            config,
            gpu,
        })
    }
    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width > 0 && size.height > 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.gpu.device, &self.config);
        }
    }
}
struct App {
    options: Options,
    runtime: Runtime<SandWorld>,
    view: Option<View>,
    cursor: Option<(f64, f64)>,
    buttons: [bool; 3],
    uploaded: u64,
    next_draw: Instant,
    report: Instant,
    started: Instant,
    last_draw: Option<Instant>,
    intervals: Vec<f64>,
    draws: u64,
    last_tick: u64,
    record: Option<File>,
    error: Option<String>,
}
impl App {
    fn new(options: Options, runtime: Runtime<SandWorld>) -> Result<Self> {
        let mut record = if let Some(path) = &options.record {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            Some(File::create(path)?)
        } else {
            None
        };
        if let Some(file) = &mut record {
            writeln!(file, "elapsed_s,draw_fps,sim_hz,frame_p95_ms,frame_p99_ms,tick_work_ms,tick_late_ms,sand,active_tiles,visited_cells,workers_configured,workers_used")?;
        }
        let now = Instant::now();
        Ok(Self {
            options,
            runtime,
            view: None,
            cursor: None,
            buttons: [false; 3],
            uploaded: 0,
            next_draw: now,
            report: now,
            started: now,
            last_draw: None,
            intervals: vec![],
            draws: 0,
            last_tick: 0,
            record,
            error: None,
        })
    }
    fn submit(&self, command: Command) {
        if let Err(error) = self.runtime.submit(command) {
            eprintln!("Input not applied: {error}");
        }
    }
    fn draw(&mut self) -> Result<()> {
        if self.runtime.is_finished() {
            return Err("simulation thread stopped unexpectedly".into());
        }
        let now = Instant::now();
        self.next_draw = now + Duration::from_secs_f64(1.0 / 144.0);
        let Some(view) = &mut self.view else {
            return Ok(());
        };
        let size = view.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        if let Some((x, y)) = self.cursor {
            let x = (x * self.options.width as f64 / size.width as f64) as usize;
            let y = (y * self.options.height as f64 / size.height as f64) as usize;
            for (pressed, material) in self.buttons.into_iter().zip([SAND, AIR, WALL]) {
                if pressed {
                    self.runtime
                        .submit(Command::Paint { x, y, material })
                        .map_err(|e| format!("input queue: {e}"))?;
                }
            }
        }
        let Some(snapshot) = self.runtime.latest() else {
            return Ok(());
        };
        if snapshot.tick != self.uploaded {
            view.gpu.upload(&snapshot.frame);
            self.uploaded = snapshot.tick;
        }
        let (texture, reconfigure) = match view.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
            wgpu::CurrentSurfaceTexture::Outdated => {
                view.surface.configure(&view.gpu.device, &view.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                view.surface = view.instance.create_surface(view.window.clone())?;
                view.surface.configure(&view.gpu.device, &view.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => {
                return Ok(())
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("surface validation failed".into())
            }
        };
        let mut encoder = view.gpu.device.create_command_encoder(&Default::default());
        view.gpu.draw(
            &mut encoder,
            &texture.texture.create_view(&Default::default()),
        );
        view.gpu.queue.submit([encoder.finish()]);
        view.window.pre_present_notify();
        view.gpu.queue.present(texture);
        if reconfigure {
            view.surface.configure(&view.gpu.device, &view.config);
        }
        if let Some(last) = self.last_draw.replace(now) {
            self.intervals.push((now - last).as_secs_f64() * 1000.0);
        }
        self.draws += 1;
        let elapsed = self.report.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            self.intervals.sort_by(f64::total_cmp);
            let percentile = |p: f64| {
                self.intervals
                    .get(((self.intervals.len().saturating_sub(1)) as f64 * p).ceil() as usize)
                    .copied()
                    .unwrap_or(0.0)
            };
            let (p95, p99) = (percentile(0.95), percentile(0.99));
            let draw_fps = self.draws as f64 / elapsed;
            let sim_hz = (snapshot.tick - self.last_tick) as f64 / elapsed;
            let frame = &snapshot.frame;
            view.window.set_title(&format!("Charon | draw {draw_fps:.0} FPS · sim {sim_hz:.1} Hz · tick {:.2} ms · workers {}/{} · sand {} · p99 {p99:.1} ms",
                snapshot.work_ms, frame.workers_used, frame.workers_configured, frame.sand));
            let row = format!(
                "{:.3},{draw_fps:.2},{sim_hz:.2},{p95:.3},{p99:.3},{:.3},{:.3},{},{},{},{},{}",
                self.started.elapsed().as_secs_f64(),
                snapshot.work_ms,
                snapshot.late_ms,
                frame.sand,
                frame.active_tiles,
                frame.visited_cells,
                frame.workers_configured,
                frame.workers_used
            );
            println!("{row}");
            if let Some(file) = &mut self.record {
                writeln!(file, "{row}")?;
                file.flush()?;
            }
            self.last_tick = snapshot.tick;
            self.draws = 0;
            self.intervals.clear();
            self.report = Instant::now();
        }
        Ok(())
    }
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.view.is_none() {
            match View::new(event_loop, &self.options) {
                Ok(view) => self.view = Some(view),
                Err(error) => {
                    self.error = Some(error.to_string());
                    event_loop.exit();
                }
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(view) = &mut self.view {
                    view.resize(size);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y))
            }
            WindowEvent::CursorLeft { .. } => self.cursor = None,
            WindowEvent::Focused(false) => self.buttons.fill(false),
            WindowEvent::MouseInput { state, button, .. } => {
                let index = match button {
                    MouseButton::Left => Some(0),
                    MouseButton::Right => Some(1),
                    MouseButton::Middle => Some(2),
                    _ => None,
                };
                if let Some(index) = index {
                    self.buttons[index] = state == ElementState::Pressed;
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Escape) => event_loop.exit(),
                    PhysicalKey::Code(KeyCode::KeyR) => self.submit(Command::Clear),
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
            .options
            .seconds
            .is_some_and(|seconds| self.started.elapsed() >= Duration::from_secs(seconds))
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
    let options = Options::parse()?;
    let runtime = Runtime::start(SandWorld::new(
        options.width,
        options.height,
        options.workers,
    )?)?;
    for x in [options.width / 4, options.width / 2, options.width * 3 / 4] {
        runtime.submit(Command::Paint {
            x,
            y: 8,
            material: SAND,
        })?;
    }
    if options.smoke {
        let timeout = Instant::now() + Duration::from_secs(70);
        let snapshot = loop {
            if runtime.is_finished() {
                return Err("simulation thread stopped unexpectedly".into());
            }
            if let Some(snapshot) = runtime.latest() {
                if snapshot.tick >= options.ticks {
                    break snapshot;
                }
            }
            if Instant::now() > timeout {
                return Err("simulation smoke timed out".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        if snapshot.frame.sand == 0 {
            return Err("smoke scene contains no sand".into());
        }
        let instance = gpu::instance();
        let adapter = pollster::block_on(gpu::adapter(&instance, None, options.software))?;
        let gpu = pollster::block_on(Gpu::new(
            &adapter,
            wgpu::TextureFormat::Rgba8Unorm,
            options.width as u32,
            options.height as u32,
        ))?;
        gpu.smoke(&snapshot.frame, &options.output)?;
        println!("simulation_tick={} sand={} workers_used={}/{} tick_work_ms={:.3} (smoke, not FPS verification)",
            snapshot.tick, snapshot.frame.sand, snapshot.frame.workers_used, snapshot.frame.workers_configured, snapshot.work_ms);
        return Ok(());
    }
    let event_loop = EventLoop::new()?;
    let mut app = App::new(options, runtime)?;
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(error.into());
    }
    Ok(())
}
