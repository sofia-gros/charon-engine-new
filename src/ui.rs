//! egui overlay: stats, material palette, scenario picker, history scrubber.

use sandengine::sim::material::Cell;
use sandengine::sim::scenarios;

pub struct UiState {
    pub show_help: bool,
}

impl UiState {
    pub fn new() -> Self {
        Self { show_help: false }
    }
}

#[derive(Default)]
pub struct UiAction {
    pub toggle_pause: bool,
    pub reset: bool,
    pub clear_history: bool,
}

pub struct FrameStats {
    pub fps: f32,
    pub sim_ms: f32,
    pub render_ms: f32,
    pub particles: usize,
    pub dirty_chunks: usize,
    pub total_chunks: usize,
    pub tick: u64,
}

const PALETTE: &[(Cell, &str, egui::Color32)] = &[
    (Cell::Sand, "Sand", egui::Color32::from_rgb(215, 192, 132)),
    (Cell::Water, "Water", egui::Color32::from_rgb(35, 98, 210)),
    (Cell::Stone, "Stone", egui::Color32::from_rgb(118, 120, 126)),
    (Cell::Fire, "Fire", egui::Color32::from_rgb(255, 145, 40)),
    (
        Cell::Gravel,
        "Gravel",
        egui::Color32::from_rgb(118, 108, 98),
    ),
    (Cell::Oil, "Oil", egui::Color32::from_rgb(58, 38, 18)),
    (Cell::Acid, "Acid", egui::Color32::from_rgb(48, 210, 60)),
    (Cell::Steam, "Steam", egui::Color32::from_rgb(210, 210, 220)),
];

