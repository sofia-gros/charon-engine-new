mod config;
mod diagnostics;
mod materials;
mod settings;
use charon_assets::AssetsPlugin;
use charon_core::App;
use charon_input::InputPlugin;
use charon_m5_sample::{
    host::{self, RunOptions},
    scenes,
};
use charon_render::gpu::GpuRenderer;
use charon_scene::SceneManager;
use charon_tracing_gpu::{TracingPlugin, wgpu};
use config::{Preferences, Selection};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};
static PREFERENCES: OnceLock<Mutex<Preferences>> = OnceLock::new();
static CONFIG_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
fn preferences() -> Preferences {
    *PREFERENCES.get().unwrap().lock().unwrap()
}
fn factory(ctx: scenes::Context) -> Result<SceneManager, String> {
    let mut manager = SceneManager::new();
    let menu = ctx.clone();
    manager.register("menu", move |_| {
        let mut app = App::new();
        let (plugin, client) = InputPlugin::new(256)?;
        app.add_plugin(plugin).map_err(|e| e.to_string())?;
        app.add_plugin(AssetsPlugin::default())
            .map_err(|e| e.to_string())?;
        app.add_plugin(TracingPlugin::idle())
            .map_err(|e| e.to_string())?;
        Ok(scenes::menu_scene(menu.clone(), app, client))
    })?;
    for (name, coupled) in [("sand-game", true), ("tile-game", false)] {
        let ctx = ctx.clone();
        manager.register(name, move |_| {
            let mut app = App::new();
            app.add_plugin(AssetsPlugin::default())
                .map_err(|e| e.to_string())?;
            app.add_plugin(TracingPlugin::idle())
                .map_err(|e| e.to_string())?;
            Ok(scenes::game_scene(ctx.clone(), app, name, coupled))
        })?;
    }
    manager.switch_to("menu")?;
    Ok(manager)
}
fn renderer(window: Arc<charon_render::winit::window::Window>) -> Result<GpuRenderer, String> {
    let backends = if cfg!(target_os = "macos") {
        wgpu::Backends::METAL
    } else {
        wgpu::Backends::VULKAN
    };
    let optional =
        if preferences().mobile || !preferences().enable_rtx_device || !cfg!(feature = "rtx") {
            wgpu::Features::empty()
        } else {
            charon_tracing_gpu::rtx_features()
        };
    match pollster::block_on(GpuRenderer::with_optional_features(
        window.clone(),
        backends,
        optional,
    )) {
        Ok(renderer) => Ok(renderer),
        Err(error) if !optional.is_empty() => {
            eprintln!("RTX device creation failed; not applied: {error}");
            pollster::block_on(GpuRenderer::with_optional_features(
                window,
                backends,
                wgpu::Features::empty(),
            ))
        }
        Err(error) => Err(error),
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--trace-check") {
        return diagnostics::run(&args);
    }

    let config = args
        .iter()
        .position(|a| a == "--config")
        .map(|i| {
            args.get(i + 1)
                .map(PathBuf::from)
                .ok_or("missing config path")
        })
        .transpose()?;
    let mut prefs = if let Some(path) = &config {
        Preferences::parse(&std::fs::read_to_string(path)?)?
    } else if args.iter().any(|a| a == "--mobile") {
        Preferences {
            mobile: true,
            lighting: Selection::LitePath,
            ..Default::default()
        }
    } else {
        Preferences::default()
    };
    if args.iter().any(|a| a == "--mobile") {
        prefs.mobile = true;
    }
    if let Some(i) = args.iter().position(|a| a == "--lighting") {
        prefs.pending = None;
        prefs.lighting = match args.get(i + 1).ok_or("missing lighting mode")?.as_str() {
            "rtx" => Selection::Rtx,
            "path" => Selection::Path,
            "lite_path" => Selection::LitePath,
            "m5" => Selection::M5,
            _ => return Err("lighting must be rtx, path, lite_path or m5".into()),
        };
    }
    prefs.validate()?;
    PREFERENCES.set(Mutex::new(prefs)).unwrap();
    CONFIG_PATH.set(config).unwrap();
    if args.iter().any(|a| a == "--smoke-test") {
        scenes::smoke_with(factory)?;
        println!("Tracing slot lifecycle and Tilemap/Sand workers OK");
        return Ok(());
    }
    host::run(RunOptions {
        title: "Charon RT / Lighting Lab",
        ready_marker: "M6 READY: first frame presented",
        manager: factory,
        renderer,
        frame_hook: Some(settings::frame),
        input_filter: Some(settings::input),
        ..Default::default()
    })
}
