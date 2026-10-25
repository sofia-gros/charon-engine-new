//! Same game definitions and owner scopes; only composition differs.
use charon_engine::{
    App, AppBuilder, Lane, Rates, SceneCatalog, Startup,
    assets::AssetsPlugin,
    bundles::{Assets, Interaction2D},
    input::InputPlugin,
    scenes::SceneManager,
};
use charon_m5_sample::{
    controller,
    scenes::{self, Context},
};

pub fn high(ctx: Context) -> Result<SceneManager, String> {
    let startup = Startup::new(Rates::new(144, 60, ctx.hz)?, 8)?;
    let menu = ctx.clone();
    let mut catalog = SceneCatalog::new(startup)?.scene("menu", move |_, app| {
        let bundle = Interaction2D::new(controller::camera(menu.size.get()), 256)?;
        let input = bundle.client();
        let mut app = app.bundle(bundle).map_err(|e| e.to_string())?.build();
        crate::configure(&mut app, true);
        Ok(scenes::menu_scene(menu.clone(), app, input))
    })?;
    for (name, coupled) in [("sand-game", true), ("tile-game", false)] {
        let ctx = ctx.clone();
        catalog = catalog.scene(name, move |_, app| {
            let mut app = app.bundle(Assets).map_err(|e| e.to_string())?.build();
            crate::configure(&mut app, true);
            Ok(scenes::game_scene(ctx.clone(), app, name, coupled))
        })?;
    }
    catalog.build("menu")
}
pub fn low(ctx: Context) -> Result<SceneManager, String> {
    let delta = std::time::Duration::from_secs_f64(1. / 144.);
    let mut manager = SceneManager::with_timing(delta, 8)?;
    let menu = ctx.clone();
    manager.register("menu", move |_| {
        let mut app = App::with_timing(delta, 8).map_err(|e| e.to_string())?;
        let (plugin, input) = InputPlugin::new(256)?;
        app.add_plugin(plugin).map_err(|e| e.to_string())?;
        app.add_plugin(AssetsPlugin::default())
            .map_err(|e| e.to_string())?;
        app.add_plugin(charon_engine::camera::CameraPlugin::new(
            controller::camera(menu.size.get()),
        ))
        .map_err(|e| e.to_string())?;
        app.world_mut().insert(input.clone());
        crate::configure(&mut app, false);
        Ok(scenes::menu_scene(menu.clone(), app, input))
    })?;
    for (name, coupled) in [("sand-game", true), ("tile-game", false)] {
        let ctx = ctx.clone();
        manager.register(name, move |_| {
            let mut app = App::with_timing(delta, 8).map_err(|e| e.to_string())?;
            app.add_plugin(AssetsPlugin::default())
                .map_err(|e| e.to_string())?;
            crate::configure(&mut app, false);
            Ok(scenes::game_scene(ctx.clone(), app, name, coupled))
        })?;
    }
    manager.switch_to("menu")?;
    Ok(manager)
}
type InitialState = (Vec<Option<u16>>, [f32; 2], f32);
fn state(
    factory: fn(Context) -> Result<SceneManager, String>,
    coupled: bool,
    hz: u32,
) -> Result<InitialState, String> {
    use std::{cell::Cell, rc::Rc};
    let ctx = Context {
        graphics: None,
        size: Rc::new(Cell::new([960, 696])),
        hz,
        prefer_sand: coupled,
    };
    let mut manager = factory(ctx)?;
    scenes::pulse(&manager, charon_engine::input::keys::ENTER)?;
    scenes::wait(&mut manager, |m| scenes::session(m).is_some())?;
    // Both APIs start the same world. An edit, restart, and worker cleanup are
    // exercised separately through smoke_with for every clock/mode combination.
    let session = scenes::session(&manager).unwrap();
    let view = session
        .runtime
        .as_ref()
        .unwrap()
        .shared
        .view
        .lock()
        .unwrap()
        .clone();
    let camera = *session.camera.lock().unwrap();
    let cells = (0..24)
        .flat_map(|y| (0..40).map(move |x| (x, y)))
        .map(|(x, y)| view.map.get(x, y).map(|id| id.0))
        .collect();
    let result = (cells, camera.center(), camera.zoom());
    manager.shutdown()?;
    Ok(result)
}
pub fn smoke() -> Result<(), Box<dyn std::error::Error>> {
    for factory in [high as fn(Context) -> Result<SceneManager, String>, low] {
        scenes::smoke_with(factory)?;
    }
    for hz in [30, 60] {
        for coupled in [false, true] {
            if coupled && !cfg!(feature = "bridge") {
                continue;
            }
            assert_eq!(state(high, coupled, hz)?, state(low, coupled, hz)?);
        }
    }
    // The worker API builds the App in the chosen owner thread, not on the host.
    let config = Startup::default();
    let mut worker = config.spawn(
        Lane::Compute,
        "m6-api-compute",
        |app: AppBuilder| Ok(app.resource(7_u32)),
        (),
    )?;
    worker.stop();
    worker.join()?;
    println!(
        "M6 HIGH/LOW: identical worlds/cameras at game144/compute30..60, restart/edit/cleanup, worker builder OK"
    );
    Ok(())
}
