mod actor_view;
mod cup;
mod debug_view;
mod game;
mod material_view;
mod materials;
mod scenes;
mod smoke;
mod trace;
use charon_render::{
    self as gpu, Camera2D, DebugPerformance, Gpu, Placement, Result, WindowRenderer,
};
use charon_runtime::Runtime;
use charon_sand::Command;
use game::{Command as GameCommand, Game};
use materials::{AIR, ASH, OIL, SAND, WALL, WATER};
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

#[derive(Clone, Copy)]
enum Tool {
    Brush,
    Blocks,
    Images,
}
impl Tool {
    fn label(self) -> &'static str {
        match self {
            Self::Brush => "1 SAND 2 ASH 3 BLOCK 4 IMAGE 5 WATER 6 OIL L SEA",
            Self::Blocks => "3 BLOCK GRID 8",
            Self::Images => "4 IMAGE FREE",
        }
    }
}
struct Options {
    fluid_gpu: bool,
    fluid_timing: bool,
    input_log: Option<PathBuf>,
    replay: Option<PathBuf>,
    play: bool,
    workers: usize,
    width: usize,
    height: usize,
    smoke: bool,
    software: bool,
    ticks: u64,
    record: Option<PathBuf>,
    seconds: Option<u64>,
}
impl Options {
    fn parse() -> Result<Self> {
        let mut options = Self {
            fluid_gpu: true,
            fluid_timing: true,
            input_log: None,
            replay: None,
            play: false,
            workers: std::thread::available_parallelism()
                .map_or(2, usize::from)
                .min(4),
            width: 480,
            height: 270,
            smoke: false,
            software: false,
            ticks: 40,
            record: None,
            seconds: None,
        };
        let mut args = std::env::args().skip(1);
        while let Some(argument) = args.next() {
            let mut value = || args.next().ok_or("missing option value");
            match argument.as_str() {
                "--fluid-backend" => {
                    options.fluid_gpu = match value()?.as_str() {
                        "gpu" => true,
                        "cpu" => false,
                        _ => return Err("fluid backend must be gpu or cpu".into()),
                    }
                }
                "--no-fluid-timing" => options.fluid_timing = false,
                "--input-log" => options.input_log = Some(value()?.into()),
                "--replay" => options.replay = Some(value()?.into()),
                "--play" => options.play = true,
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
                "--record" => options.record = Some(value()?.into()),
                "--seconds" => options.seconds = Some(value()?.parse()?),
                "--help" => {
                    println!("Charon sandbox\n--play: A/D move, Space jump, hold F dig, B toggle sand collision, J fires right\n--input-log FILE records authoritative inputs; --replay FILE validates without a window\n--fluid-backend gpu|cpu (default gpu; CPU is reference/fallback)\n--no-fluid-timing disables optional GPU timestamp queries\n--workers 1..32 --grid WIDTHxHEIGHT --record FILE --seconds N\n--smoke --software --ticks N\nC toggles follow camera; =/- zoom. P pauses/resumes; N pauses and advances one tick; I toggles cell inspection.\n1 sand / 2 ash brush; 3 grid blocks; 4 freely positioned images; 5 water / 6 oil brush; L sea test; K cup test, A/D move, O add oil. Mouse: left paint, right erase, middle grid block. R clears; Esc exits.");
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
struct App {
    options: Options,
    runtime: Runtime<Game>,
    actor: Option<actor_view::ActorVisual>,
    material_visual: material_view::MaterialVisual,
    view: Option<WindowRenderer>,
    cursor: Option<(f64, f64)>,
    buttons: [bool; 3],
    brush: u8,
    tool: Tool,
    clicks: [bool; 3],
    follow_camera: bool,
    zoom: f32,
    inspect_enabled: bool,
    inspected: Option<(i32, i32)>,
    next_draw: Instant,
    report: Instant,
    started: Instant,
    last_draw: Option<Instant>,
    intervals: Vec<f64>,
    draws: u64,
    last_tick: u64,
    last_sim_sample: Instant,
    performance: Option<DebugPerformance>,
    record: Option<File>,
    error: Option<String>,
}
impl App {
    fn new(options: Options, runtime: Runtime<Game>) -> Result<Self> {
        let mut record = if let Some(path) = &options.record {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            Some(File::create(path)?)
        } else {
            None
        };
        if let Some(file) = &mut record {
            writeln!(file, "elapsed_s,draw_fps,sim_hz,frame_p95_ms,frame_p99_ms,tick_work_ms,tick_late_ms,sand,active_tiles,visited_cells,workers_configured,workers_used,update_ms,snapshot_ms,upload_enqueue_ms,upload_bytes,acquire_ms,encode_present_ms,converted_cells,player_x,player_y,grounded,embedded,sand_solid,overlay_upload_bytes,runtime_tick,simulation_tick,paused,inspect_x,inspect_y,material,next_tile_awake,shots,sparks,impacts,sprite_upload_bytes,appearance_upload_bytes,atlas_upload_bytes,camera_x,camera_y,camera_width,camera_height,display_player_x,display_player_y,window_width,window_height,facing,placed_images,last_image_x,last_image_y,liquid,cup_x,cup_y,cup_width,cup_height,cup_moves,cup_blocked,water,oil,fluid_water_volume,fluid_oil_volume,fluid_divergence,fluid_substeps,fluid_pressure_ms,fluid_transport_ms,fluid_velocity_ms,fluid_flux_retries,fluid_max_speed,fluid_sleeping,fluid_surface_delta,fluid_rms_speed,fluid_mass_speed,fluid_fraction_change,fluid_compute_backend,fluid_gpu_velocity_ms,fluid_gpu_pressure_ms,fluid_gpu_upload_bytes,fluid_gpu_readback_bytes,fluid_gpu_dispatches,fluid_gpu_velocity_workgroups,fluid_gpu_pressure_workgroups,fluid_pressure_iterations")?;
        }
        let now = Instant::now();
        let actor = Some(actor_view::ActorVisual::new()?);
        Ok(Self {
            material_visual: material_view::MaterialVisual::new()?,
            actor,
            options,
            runtime,
            view: None,
            cursor: None,
            buttons: [false; 3],
            brush: SAND,
            tool: Tool::Brush,
            clicks: [false; 3],
            follow_camera: false,
            zoom: 1.,
            inspect_enabled: false,
            inspected: None,
            next_draw: now,
            report: now,
            started: now,
            last_draw: None,
            intervals: vec![],
            draws: 0,
            last_tick: 0,
            last_sim_sample: now,
            performance: None,
            record,
            error: None,
        })
    }
    fn submit(&self, command: Command) {
        if let Err(error) = self.runtime.submit(GameCommand::Terrain(command)) {
            eprintln!("Input not applied: {error}");
        }
    }
    fn game_command(&self, command: GameCommand) {
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
        let Some(snapshot) = self.runtime.latest() else {
            return Ok(());
        };
        let sampled_at = Instant::now();
        let displayed_player = snapshot
            .frame
            .player_position(sampled_at.saturating_duration_since(snapshot.published_at));
        let world_size = [self.options.width as f32, self.options.height as f32];
        let visible = world_size.map(|v| v / self.zoom);
        let focus = if self.follow_camera {
            displayed_player.map_or(world_size.map(|v| v / 2.), |p| [p[0] + 3.5, p[1] + 6.])
        } else {
            world_size.map(|v| v / 2.)
        };
        let origin = std::array::from_fn(|i| {
            (focus[i] - visible[i] / 2.).clamp(0., world_size[i] - visible[i])
        });
        let camera = Camera2D::new(origin, visible)?;
        view.set_camera(camera);
        let pointer = self.cursor.and_then(|(x, y)| {
            camera.view_to_world(
                [x as f32, y as f32],
                [size.width as f32, size.height as f32],
            )
        });
        let preview = match self.tool {
            Tool::Brush => None,
            Tool::Blocks => pointer
                .map(|point| {
                    Placement::Grid {
                        origin: [0., 0.],
                        cell: [8., 8.],
                    }
                    .position(point)
                })
                .transpose()?
                .map(|[x, y]| [x, y, 8., 8.]),
            Tool::Images => pointer
                .map(|point| Placement::Free.position(point))
                .transpose()?
                .map(|[x, y]| [x, y, 8., 14.]),
        };
        if let Some([x, y]) = pointer {
            match self.tool {
                Tool::Brush => {
                    for (pressed, material) in self.buttons.into_iter().zip([self.brush, AIR, WALL])
                    {
                        if pressed {
                            let command = if material == WALL {
                                let [bx, by] = Placement::Grid {
                                    origin: [0., 0.],
                                    cell: [8., 8.],
                                }
                                .position([x, y])?;
                                Command::FillRect {
                                    x: bx as usize,
                                    y: by as usize,
                                    width: 8,
                                    height: 8,
                                    material,
                                }
                            } else {
                                Command::Paint {
                                    x: x.floor() as usize,
                                    y: y.floor() as usize,
                                    material,
                                }
                            };
                            self.runtime
                                .submit(GameCommand::Terrain(command))
                                .map_err(|e| format!("input queue: {e}"))?;
                        }
                    }
                }
                Tool::Blocks => {
                    let [bx, by, _, _] = preview.expect("block pointer preview");
                    if self.buttons[0] || self.buttons[1] {
                        self.runtime
                            .submit(GameCommand::Terrain(Command::FillRect {
                                x: bx as usize,
                                y: by as usize,
                                width: 8,
                                height: 8,
                                material: if self.buttons[1] { AIR } else { WALL },
                            }))
                            .map_err(|e| format!("input queue: {e}"))?;
                    }
                }
                Tool::Images => {
                    if self.clicks[0] {
                        self.runtime
                            .submit(GameCommand::PlaceImage([x, y]))
                            .map_err(|e| format!("input queue: {e}"))?;
                    }
                    if self.clicks[1] {
                        self.runtime
                            .submit(GameCommand::EraseImage([x, y]))
                            .map_err(|e| format!("input queue: {e}"))?;
                    }
                }
            }
        }
        self.clicks.fill(false);
        let inspection = pointer
            .filter(|_| self.inspect_enabled)
            .map(|[x, y]| (x.floor() as i32, y.floor() as i32));
        if inspection != self.inspected {
            self.runtime
                .submit(GameCommand::Inspect(inspection))
                .map_err(|e| format!("inspection queue: {e}"))?;
            self.inspected = inspection;
        }
        let overlays = debug_view::overlays(
            &snapshot.frame,
            self.inspect_enabled,
            self.performance,
            view.software_adapter,
            camera,
            self.tool.label(),
            preview,
        );
        let sprites = self.actor.as_mut().map_or_else(Vec::new, |actor| {
            actor.sprites(&snapshot.frame, displayed_player)
        });
        self.material_visual.update(self.started.elapsed());
        let Some(render) = view.present_indexed_with_sprites(
            &snapshot.frame.terrain.indices,
            self.material_visual.looks(),
            self.material_visual.texture(),
            &sprites,
            &overlays,
        )?
        else {
            return Ok(());
        };
        if let Some(last) = self.last_draw.replace(now) {
            self.intervals.push((now - last).as_secs_f64() * 1000.0);
        }
        self.draws += 1;
        let elapsed = self.report.elapsed().as_secs_f64();
        if elapsed
            >= if self.options.play || snapshot.frame.paused || self.inspect_enabled {
                0.1
            } else {
                1.0
            }
        {
            self.intervals.sort_by(f64::total_cmp);
            let percentile = |p: f64| {
                self.intervals
                    .get(((self.intervals.len().saturating_sub(1)) as f64 * p).ceil() as usize)
                    .copied()
                    .unwrap_or(0.0)
            };
            let (p95, p99) = (percentile(0.95), percentile(0.99));
            let draw_fps = self.draws as f64 / elapsed;
            let sim_hz = snapshot
                .frame
                .simulation_tick
                .saturating_sub(self.last_tick) as f64
                / sampled_at
                    .duration_since(self.last_sim_sample)
                    .as_secs_f64();
            self.performance = Some(DebugPerformance {
                draw_fps,
                simulation_hz: sim_hz,
            });
            let frame = &snapshot.frame.terrain;
            view.window.set_title(&format!("Charon | {} tick {} | P pause N step I inspect | draw {draw_fps:.0} FPS · sim {sim_hz:.1} Hz · tick {:.2} ms · workers {}/{} · sand {} · p99 {p99:.1} ms",
                if snapshot.frame.paused {"PAUSED"} else {"RUNNING"},snapshot.frame.simulation_tick,snapshot.work_ms, frame.workers_used, frame.workers_configured, frame.sand));
            let mut row = format!(
                "{:.3},{draw_fps:.2},{sim_hz:.2},{p95:.3},{p99:.3},{:.3},{:.3},{},{},{},{},{},{:.6},{:.6},{:.6},{},{:.6},{:.6},{},{:.3},{:.3},{},{},{},{}",
                self.started.elapsed().as_secs_f64(),
                snapshot.work_ms,
                snapshot.late_ms,
                frame.sand,
                frame.active_tiles,
                frame.visited_cells,
                frame.workers_configured,
                frame.workers_used, frame.update_ms, frame.snapshot_ms, render.upload_enqueue_ms, render.upload_bytes, render.acquire_ms, render.encode_present_ms, frame.converted_cells, snapshot.frame.player.map_or(0.0, |p|p.x()), snapshot.frame.player.map_or(0.0, |p|p.y()), snapshot.frame.player.is_some_and(|p|p.grounded),snapshot.frame.player.is_some_and(|p|p.embedded),snapshot.frame.sand_solid, render.overlay_upload_bytes
            );
            let cell = snapshot.frame.inspection;
            row.push_str(&format!(
                ",{},{},{},{},{},{},{}",
                snapshot.tick,
                snapshot.frame.simulation_tick,
                snapshot.frame.paused,
                cell.map_or_else(String::new, |c| c.x.to_string()),
                cell.map_or_else(String::new, |c| c.y.to_string()),
                cell.and_then(|c| c.material)
                    .map_or_else(String::new, |m| m.to_string()),
                cell.and_then(|c| c.awake)
                    .map_or_else(String::new, |a| a.to_string())
            ));
            row.push_str(&format!(
                ",{},{},{}",
                snapshot.frame.shots.len(),
                snapshot.frame.sparks.len(),
                snapshot.frame.impacts
            ));
            row.push_str(&format!(
                ",{},{},{}",
                render.sprite_upload_bytes,
                render.appearance_upload_bytes,
                render.atlas_upload_bytes
            ));
            row.push_str(&format!(
                ",{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}",
                origin[0],
                origin[1],
                visible[0],
                visible[1],
                displayed_player.map_or(0., |p| p[0]),
                displayed_player.map_or(0., |p| p[1])
            ));
            row.push_str(&format!(
                ",{},{},{}",
                size.width, size.height, snapshot.frame.facing
            ));
            let last_image = snapshot.frame.images.last().copied().unwrap_or([0.; 2]);
            row.push_str(&format!(
                ",{},{:.4},{:.4}",
                snapshot.frame.images.len(),
                last_image[0],
                last_image[1]
            ));
            row.push_str(&format!(",{}", frame.liquid));
            if let Some(cup) = snapshot.frame.cup {
                row.push_str(&format!(
                    ",{},{},{},{},{},{},{},{}",
                    cup.x, cup.y, cup.width, cup.height, cup.moves, cup.blocked, cup.water, cup.oil
                ));
                let f = cup.fluid;
                row.push_str(&format!(
                    ",{:.9},{:.9},{:.9},{},{:.6},{:.6},{:.6},{},{:.6},{},{:.6}",
                    f.volume[0],
                    f.volume[1],
                    f.max_divergence,
                    f.substeps,
                    f.pressure_ms,
                    f.transport_ms,
                    f.velocity_ms,
                    f.flux_retries,
                    f.max_speed,
                    f.sleeping,
                    cup.surface_delta
                ));
                row.push_str(&format!(
                    ",{:.9},{:.9},{:.9}",
                    f.rms_speed, f.max_mass_speed, f.max_fraction_change
                ));
            } else {
                row.push_str(",,,,,,,,,,,,,,,,,,,,,,");
            }
            row.push(',');
            if let Some(cup) = snapshot.frame.cup {
                row.push_str(cup.compute_backend);
            }
            if let Some(gpu) = snapshot.frame.cup.and_then(|c| c.fluid.gpu) {
                row.push_str(&format!(
                    ",{},{},{},{},{},{},{}",
                    gpu.velocity_ms
                        .map_or_else(String::new, |v| format!("{v:.6}")),
                    gpu.pressure_ms
                        .map_or_else(String::new, |v| format!("{v:.6}")),
                    gpu.upload_bytes,
                    gpu.readback_bytes,
                    gpu.dispatches,
                    gpu.velocity_workgroups,
                    gpu.pressure_workgroups
                ));
            } else {
                row.push_str(",,,,,,,");
            }
            row.push(',');
            if let Some(cup) = snapshot.frame.cup {
                row.push_str(&cup.fluid.pressure_iterations.to_string());
            }
            println!("{row}");
            if let Some(file) = &mut self.record {
                writeln!(file, "{row}")?;
                file.flush()?;
            }
            self.last_tick = snapshot.frame.simulation_tick;
            self.last_sim_sample = sampled_at;
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
            match WindowRenderer::new(
                event_loop,
                "Charon | LMB sand · RMB erase · MMB wall · R clear",
                self.options.width as u32,
                self.options.height as u32,
                self.options.software,
            ) {
                Ok(view) => {
                    if let Some(actor) = &mut self.actor {
                        actor.upload(&view);
                    }
                    self.view = Some(view);
                }
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
            WindowEvent::Focused(false) => {
                self.buttons.fill(false);
                self.clicks.fill(false);
                self.game_command(GameCommand::Left(false));
                self.game_command(GameCommand::Right(false));
                self.game_command(GameCommand::Dig(false));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let index = match button {
                    MouseButton::Left => Some(0),
                    MouseButton::Right => Some(1),
                    MouseButton::Middle => Some(2),
                    _ => None,
                };
                if let Some(index) = index {
                    self.buttons[index] = state == ElementState::Pressed;
                    if state == ElementState::Pressed {
                        self.clicks[index] = true;
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } if !event.repeat => {
                let down = event.state == ElementState::Pressed;
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::KeyA | KeyCode::ArrowLeft) => {
                        self.game_command(GameCommand::Left(down))
                    }
                    PhysicalKey::Code(KeyCode::KeyD | KeyCode::ArrowRight) => {
                        self.game_command(GameCommand::Right(down))
                    }
                    PhysicalKey::Code(KeyCode::KeyF) => self.game_command(GameCommand::Dig(down)),
                    PhysicalKey::Code(KeyCode::Digit1) if down => {
                        self.brush = SAND;
                        self.tool = Tool::Brush;
                    }
                    PhysicalKey::Code(KeyCode::Digit2) if down => {
                        self.brush = ASH;
                        self.tool = Tool::Brush;
                    }
                    PhysicalKey::Code(KeyCode::Digit5) if down => {
                        self.brush = WATER;
                        self.tool = Tool::Brush;
                    }
                    PhysicalKey::Code(KeyCode::Digit6) if down => {
                        self.brush = OIL;
                        self.tool = Tool::Brush;
                    }
                    PhysicalKey::Code(KeyCode::KeyH) if down => {
                        self.game_command(GameCommand::DamScene)
                    }
                    PhysicalKey::Code(KeyCode::KeyK) if down => {
                        self.game_command(GameCommand::CupScene)
                    }
                    PhysicalKey::Code(KeyCode::KeyO) if down => {
                        self.game_command(GameCommand::AddOil)
                    }
                    PhysicalKey::Code(KeyCode::KeyL) if down => {
                        for command in scenes::sea(self.options.width, self.options.height) {
                            self.submit(command);
                        }
                    }
                    PhysicalKey::Code(KeyCode::Digit3) if down => self.tool = Tool::Blocks,
                    PhysicalKey::Code(KeyCode::Digit4) if down => self.tool = Tool::Images,
                    PhysicalKey::Code(KeyCode::KeyP) if down => {
                        self.game_command(GameCommand::Pause)
                    }
                    PhysicalKey::Code(KeyCode::KeyN) if down => {
                        self.game_command(GameCommand::Step)
                    }
                    PhysicalKey::Code(KeyCode::KeyI) if down => {
                        self.inspect_enabled = !self.inspect_enabled
                    }
                    PhysicalKey::Code(KeyCode::Space) if down => {
                        self.game_command(GameCommand::Jump)
                    }
                    PhysicalKey::Code(KeyCode::KeyC) if down => {
                        self.follow_camera = !self.follow_camera;
                        self.zoom = if self.follow_camera { 2. } else { 1. };
                    }
                    PhysicalKey::Code(KeyCode::Equal) if down => {
                        self.zoom = (self.zoom * 1.25).min(4.)
                    }
                    PhysicalKey::Code(KeyCode::Minus) if down => {
                        self.zoom = (self.zoom / 1.25).max(1.)
                    }
                    PhysicalKey::Code(KeyCode::KeyJ) if down => {
                        self.game_command(GameCommand::Fire)
                    }
                    PhysicalKey::Code(KeyCode::KeyB) if down => {
                        self.game_command(GameCommand::ToggleSand)
                    }
                    PhysicalKey::Code(KeyCode::Escape) if down => event_loop.exit(),
                    PhysicalKey::Code(KeyCode::KeyR) if down => self.submit(Command::Clear),
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
    if let Some(path) = &options.replay {
        if options.input_log.is_some() {
            return Err("replay cannot record to an input log".into());
        }
        let ticks = trace::replay(path, options.workers)?;
        println!(
            "replay_ticks={ticks} checks=terrain,player,input_state workers={}",
            options.workers
        );
        return Ok(());
    }
    let mut game = Game::new(options.width, options.height, options.workers, options.play)?;
    if options.fluid_gpu {
        let gpu = std::sync::Arc::new(
            charon_fluid::gpu::GpuContext::with_timing(options.fluid_timing).map_err(|e| {
                format!(
                    "fluid compute unavailable: {e}; use --fluid-backend cpu for the CPU fallback"
                )
            })?,
        );
        println!(
            "fluid_compute={:?} identity={} timestamps={}",
            gpu.adapter_info(),
            gpu.identity(),
            gpu.timing_available()
        );
        game.use_fluid_gpu(gpu);
    }
    if let Some(path) = &options.input_log {
        game.record_to(path)?;
    }
    let runtime = Runtime::start(game)?;
    for x in if options.play {
        vec![]
    } else {
        vec![options.width / 4, options.width / 2, options.width * 3 / 4]
    } {
        runtime.submit(GameCommand::Terrain(Command::Paint {
            x,
            y: 8,
            material: SAND,
        }))?;
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
        if snapshot.frame.terrain.sand == 0 {
            return Err("smoke scene contains no sand".into());
        }
        let instance = gpu::instance();
        let adapter = pollster::block_on(gpu::adapter(&instance, None, options.software))?;
        let mut gpu = pollster::block_on(Gpu::new(
            &adapter,
            wgpu::TextureFormat::Rgba8Unorm,
            options.width as u32,
            options.height as u32,
        ))?;
        gpu.smoke_tiled(&snapshot.frame.terrain.image)?;
        smoke::verify_updates(&mut gpu, options.width, options.height)?;
        println!("simulation_tick={} sand={} workers_used={}/{} tick_work_ms={:.3} (smoke, not FPS verification)",
            snapshot.tick, snapshot.frame.terrain.sand, snapshot.frame.terrain.workers_used, snapshot.frame.terrain.workers_configured, snapshot.work_ms);
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