pub fn install_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_corner_radius = egui::CornerRadius::same(8);
    visuals.window_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: egui::Color32::from_black_alpha(140),
    };
    visuals.window_fill = egui::Color32::from_rgba_unmultiplied(14, 16, 22, 232);
    visuals.panel_fill = egui::Color32::from_rgba_unmultiplied(14, 16, 22, 232);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(20, 24, 32);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(30, 36, 48);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(48, 56, 72);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(60, 88, 140);
    visuals.selection.bg_fill = egui::Color32::from_rgb(48, 110, 200);
    ctx.set_visuals(visuals);
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    ctx: &egui::Context,
    stats: &FrameStats,
    paused: bool,
    scenario_idx: &mut usize,
    brush_size: &mut u32,
    sim_speed: &mut f32,
    history_cursor: &mut usize,
    history_len: usize,
    ui_state: &mut UiState,
    selected: &mut Cell,
    bloom_strength: &mut f32,
) -> UiAction {
    let mut action = UiAction::default();

    egui::Window::new("sandengine")
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(12.0, 12.0))
        .resizable(false)
        .collapsible(true)
        .default_open(true)
        .show(ctx, |ui| {
            ui.set_width(260.0);

            ui.label(
                egui::RichText::new("Falling-sand cellular automaton")
                    .size(11.0)
                    .color(egui::Color32::from_rgb(160, 170, 190)),
            );

            ui.add_space(8.0);

            // Stats
            ui.group(|ui| {
                ui.set_min_width(240.0);
                ui.label(egui::RichText::new("Performance").strong().size(12.0));
                ui.label(
                    egui::RichText::new(format!(
                        "{:.0} fps   ·   sim {:.1} ms   ·   render {:.1} ms",
                        stats.fps, stats.sim_ms, stats.render_ms,
                    ))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(180, 190, 210)),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{} particles   ·   {}/{} chunks active   ·   tick {}",
                        stats.particles, stats.dirty_chunks, stats.total_chunks, stats.tick,
                    ))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(180, 190, 210)),
                );
            });

            ui.add_space(6.0);

            // Materials
            ui.label(egui::RichText::new("Material  (1-8)").strong().size(12.0));
            egui::Grid::new("palette_grid")
                .num_columns(4)
                .spacing([6.0, 6.0])
                .show(ui, |ui| {
                    for (i, (mat, name, color)) in PALETTE.iter().enumerate() {
                        let active = *selected == *mat;
                        let mut text = egui::RichText::new(*name).size(11.0);
                        text = if active {
                            text.color(egui::Color32::WHITE).strong()
                        } else {
                            text.color(egui::Color32::from_rgb(220, 220, 230))
                        };
                        let fill = if active {
                            *color
                        } else {
                            egui::Color32::from_rgba_unmultiplied(
                                color.r(),
                                color.g(),
                                color.b(),
                                120,
                            )
                        };
                        let btn = egui::Button::new(text)
                            .fill(fill)
                            .corner_radius(egui::CornerRadius::same(4))
                            .min_size(egui::vec2(54.0, 26.0));
                        if ui.add(btn).clicked() {
                            *selected = *mat;
                        }
                        if i % 4 == 3 {
                            ui.end_row();
                        }
                    }
                });

            ui.add_space(6.0);
            ui.separator();

            ui.add(egui::Slider::new(brush_size, 1..=60).text("brush"));
            ui.add(
                egui::Slider::new(sim_speed, 0.1..=10.0)
                    .logarithmic(true)
                    .text("speed"),
            );
            ui.add(egui::Slider::new(bloom_strength, 0.0..=2.0).text("bloom"));

            ui.add_space(4.0);

            let current = scenarios::NAMES[*scenario_idx % scenarios::NAMES.len()];
            egui::ComboBox::from_label("Scenario")
                .selected_text(current)
                .width(180.0)
                .show_ui(ui, |ui| {
                    for (i, name) in scenarios::NAMES.iter().enumerate() {
                        if ui.selectable_label(i == *scenario_idx, *name).clicked() {
                            *scenario_idx = i;
                        }
                    }
                });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let pause_label = if paused {
                    "▶  Play  (Space)"
                } else {
                    "❚❚  Pause  (Space)"
                };
                if ui.button(pause_label).clicked() {
                    action.toggle_pause = true;
                }
                if ui.button("Reset  (R)").clicked() {
                    action.reset = true;
                }
                if ui.button("Next  (Tab)").clicked() {
                    *scenario_idx = (*scenario_idx + 1) % scenarios::NAMES.len();
                }
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.small_button("Help  (H)").clicked() {
                    ui_state.show_help = !ui_state.show_help;
                }
                if ui.small_button("Clear history").clicked() {
                    action.clear_history = true;
                }
            });
        });

    // History scrubber: appears when paused.
    if paused && history_len > 0 {
        egui::Window::new("history")
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -16.0))
            .title_bar(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("⏺ TIME SCRUB")
                            .color(egui::Color32::from_rgb(120, 200, 255))
                            .strong()
                            .size(13.0),
                    );
                    let max = history_len.saturating_sub(1);
                    let mut c = *history_cursor as f64;
                    let resp = ui.add(
                        egui::Slider::new(&mut c, 0.0..=(max as f64))
                            .show_value(true)
                            .integer()
                            .text(format!("frame / {max}")),
                    );
                    if resp.changed() { *history_cursor = c as usize; }
                });
                ui.label(
                    egui::RichText::new(format!(
                        "Snapshot ring buffer  ·  {history_len} frames  ·  ←/→ step, Shift+←/→ jump 50"
                    ))
                    .size(10.0)
                    .color(egui::Color32::from_rgb(160, 170, 190)),
                );
            });
    }

    if ui_state.show_help {
        egui::Window::new("Controls")
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 12.0))
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_min_width(230.0);
                let rows: &[(&str, &str)] = &[
                    ("1 – 8", "select material"),
                    ("Left-click", "paint / grab block"),
                    ("Right-click", "erase"),
                    ("Scroll", "brush size"),
                    ("Space", "pause / resume"),
                    ("← / →", "scrub history"),
                    ("Shift+←/→", "jump 50 frames"),
                    ("Tab", "next scenario"),
                    ("R", "reset scenario"),
                    ("+ / -", "sim speed"),
                    ("H", "toggle this help"),
                ];
                egui::Grid::new("help_grid")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (k, v) in rows {
                            ui.label(
                                egui::RichText::new(*k)
                                    .monospace()
                                    .color(egui::Color32::from_rgb(180, 220, 255)),
                            );
                            ui.label(
                                egui::RichText::new(*v)
                                    .color(egui::Color32::from_rgb(220, 220, 230)),
                            );
                            ui.end_row();
                        }
                    });
            });
    }

    action
}
