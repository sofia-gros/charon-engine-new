//! Windowing, event loop and frame management.
//!
//! Time-stepping uses the canonical "accumulator" pattern from Glenn
//! Fiedler's *Fix Your Timestep!* (gafferongames.com, 2004): real frame
//! time accumulates, the physics steps in fixed `SIM_DT` increments, and we
//! cap iterations per frame so a long stall doesn't produce a death spiral.

use crate::input::InputState;
use crate::render::context::GpuContext;
use crate::render::renderer::Renderer;
use crate::ui::{self, UiState};
use glam::Vec2;
use rand::RngExt;
use sandengine::sim::block::Block;
use sandengine::sim::grid::Grid;
use sandengine::sim::history::History;
use sandengine::sim::material::{Cell, CellData};
use sandengine::sim::{physics, scenarios};
use std::sync::Arc;
use std::time::Instant;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::KeyCode;
use winit::window::Window;

const GRID_W: usize = 1280;
const GRID_H: usize = 720;
const SIM_DT: f32 = 1.0 / 120.0;
const HISTORY_CAP: usize = 600;

/// Command-line options parsed by `main.rs`.
#[derive(Default)]
pub struct AppOptions {
    pub initial_scenario: usize,
}

pub struct App {
    pub window: Arc<Window>,
    gpu: GpuContext,

    grid: Grid,
    block: Block,
    history: History,
    scenario: usize,
    renderer: Renderer,

    paused: bool,
    sim_speed: f32,
    tick: u64,

    input: InputState,
    ui_state: UiState,
    brush_size: u32,
    selected_material: Cell,
    egui_consumed: bool,

    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,

    rng: rand::rngs::ThreadRng,
    last_frame: Instant,
    sim_accumulator: f32,
    frame_count: u32,
    fps_timer: f32,
    fps: f32,
    sim_ms: f32,
    render_ms: f32,
}

