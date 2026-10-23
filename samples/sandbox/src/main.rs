mod actor_view;
mod combat;
mod cup;
mod debug_view;
mod game;
mod material_view;
mod materials;
mod scenes;
mod smoke;
mod trace;
mod world_view;
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

// Full is a recoverable UI condition; disconnection still stops the client.
fn submit_draw_command<S: charon_runtime::Simulation<Command = GameCommand>>(
    runtime: &Runtime<S>,
    command: GameCommand,
) -> Result<bool> {
    match runtime.submit(command) {
        Ok(()) => Ok(true),
        Err(std::sync::mpsc::TrySendError::Full(_)) => Ok(false),
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
            Err("simulation input disconnected".into())
        }
    }
}

#[derive(Clone, Copy)]
enum Tool {
    Lob,
    Wand,
    Brush,
    Blocks,
    Images,
}
impl Tool {
    fn label(self) -> &'static str {
        match self {
            Self::Lob => "E ARC SHOT CLICK FIRE Q STRAIGHT",
            Self::Wand => "Q WAND CLICK FIRE 3 BUILD",
            Self::Brush => "1 SAND 2 ASH 3 BLOCK 4 IMAGE 5 WATER 6 OIL L SEA",
            Self::Blocks => "3 BLOCK GRID 8",
            Self::Images => "4 IMAGE FREE",
        }
    }
}
struct Options {
    world: Option<u64>,
    world_game: bool,
    world_dir: PathBuf,
    game: bool,
    explore: Option<u64>,
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
            world: None,
            world_game: false,
            world_dir: "artifacts/local/worlds".into(),
            game: false,
            explore: None,
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
                kind @ ("--world" | "--world-game") => {
                    options.world_game = kind == "--world-game";
                    options.world = Some(value()?.parse()?);
                    options.play = true;
                    options.width = 1920;
                    options.height = 1080;
                }
                "--world-dir" => options.world_dir = value()?.into(),
                "--game" => {
                    options.game = true;
                    options.explore = Some(value()?.parse()?);
                    options.play = true;
                }
                "--explore" => {
                    options.explore = Some(value()?.parse()?);
                    options.play = true;
                }
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
                    println!("Charon sandbox\n--play: A/D move, Space jump, hold F dig, B toggle sand collision, J fires right\n--world-game SEED adds streamed enemies and combat; Enter begins\n--world SEED loads a Noita-scale streamed terrain prototype; --world-dir PATH selects chunk storage\n--game SEED starts the combat prototype: Enter to begin, R to retry\n--explore SEED starts a generated cave; G creates the next seed; R retries the same seed; Q straight shot, E gravity shot; click aims and fires\n--input-log FILE records authoritative inputs; --replay FILE validates without a window\n--fluid-backend gpu|cpu (default gpu; CPU is reference/fallback)\n--no-fluid-timing disables optional GPU timestamp queries\n--workers 1..32 --grid WIDTHxHEIGHT --record FILE --seconds N\n--smoke --software --ticks N\nC toggles follow camera; =/- zoom. P pauses/resumes; N pauses and advances one tick; I toggles cell inspection.\n1 sand / 2 ash brush; 3 grid blocks; 4 freely positioned images; 5 water / 6 oil brush; L sea test; K cup test, A/D move, O add oil. Mouse: left paint, right erase, middle grid block. R clears; Esc exits.");
                    std::process::exit(0);
                }
                _ => return Err(format!("unknown option: {argument}").into()),
            }
        }
        if options.world.is_some() && (options.game || options.explore.is_some() || options.smoke) {
            return Err("--world cannot be combined with --game, --explore or --smoke".into());
        }
        if options.world.is_some() && (options.width < 1024 || options.height < 1024) {
            return Err("streamed active windows need at least 1024 cells per axis for the current guard band".into());
        }
        if options.ticks == 0 || options.ticks > 3600 {
            return Err("ticks must be 1..3600".into());
        }
        Ok(options)
    }
}
struct App {
    world_session: Option<world_view::Session>,
    world_target: Option<[i64; 2]>,
    world_payload: Option<std::sync::Arc<[u8]>>,
    world_sent: bool,
    world_captured: Option<[i64; 2]>,
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
    rejected_edits: u64,
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
            writeln!(file, "elapsed_s,draw_fps,sim_hz,frame_p95_ms,frame_p99_ms,tick_work_ms,tick_late_ms,sand,active_tiles,visited_cells,workers_configured,workers_used,update_ms,snapshot_ms,upload_enqueue_ms,upload_bytes,acquire_ms,encode_present_ms,converted_cells,player_x,player_y,grounded,embedded,sand_solid,overlay_upload_bytes,runtime_tick,simulation_tick,paused,inspect_x,inspect_y,material,next_tile_awake,shots,sparks,impacts,sprite_upload_bytes,appearance_upload_bytes,atlas_upload_bytes,camera_x,camera_y,camera_width,camera_height,display_player_x,display_player_y,window_width,window_height,facing,placed_images,last_image_x,last_image_y,liquid,cup_x,cup_y,cup_width,cup_height,cup_moves,cup_blocked,water,oil,fluid_water_volume,fluid_oil_volume,fluid_divergence,fluid_substeps,fluid_pressure_ms,fluid_transport_ms,fluid_velocity_ms,fluid_flux_retries,fluid_max_speed,fluid_sleeping,fluid_surface_delta,fluid_rms_speed,fluid_mass_speed,fluid_fraction_change,fluid_compute_backend,fluid_gpu_velocity_ms,fluid_gpu_pressure_ms,fluid_gpu_upload_bytes,fluid_gpu_readback_bytes,fluid_gpu_dispatches,fluid_gpu_velocity_workgroups,fluid_gpu_pressure_workgroups,fluid_pressure_iterations,fluid_pressure_assembly_ms,fluid_pressure_setup_ms,fluid_gpu_readbacks,fluid_velocity_extension_ms,exploration_seed,run_phase,player_health,enemies,enemy_shots,kills,world_x,world_y,world_origin_x,world_origin_y,resident_chunks,world_waiting,shot_x,shot_y,shot_vx,shot_vy,shot_gravity,enemy_x,enemy_y,sleeping_enemies,sleeping_enemy_shots,rejected_enemy_shots,enemy_id,enemy_health")?;
        }
        let now = Instant::now();
        let actor = Some(actor_view::ActorVisual::new()?);
        let streaming = options.world.is_some();
        let world_session = if let Some(seed) = options.world {
            Some(world_view::Session::new(
                options.world_dir.join(seed.to_string()),
                seed,
                materials::palette().fingerprint(),
                options.width,
                options.height,
            )?)
        } else {
            None
        };
        let tool = if options.explore.is_some() || options.world.is_some() {
            Tool::Wand
        } else {
            Tool::Brush
        };
        Ok(Self {
            world_session,
            world_target: None,
            world_payload: None,
            world_sent: false,
            world_captured: None,
            material_visual: material_view::MaterialVisual::new()?,
            actor,
            options,
            runtime,
            view: None,
            cursor: None,
            buttons: [false; 3],
            brush: SAND,
            tool,
            clicks: [false; 3],
            follow_camera: streaming,
            zoom: if streaming { 4. } else { 1. },
            inspect_enabled: false,
            inspected: None,
            rejected_edits: 0,
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
        if let Err(error) = game::submit_input(&self.runtime, command) {
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
        if let (Some(session), Some(stream)) = (&mut self.world_session, snapshot.frame.stream) {
            if !stream.waiting {
                self.world_target = None;
                self.world_payload = None;
                self.world_sent = false;
                self.world_captured = None;
            } else if !self.world_sent {
                if self.world_target.is_none() {
                    if stream.loaded {
                        session
                            .capture(stream.origin, &snapshot.frame.terrain.indices.to_bytes())?;
                        self.world_captured = Some(stream.origin);
                    }
                    let p = snapshot.frame.player.expect("streamed player");
                    let target = [
                        stream.origin[0]
                            + if p.x() < self.options.width as f32 / 2. - 256. {
                                -512
                            } else if p.x() > self.options.width as f32 / 2. + 256. {
                                512
                            } else {
                                0
                            },
                        stream.origin[1]
                            + if p.y() < self.options.height as f32 / 2. - 256. {
                                -512
                            } else if p.y() > self.options.height as f32 / 2. + 256. {
                                512
                            } else {
                                0
                            },
                    ];
                    self.world_target = Some(target);
                }
                if self.world_payload.is_none() {
                    self.world_payload =
                        session.prepare(self.world_target.unwrap())?.map(Into::into);
                }
                if let Some(cells) = &self.world_payload {
                    match self.runtime.submit(GameCommand::LoadRegion {
                        origin: self.world_target.unwrap(),
                        cells: cells.clone(),
                    }) {
                        Ok(()) => self.world_sent = true,
                        Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                        Err(error) => return Err(format!("world handoff: {error}").into()),
                    }
                }
            }
        }
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
            Tool::Wand | Tool::Lob => pointer.map(|[x, y]| [x - 0.5, y - 0.5, 1., 1.]),
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
        let world_loading = snapshot.frame.stream.is_some_and(|s| s.waiting);
        if world_loading {
            self.rejected_edits += self.clicks.iter().filter(|&&c| c).count() as u64;
        }
        if let Some([x, y]) = pointer.filter(|_| !world_loading) {
            match self.tool {
                Tool::Wand | Tool::Lob => {
                    if self.clicks[0] {
                        self.rejected_edits += u64::from(!submit_draw_command(
                            &self.runtime,
                            if matches!(self.tool, Tool::Lob) {
                                GameCommand::LobAt([x, y])
                            } else {
                                GameCommand::FireAt([x, y])
                            },
                        )?);
                    }
                }
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
                            self.rejected_edits += u64::from(!submit_draw_command(
                                &self.runtime,
                                GameCommand::Terrain(command),
                            )?);
                        }
                    }
                }
                Tool::Blocks => {
                    let [bx, by, _, _] = preview.expect("block pointer preview");
                    if self.buttons[0] || self.buttons[1] {
                        self.rejected_edits += u64::from(!submit_draw_command(
                            &self.runtime,
                            GameCommand::Terrain(Command::FillRect {
                                x: bx as usize,
                                y: by as usize,
                                width: 8,
                                height: 8,
                                material: if self.buttons[1] { AIR } else { WALL },
                            }),
                        )?);
                    }
                }
                Tool::Images => {
                    if self.clicks[0] {
                        self.rejected_edits += u64::from(!submit_draw_command(
                            &self.runtime,
                            GameCommand::PlaceImage([x, y]),
                        )?);
                    }
                    if self.clicks[1] {
                        self.rejected_edits += u64::from(!submit_draw_command(
                            &self.runtime,
                            GameCommand::EraseImage([x, y]),
                        )?);
                    }
                }
            }
        }
        self.clicks.fill(false);
        let inspection = pointer
            .filter(|_| self.inspect_enabled)
            .map(|[x, y]| (x.floor() as i32, y.floor() as i32));
        if inspection != self.inspected {
            game::submit_input(&self.runtime, GameCommand::Inspect(inspection))
                .map_err(|e| format!("inspection queue: {e}"))?;
            self.inspected = inspection;
        }
        let input_label = if self.rejected_edits > 0 {
            format!("{} ACTIONS NOT APPLIED", self.rejected_edits)
        } else if let Some(stream) = snapshot.frame.stream {
            let p = snapshot.frame.player.expect("stream player");
            format!(
                "WORLD {} {}{}",
                stream.origin[0] + p.x() as i64,
                stream.origin[1] + p.y() as i64,
                if stream.waiting {
                    " LOADING"
                } else {
                    " Q WAND 3 BUILD"
                }
            )
        } else if self.rejected_edits == 0 {
            snapshot.frame.exploration_seed.map_or_else(
                || self.tool.label().to_owned(),
                |seed| format!("CAVE {seed} {} R RETRY", self.tool.label()),
            )
        } else {
            format!("QUEUE FULL: {} ACTIONS NOT APPLIED", self.rejected_edits)
        };
        let overlays = debug_view::overlays(
            &snapshot.frame,
            self.inspect_enabled,
            self.performance,
            view.software_adapter,
            camera,
            &input_label,
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
                snapshot.frame.state_label(),snapshot.frame.simulation_tick,snapshot.work_ms, frame.workers_used, frame.workers_configured, frame.sand));
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
            row.push(',');
            if let Some(cup) = snapshot.frame.cup {
                row.push_str(&format!("{:.6}", cup.fluid.pressure_assembly_ms));
            }
            if let Some(gpu) = snapshot.frame.cup.and_then(|c| c.fluid.gpu) {
                row.push_str(&format!(",{:.6},{}", gpu.pressure_setup_ms, gpu.readbacks));
            } else {
                row.push_str(",,");
            }
            row.push(',');
            if let Some(cup) = snapshot.frame.cup {
                row.push_str(&format!("{:.6}", cup.fluid.velocity_extension_ms));
            }
            row.push(',');
            if let Some(seed) = snapshot.frame.exploration_seed {
                row.push_str(&seed.to_string());
            }
            if let Some(combat) = &snapshot.frame.combat {
                row.push_str(&format!(
                    ",{:?},{},{},{},{}",
                    combat.phase,
                    combat.health,
                    combat.enemies.len(),
                    combat.shots.len(),
                    combat.kills
                ));
            } else {
                row.push_str(",,,,,");
            }
            if let Some(stream) = snapshot.frame.stream {
                let p = snapshot.frame.player.expect("world player");
                row.push_str(&format!(
                    ",{},{},{},{},{},{}",
                    stream.origin[0] + p.x() as i64,
                    stream.origin[1] + p.y() as i64,
                    stream.origin[0],
                    stream.origin[1],
                    self.world_session
                        .as_ref()
                        .map_or(0, |s| s.storage.resident_count()),
                    stream.waiting
                ));
            } else {
                row.push_str(",,,,,,");
            }
            if let Some(shot) = snapshot.frame.shot_motion {
                row.push_str(&format!(
                    ",{:.6},{:.6},{:.6},{:.6},{:.6}",
                    shot.position[0],
                    shot.position[1],
                    shot.velocity[0],
                    shot.velocity[1],
                    shot.acceleration[1]
                ));
            } else {
                row.push_str(",,,,,");
            }
            let nearest_enemy = snapshot.frame.combat.as_ref().and_then(|combat| {
                let player = snapshot.frame.player?;
                combat.enemies.iter().min_by(|a, b| {
                    let distance = |e: &crate::combat::EnemyView| {
                        (e.rect[0] - player.x()).hypot(e.rect[1] - player.y())
                    };
                    distance(a).total_cmp(&distance(b))
                })
            });
            if let Some(enemy) = nearest_enemy {
                row.push_str(&format!(
                    ",{:.6},{:.6}",
                    enemy.rect[0] + enemy.rect[2] / 2.,
                    enemy.rect[1] + enemy.rect[3] / 2.
                ));
            } else {
                row.push_str(",,");
            }
            if let Some(combat) = &snapshot.frame.combat {
                row.push_str(&format!(
                    ",{},{},{}",
                    combat.sleeping, combat.sleeping_shots, combat.rejected_shots
                ));
            } else {
                row.push_str(",,,");
            }
            if let Some(enemy) = nearest_enemy {
                row.push_str(&format!(",{},{}", enemy.id, enemy.health));
            } else {
                row.push_str(",,");
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
            WindowEvent::CloseRequested => {
                self.runtime.request_stop();
                event_loop.exit();
            }
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
                    PhysicalKey::Code(KeyCode::Enter) if down => {
                        self.game_command(GameCommand::Begin)
                    }
                    PhysicalKey::Code(KeyCode::KeyQ) if down => self.tool = Tool::Wand,
                    PhysicalKey::Code(KeyCode::KeyE) if down => self.tool = Tool::Lob,
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
                    PhysicalKey::Code(KeyCode::KeyC) if down && self.options.world.is_none() => {
                        self.follow_camera = !self.follow_camera;
                        self.zoom = if self.follow_camera { 2. } else { 1. };
                    }
                    PhysicalKey::Code(KeyCode::Equal) if down => {
                        self.zoom = (self.zoom * 1.25).min(4.)
                    }
                    PhysicalKey::Code(KeyCode::Minus) if down => {
                        self.zoom = (self.zoom / 1.25).max(if self.options.world.is_some() {
                            4.
                        } else {
                            1.
                        })
                    }
                    PhysicalKey::Code(KeyCode::KeyG) if down => {
                        let seed = self
                            .runtime
                            .latest()
                            .and_then(|s| s.frame.exploration_seed)
                            .unwrap_or(0)
                            .wrapping_add(1);
                        self.game_command(GameCommand::Explore(seed));
                    }
                    PhysicalKey::Code(KeyCode::KeyJ) if down => {
                        self.game_command(GameCommand::Fire)
                    }
                    PhysicalKey::Code(KeyCode::KeyB) if down => {
                        self.game_command(GameCommand::ToggleSand)
                    }
                    PhysicalKey::Code(KeyCode::Escape) if down => {
                        self.runtime.request_stop();
                        event_loop.exit();
                    }
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
            self.runtime.request_stop();
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
    if options.world.is_some() {
        println!("world_profile=noita base={:?} main_capture={:?} horizontal_period={} (capture bounds are not world limits)",world_view::BASE,world_view::MAIN_CAPTURE,world_view::PARALLEL_PERIOD);
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
    if let Some(seed) = options.world {
        runtime.submit(if options.world_game {
            GameCommand::StartWorldGame(seed)
        } else {
            GameCommand::StartWorld(seed)
        })?;
    }
    if let Some(seed) = options.explore {
        runtime.submit(if options.game {
            GameCommand::NewRun(seed)
        } else {
            GameCommand::Explore(seed)
        })?;
    }
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
    app.runtime
        .finish()
        .map_err(|_| "simulation worker failed during shutdown")?;
    if let Some(session) = &mut app.world_session {
        if let Some(snapshot) = app.runtime.latest() {
            if let Some(stream) = snapshot.frame.stream.filter(|s| s.loaded) {
                if !stream.waiting || app.world_captured != Some(stream.origin) {
                    session.capture(stream.origin, &snapshot.frame.terrain.indices.to_bytes())?;
                }
            }
        }
        session.flush()?;
    }
    if let Some(error) = app.error {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn draw_queue_full_is_recoverable_but_disconnection_is_fatal() {
        struct Gated {
            game: Game,
            entered: mpsc::SyncSender<()>,
            resume: mpsc::Receiver<()>,
            gate: bool,
        }
        impl charon_runtime::Simulation for Gated {
            type Command = GameCommand;
            type Frame = (Vec<[f32; 2]>, Option<(i32, i32)>);
            fn step(&mut self, commands: Vec<GameCommand>) -> Self::Frame {
                if self.gate {
                    self.gate = false;
                    self.entered.send(()).unwrap();
                    self.resume.recv_timeout(Duration::from_secs(2)).unwrap();
                }
                let frame = self.game.step(commands);
                (frame.images, frame.inspection.map(|cell| (cell.x, cell.y)))
            }
        }
        let (entered_tx, entered) = mpsc::sync_channel(1);
        let (resume, resume_rx) = mpsc::sync_channel(1);
        let runtime = Runtime::start(Gated {
            game: Game::new(160, 120, 1, true).unwrap(),
            entered: entered_tx,
            resume: resume_rx,
            gate: true,
        })
        .unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..127 {
            runtime
                .submit(GameCommand::PlaceImage([1.25, 2.5]))
                .unwrap();
        }
        runtime.submit(GameCommand::Inspect(None)).unwrap();
        let rejected = submit_draw_command(&runtime, GameCommand::EraseImage([1.25, 2.5]));
        for point in [(10, 11), (20, 21)] {
            game::submit_input(&runtime, GameCommand::Inspect(Some(point))).unwrap();
        }
        resume.send(()).unwrap();
        assert!(
            matches!(rejected, Ok(false)),
            "Full must keep the draw loop alive: {rejected:?}"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while runtime.latest().is_none_or(|s| s.tick < 2) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let snapshot = runtime.latest().unwrap();
        assert_eq!(snapshot.frame.0, vec![[1.25, 2.5]; 127]);
        assert_eq!(snapshot.frame.1, Some((20, 21)));
        assert!(submit_draw_command(&runtime, GameCommand::PlaceImage([3.5, 4.25])).unwrap());
        while runtime.latest().is_none_or(|s| s.frame.0.len() != 128) {
            assert!(Instant::now() < deadline, "editing did not recover");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(runtime.latest().unwrap().frame.0.last(), Some(&[3.5, 4.25]));
        // A failed worker must still be reported as fatal.
        struct Finished;
        impl charon_runtime::Simulation for Finished {
            type Command = GameCommand;
            type Frame = ();
            fn step(&mut self, _: Vec<GameCommand>) {
                panic!("simulated worker failure");
            }
        }
        let mut stopped = Runtime::start(Finished).unwrap();
        while !stopped.is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(submit_draw_command(&stopped, GameCommand::Inspect(None)).is_err());
        assert!(
            stopped.finish().is_err(),
            "host must observe worker failure at shutdown"
        );
    }
}