impl App {
    pub fn new(event_loop: &ActiveEventLoop, options: AppOptions) -> Self {
        let attrs = Window::default_attributes()
            .with_title("sandengine — 2D falling-sand cellular automaton")
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 720));
        let window = Arc::new(event_loop.create_window(attrs).unwrap());

        let gpu = GpuContext::new(window.clone());
        let renderer = Renderer::new(&gpu.device, gpu.format(), GRID_W as u32, GRID_H as u32);

        let mut rng = rand::rng();
        let mut grid = Grid::new(GRID_W, GRID_H);
        let mut block = Block::new(0.0, 0.0, 30.0, 15.0);
        let initial_scenario = options.initial_scenario % scenarios::NAMES.len();
        scenarios::apply(initial_scenario, &mut grid, &mut block, &mut rng);

        let egui_ctx = egui::Context::default();
        ui::install_style(&egui_ctx);
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            Some(winit::window::Theme::Dark),
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            gpu.format(),
            egui_wgpu::RendererOptions::default(),
        );

        Self {
            window,
            gpu,
            grid,
            block,
            history: History::new(HISTORY_CAP),
            scenario: initial_scenario,
            renderer,
            paused: false,
            sim_speed: 3.0,
            tick: 0,
            input: InputState::new(),
            ui_state: UiState::new(),
            brush_size: 8,
            selected_material: Cell::Sand,
            egui_consumed: false,
            egui_ctx,
            egui_state,
            egui_renderer,
            rng,
            last_frame: Instant::now(),
            sim_accumulator: 0.0,
            frame_count: 0,
            fps_timer: 0.0,
            fps: 60.0,
            sim_ms: 0.0,
            render_ms: 0.0,
        }
    }

    pub fn handle_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        let resp = self.egui_state.on_window_event(&self.window, &event);
        self.egui_consumed = resp.consumed;
        if !resp.consumed {
            self.input.handle_event(&event);
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.gpu.resize(size.width, size.height),
            WindowEvent::RedrawRequested => self.frame(),
            _ => {}
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        self.fps_timer += dt;
        self.frame_count += 1;
        if self.fps_timer >= 0.5 {
            self.fps = self.frame_count as f32 / self.fps_timer;
            self.frame_count = 0;
            self.fps_timer = 0.0;
        }

        self.input.prepare_frame();
        self.handle_keys();
        if !self.egui_consumed {
            self.handle_brush();
        }

        if !self.paused {
            self.sim_accumulator += dt * self.sim_speed;
            let mut steps = 0;
            let sim_start = Instant::now();
            while self.sim_accumulator >= SIM_DT && steps < 8 {
                self.sim_accumulator -= SIM_DT;
                steps += 1;
                self.step_sim();
            }
            self.sim_ms = sim_start.elapsed().as_secs_f32() * 1000.0;
            if self.sim_accumulator > SIM_DT * 3.0 {
                self.sim_accumulator = 0.0;
            }
        }

        self.render_frame();
        self.input.end_frame();
    }

    fn handle_keys(&mut self) {
        if self.input.just_pressed(KeyCode::Space) {
            self.paused = !self.paused;
        }
        if self.input.just_pressed(KeyCode::KeyH) {
            self.ui_state.show_help = !self.ui_state.show_help;
        }
        if self.input.just_pressed(KeyCode::KeyR) {
            self.reset_scenario();
        }
        if self.input.just_pressed(KeyCode::Tab) {
            self.next_scenario();
        }
        if self.input.just_pressed(KeyCode::Equal) || self.input.just_pressed(KeyCode::NumpadAdd) {
            self.sim_speed = (self.sim_speed * 2.0).min(10.0);
        }
        if self.input.just_pressed(KeyCode::Minus)
            || self.input.just_pressed(KeyCode::NumpadSubtract)
        {
            self.sim_speed = (self.sim_speed * 0.5).max(0.1);
        }

        if self.input.just_pressed(KeyCode::Digit1) {
            self.selected_material = Cell::Sand;
        }
        if self.input.just_pressed(KeyCode::Digit2) {
            self.selected_material = Cell::Water;
        }
        if self.input.just_pressed(KeyCode::Digit3) {
            self.selected_material = Cell::Stone;
        }
        if self.input.just_pressed(KeyCode::Digit4) {
            self.selected_material = Cell::Fire;
        }
        if self.input.just_pressed(KeyCode::Digit5) {
            self.selected_material = Cell::Gravel;
        }
        if self.input.just_pressed(KeyCode::Digit6) {
            self.selected_material = Cell::Oil;
        }
        if self.input.just_pressed(KeyCode::Digit7) {
            self.selected_material = Cell::Acid;
        }
        if self.input.just_pressed(KeyCode::Digit8) {
            self.selected_material = Cell::Steam;
        }

        if self.paused {
            let jump = if self.input.shift_held { 50 } else { 1 };
            if self.input.just_pressed(KeyCode::ArrowLeft) {
                self.history.cursor = self.history.cursor.saturating_sub(jump);
                self.restore_history();
            }
            if self.input.just_pressed(KeyCode::ArrowRight) {
                let max = self.history.len().saturating_sub(1);
                self.history.cursor = (self.history.cursor + jump).min(max);
                self.restore_history();
            }
        }
    }

    fn handle_brush(&mut self) {
        if self.input.scroll_delta != 0.0 {
            let new = self.brush_size as i32 + if self.input.scroll_delta > 0.0 { 2 } else { -2 };
            self.brush_size = new.clamp(1, 60) as u32;
        }

        let (ww, wh) = {
            let s = self.window.inner_size();
            (s.width as f32, s.height as f32)
        };
        let gx = self.input.mouse_pos.x / ww * self.grid.w as f32;
        let gy = self.input.mouse_pos.y / wh * self.grid.h as f32;

        if self.input.left_just_pressed() && self.block.contains(gx, gy) {
            self.block.grabbed = true;
            self.block.grab_offset = self.block.pos - Vec2::new(gx, gy);
        }
        if self.input.left_just_released() {
            self.block.grabbed = false;
        }

        if self.block.grabbed && self.input.left_down() {
            let target = Vec2::new(gx, gy) + self.block.grab_offset;
            self.block.move_and_displace(target, &mut self.grid);
        } else if self.input.left_down() && !self.block.grabbed {
            self.paint_brush(gx as i32, gy as i32, false);
        }
        if self.input.right_down() {
            self.paint_brush(gx as i32, gy as i32, true);
        }
    }

    fn paint_brush(&mut self, cx: i32, cy: i32, erase: bool) {
        let r = self.brush_size as i32;
        let mat = self.selected_material;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy > r * r {
                    continue;
                }
                let x = cx + dx;
                let y = cy + dy;
                if !self.grid.in_bounds(x, y) {
                    continue;
                }
                let (ux, uy) = (x as usize, y as usize);
                let kind = self.grid.get(ux, uy).kind;
                if erase {
                    if kind != Cell::Air && kind != Cell::Block {
                        self.grid.set(ux, uy, CellData::AIR);
                    }
                } else if kind == Cell::Air {
                    let cell = match mat {
                        Cell::Sand => CellData::sand(self.rng.random()),
                        Cell::Water => CellData::water(self.rng.random()),
                        Cell::Stone => CellData::stone(self.rng.random()),
                        Cell::Fire => CellData::fire(self.rng.random_range(60..120)),
                        Cell::Steam => CellData::steam(self.rng.random_range(40..80)),
                        Cell::Gravel => CellData::gravel(self.rng.random()),
                        Cell::Oil => CellData::oil(self.rng.random()),
                        Cell::Acid => CellData::acid(self.rng.random_range(60..100)),
                        _ => CellData::sand(self.rng.random()),
                    };
                    self.grid.set(ux, uy, cell);
                }
            }
        }
    }

    fn step_sim(&mut self) {
        // The block stays rasterized as immovable `Cell::Block` between ticks;
        // physics already ignores it. Only `move_and_displace` (called from
        // `handle_brush`) clears/re-rasterizes when the block actually moves.
        scenarios::spawner_tick(self.scenario, &mut self.grid, &mut self.rng);
        physics::step(&mut self.grid, &mut self.rng);
        self.history.record(&self.grid.cells, &self.block);
        self.tick += 1;
    }

    fn restore_history(&mut self) {
        if let Some(snap) = self.history.scrub(self.history.cursor) {
            self.grid.cells.copy_from_slice(&snap.cells);
            // After scrubbing, every chunk needs re-evaluation.
            self.grid.dirty.fill(true);
            self.grid.dirty_next.fill(true);
            self.block = snap.block.clone();
        }
    }

    fn reset_scenario(&mut self) {
        scenarios::apply(
            self.scenario,
            &mut self.grid,
            &mut self.block,
            &mut self.rng,
        );
        self.history.clear();
        self.tick = 0;
    }

    fn next_scenario(&mut self) {
        self.scenario = (self.scenario + 1) % scenarios::NAMES.len();
        self.reset_scenario();
    }

    fn render_frame(&mut self) {
        let render_start = Instant::now();
        let surface_tex = match self.gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                let (w, h) = self.gpu.size();
                self.gpu.resize(w, h);
                return;
            }
            _ => return,
        };
        let surface_view = surface_tex
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let raw_input = self.egui_state.take_egui_input(&self.window);

        let mut scenario_idx = self.scenario;
        let mut history_cursor = self.history.cursor;
        let history_len = self.history.len();
        let particles = self.grid.particle_count();
        let dirty_chunks = self.grid.dirty_chunk_count();
        let total_chunks = self.grid.chunks_x * self.grid.chunks_y;

        let frame_stats = ui::FrameStats {
            fps: self.fps,
            sim_ms: self.sim_ms,
            render_ms: self.render_ms,
            particles,
            dirty_chunks,
            total_chunks,
            tick: self.tick,
        };

        let mut ui_action = ui::UiAction::default();
        #[allow(deprecated)]
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            ui_action = ui::draw(
                ctx,
                &frame_stats,
                self.paused,
                &mut scenario_idx,
                &mut self.brush_size,
                &mut self.sim_speed,
                &mut history_cursor,
                history_len,
                &mut self.ui_state,
                &mut self.selected_material,
                &mut self.renderer.bloom_strength,
            );
        });

        if ui_action.toggle_pause {
            self.paused = !self.paused;
        }
        if ui_action.reset {
            self.reset_scenario();
        }
        if ui_action.clear_history {
            self.history.clear();
        }

        if scenario_idx != self.scenario {
            self.scenario = scenario_idx;
            self.reset_scenario();
        }

        if self.paused && history_cursor != self.history.cursor {
            self.history.cursor = history_cursor;
            self.restore_history();
        }

        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);
        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let (sw, sh) = self.gpu.size();
        let screen_desc = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [sw, sh],
            pixels_per_point: self.window.scale_factor() as f32,
        };

        for (id, delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
        }

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame_encoder"),
            });

        self.egui_renderer.update_buffers(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &paint_jobs,
            &screen_desc,
        );

        self.renderer.upload(&self.gpu.queue, &self.grid);

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sand_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.03,
                            g: 0.04,
                            b: 0.07,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            self.renderer.render(&mut rpass);
            self.egui_renderer
                .render(&mut rpass.forget_lifetime(), &paint_jobs, &screen_desc);
        }

        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        surface_tex.present();

        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        self.render_ms = render_start.elapsed().as_secs_f32() * 1000.0;
    }
}
