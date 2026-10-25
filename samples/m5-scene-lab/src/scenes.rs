use crate::controller::{self, Action, EditSink, Flags};
use charon_assets::{Assets, AssetsPlugin, Handle, Image, ShaderSource};
use charon_camera::{Camera2D, CameraPlugin};
use charon_core::{App, World};
use charon_input::{InputClient, InputPlugin, InputState, Key, keys};
use charon_m3_sample::runtime::Runtime;
use charon_render::{DrawFrame, PassId, Quad, TextureId, gpu::GpuRenderer};
use charon_scene::{
    Directive, SceneId, SceneManager, Step,
    integration::{SceneLogic, ScopedScene},
};
use charon_tilemap::{Atlas, Sprite, TileId};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver},
    },
    time::Duration,
};
pub type Graphics = Rc<RefCell<GpuRenderer>>;
pub struct Frontend(pub InputClient);
#[derive(Clone, Copy)]
pub struct Ids {
    pub atlas: TextureId,
    pub sand: TextureId,
    pub light: TextureId,
    pub plain: PassId,
    pub lit: PassId,
}
#[derive(Clone)]
pub struct Context {
    pub graphics: Option<Graphics>,
    pub size: Rc<Cell<[u32; 2]>>,
    pub hz: u32,
    pub prefer_sand: bool,
}
pub struct Session {
    pub id: SceneId,
    pub name: String,
    pub coupled: bool,
    pub runtime: Option<Runtime>,
    pub camera: Arc<Mutex<Camera2D>>,
    pub flags: Arc<Mutex<Flags>>,
    pub selected: Arc<Mutex<TileId>>,
    pub input: InputClient,
    actions: Receiver<(SceneId, Action)>,
    pub ids: Ids,
    pub image: Option<Handle<Image>>,
    #[cfg(feature = "lighting")]
    pub light: Option<charon_m4_sample::illumination::Illumination>,
}
struct Menu {
    prefer_sand: bool,
}
impl SceneLogic for Menu {
    fn fixed_update(&mut self, w: &mut World, _: Step) -> Result<Directive, String> {
        let input = w.get::<InputState>().ok_or("menu input missing")?;
        if !input.focused {
            return Ok(Directive::Continue);
        }
        if input.pressed(keys::ESCAPE) {
            return Ok(Directive::Stop);
        }
        if input.pressed(keys::ENTER) {
            return Ok(Directive::Switch(
                if self.prefer_sand {
                    "sand-game"
                } else {
                    "tile-game"
                }
                .into(),
            ));
        }
        if input.pressed(Key('T' as u32)) {
            return Ok(Directive::Switch("tile-game".into()));
        }
        Ok(Directive::Continue)
    }
}
struct Cave {
    ctx: Context,
    coupled: bool,
    name: String,
}
impl Cave {
    fn cleanup(&self, w: &mut World) -> Result<(), String> {
        let mut errors = Vec::new();
        if let Some(session) = w.get_mut::<Session>() {
            if let Some(mut runtime) = session.runtime.take()
                && let Err(e) = runtime.shutdown()
            {
                errors.push(e.to_string());
            }
            #[cfg(feature = "lighting")]
            session.light.take();
            if let Some(graphics) = &self.ctx.graphics {
                let mut r = graphics.borrow_mut();
                r.remove_pass(session.ids.lit);
                r.remove_pass(session.ids.plain);
                for id in [session.ids.atlas, session.ids.sand, session.ids.light] {
                    r.remove_texture(id);
                }
            }
        }
        w.remove::<Frontend>();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}
impl SceneLogic for Cave {
    fn start(&mut self, id: SceneId, w: &mut World) -> Result<(), String> {
        let ids = if let Some(g) = &self.ctx.graphics {
            let mut r = g.borrow_mut();
            Ids {
                atlas: r.allocate_texture_id()?,
                sand: r.allocate_texture_id()?,
                light: r.allocate_texture_id()?,
                plain: r.allocate_pass_id()?,
                lit: r.allocate_pass_id()?,
            }
        } else {
            Ids {
                atlas: TextureId(1),
                sand: TextureId(2),
                light: TextureId(3),
                plain: PassId(1),
                lit: PassId(2),
            }
        };
        let (input_plugin, input) = InputPlugin::new(256)?;
        let initial = controller::camera(self.ctx.size.get());
        let mirror = Arc::new(Mutex::new(initial));
        let flags = Arc::new(Mutex::new(Flags::default()));
        let selected = Arc::new(Mutex::new(TileId(0)));
        let sink: EditSink = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::sync_channel(16);
        w.insert(Session {
            id,
            name: self.name.clone(),
            coupled: self.coupled,
            runtime: None,
            camera: mirror.clone(),
            flags: flags.clone(),
            selected: selected.clone(),
            input: input.clone(),
            actions: rx,
            ids,
            image: None,
            #[cfg(feature = "lighting")]
            light: None,
        });
        let handle = w
            .get_mut::<Assets<Image>>()
            .ok_or("scene assets missing")?
            .load(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/atlas.png"),
                Image::decode,
            )?;
        w.get_mut::<Session>().unwrap().image = Some(handle);
        if let Some(g) = &self.ctx.graphics {
            let image = w.get::<Assets<Image>>().unwrap().get(handle).unwrap();
            let shader = ShaderSource(charon_render::DEFAULT_SHADER.into());
            w.get_mut::<Assets<ShaderSource>>()
                .unwrap()
                .insert(shader.clone());
            let mut r = g.borrow_mut();
            r.upload(ids.atlas, &image)?;
            pollster::block_on(r.register_pass(ids.plain, &shader, [1., 0., 0., 0.]))?;
            #[cfg(feature = "lighting")]
            {
                w.get_mut::<Session>().unwrap().light =
                    Some(charon_m4_sample::illumination::Illumination::new_with_ids(
                        &mut r, 4., ids.light, ids.lit,
                    )?);
            }
        }
        let coupled = self.coupled;
        let sink_for_worker = sink.clone();
        let runtime = Runtime::start_with_game_plugins(
            self.ctx.hz,
            coupled,
            charon_m4_sample::world::map(),
            move |shared| {
                let controller = controller::Controller::new(
                    shared,
                    controller::ControllerInit {
                        id,
                        coupled,
                        mirror,
                        flags,
                        selected,
                        edits: sink_for_worker,
                        actions: tx,
                    },
                );
                vec![
                    Box::new(input_plugin),
                    Box::new(CameraPlugin::new(initial)),
                    Box::new(controller),
                ]
            },
        )
        .map_err(|e| e.to_string())?;
        *sink.lock().unwrap() = Some(runtime.edits.clone());
        w.get_mut::<Session>().unwrap().runtime = Some(runtime);
        w.insert(Frontend(input));
        Ok(())
    }
    fn fixed_update(&mut self, w: &mut World, _: Step) -> Result<Directive, String> {
        let session = w.get_mut::<Session>().ok_or("session missing")?;
        session.runtime.as_ref().ok_or("runtime missing")?.check()?;
        while let Ok((from, action)) = session.actions.try_recv() {
            if from == session.id {
                return Ok(Directive::Switch(match action {
                    Action::Menu => "menu".into(),
                    Action::Restart => session.name.clone(),
                }));
            }
        }
        Ok(Directive::Continue)
    }
    fn stop(&mut self, w: &mut World) -> Result<(), String> {
        self.cleanup(w)
    }
    fn destroy(&mut self, w: &mut World) -> Result<(), String> {
        w.remove::<Session>();
        w.remove::<Frontend>();
        Ok(())
    }
}
pub fn manager(ctx: Context) -> Result<SceneManager, String> {
    let mut manager = SceneManager::new();
    let menu_ctx = ctx.clone();
    manager.register("menu", move |_| {
        let mut app = App::new();
        let (plugin, client) = InputPlugin::new(256)?;
        app.add_plugin(plugin).map_err(|e| e.to_string())?;
        app.add_plugin(AssetsPlugin::default())
            .map_err(|e| e.to_string())?;
        app.world_mut().insert(Frontend(client));
        Ok(Box::new(ScopedScene::new(
            app,
            Menu {
                prefer_sand: menu_ctx.prefer_sand,
            },
        )))
    })?;
    for (name, coupled) in [("sand-game", true), ("tile-game", false)] {
        let ctx = ctx.clone();
        manager.register(name, move |_| {
            let mut app = App::new();
            app.add_plugin(AssetsPlugin::default())
                .map_err(|e| e.to_string())?;
            Ok(Box::new(ScopedScene::new(
                app,
                Cave {
                    ctx: ctx.clone(),
                    coupled,
                    name: name.into(),
                },
            )))
        })?;
    }
    manager.switch_to("menu")?;
    Ok(manager)
}
pub fn world(manager: &SceneManager) -> Option<&World> {
    manager.active_as::<ScopedScene>().map(ScopedScene::world)
}
pub fn world_mut(manager: &mut SceneManager) -> Option<&mut World> {
    manager
        .active_as_mut::<ScopedScene>()
        .map(ScopedScene::world_mut)
}
pub fn frontend(manager: &SceneManager) -> Option<InputClient> {
    world(manager)?.get::<Frontend>().map(|v| v.0.clone())
}
pub fn session(manager: &SceneManager) -> Option<&Session> {
    world(manager)?.get::<Session>()
}
pub fn draw(
    manager: &mut SceneManager,
    r: &mut GpuRenderer,
    size: [u32; 2],
) -> Result<DrawFrame, String> {
    #[cfg(not(any(feature = "bridge", feature = "lighting")))]
    let _ = r;
    let mut frame = DrawFrame::new(size);
    let w = world_mut(manager).ok_or("scene world missing")?;
    let Some(s) = w.get_mut::<Session>() else {
        crate::ui::text(
            &mut frame,
            110.,
            140.,
            "CHARON / M5 SCENE LAB",
            [0.8, 0.95, 1., 1.],
        );
        crate::ui::text(
            &mut frame,
            110.,
            200.,
            "ENTER START / T TILEMAP ONLY",
            [1., 0.8, 0.3, 1.],
        );
        crate::ui::text(
            &mut frame,
            110.,
            244.,
            "ARROWS PAN / Q E OR WHEEL ZOOM",
            [0.7, 0.8, 1., 1.],
        );
        crate::ui::text(
            &mut frame,
            110.,
            288.,
            "TAB RETURN / R RESTART / ESC EXIT",
            [0.7, 0.8, 1., 1.],
        );
        return Ok(frame);
    };
    let runtime = s.runtime.as_ref().ok_or("runtime stopped")?;
    let view = runtime.shared.view.lock().unwrap().clone();
    let player = *runtime.shared.player.lock().unwrap();
    let mut camera = *s.camera.lock().unwrap();
    camera.set_viewport(controller::viewport(size))?;
    frame.quads.push(Quad {
        rect: [0., 0., 320., 192.],
        uv: [0., 0., 1., 1.],
        tint: [0.12, 0.17, 0.22, 1.],
        texture: TextureId(0),
        layer: -1,
        pass: s.ids.plain,
    });
    let atlas = Atlas {
        texture: s.ids.atlas.0,
        size: [32, 8],
        tile_size: [8, 8],
    };
    charon_tilemap_render::append_tiles(
        &mut frame,
        &view.map,
        &atlas,
        [0., 0.],
        1.,
        0,
        s.ids.plain,
    )?;
    #[cfg(feature = "bridge")]
    if let Some(snapshot) = &view.sand {
        r.upload(s.ids.sand, &charon_sand_render::image(snapshot)?)?;
        charon_sand_render::append(&mut frame, s.ids.sand, [0., 0., 320., 192.], 1, s.ids.plain);
    }
    for position in charon_m3_sample::runtime::GEMS
        .iter()
        .skip(player.score as usize)
    {
        charon_tilemap_render::append_sprite(
            &mut frame,
            &Sprite {
                position: *position,
                size: [6., 6.],
                atlas_index: 3,
                layer: 2,
                tint: [1.; 4],
            },
            &atlas,
            s.ids.plain,
        )?;
    }
    charon_tilemap_render::append_sprite(
        &mut frame,
        &Sprite {
            position: player.position,
            size: [6., 6.],
            atlas_index: 3,
            layer: 3,
            tint: [0.4, 1., 0.9, 1.],
        },
        &atlas,
        s.ids.plain,
    )?;
    #[cfg(feature = "lighting")]
    if let Some(light) = &mut s.light {
        let flags = s.flags.lock().unwrap().clone();
        if flags.quality != (light.cell_size == 2.) {
            light.quality(r)?;
        }
        light.enabled = flags.enabled;
        light.torch = flags.torch;
        light.ore = flags.ore;
        light.ao = flags.ao;
        light.sky = flags.sky;
        light.update(r, &view, player, size, 30)?;
        let domain = camera.project_rect(charon_camera::Rect {
            x: 0.,
            y: 0.,
            width: 320.,
            height: 192.,
        })?;
        r.set_parameters(s.ids.lit, domain.array())?;
        if light.enabled {
            for q in &mut frame.quads {
                q.pass = s.ids.lit;
            }
        }
    }
    charon_camera_render::apply(&mut frame, camera, &[s.ids.plain, s.ids.lit])?;
    let color = [0.85, 0.94, 1., 1.];
    crate::ui::text(
        &mut frame,
        16.,
        12.,
        "CHARON / M5 INPUT CAMERA SCENE",
        color,
    );
    crate::ui::text(
        &mut frame,
        16.,
        36.,
        &format!(
            "{} / GEMS {} / TILE {} / CAM {:.0} {:.0} / ZOOM {:.2}",
            if s.coupled { "SAND" } else { "TILEMAP" },
            player.score,
            s.selected.lock().unwrap().0 + 1,
            camera.center()[0],
            camera.center()[1],
            camera.zoom()
        ),
        [0.4, 1., 0.8, 1.],
    );
    crate::ui::text(
        &mut frame,
        16.,
        56.,
        "WASD MOVE / ARROWS PAN / Q E WHEEL ZOOM / SPACE FIT",
        color,
    );
    let y = size[1].saturating_sub(40) as f32;
    crate::ui::text(
        &mut frame,
        16.,
        y,
        "CLICK DIG BUILD / TAB MENU / R RESTART / B RELEASE / G DOOR",
        color,
    );
    crate::ui::text(&mut frame, 16., y + 20., &view.message, [0.4, 1., 0.8, 1.]);
    Ok(frame)
}
pub fn send(manager: &SceneManager, event: charon_input::InputEvent) -> Result<(), String> {
    frontend(manager)
        .ok_or("no frontend")?
        .try_send(event)
        .map_err(|e| e.to_string())
}
pub fn pulse(manager: &SceneManager, key: charon_input::Key) -> Result<(), String> {
    for pressed in [true, false] {
        send(
            manager,
            charon_input::InputEvent::Button {
                button: key.into(),
                pressed,
            },
        )?;
    }
    Ok(())
}
pub fn wait(
    manager: &mut SceneManager,
    mut condition: impl FnMut(&SceneManager) -> bool,
) -> Result<(), String> {
    let end = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        manager.advance(Duration::from_millis(7))?;
        if condition(manager) {
            return Ok(());
        }
        if std::time::Instant::now() > end {
            return Err("scene operation timed out".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
pub fn smoke() -> Result<(), Box<dyn std::error::Error>> {
    for hz in [30, 60] {
        for coupled in [false, true] {
            if coupled && !cfg!(feature = "bridge") {
                continue;
            }
            let ctx = Context {
                graphics: None,
                size: Rc::new(Cell::new([960, 696])),
                hz,
                prefer_sand: coupled,
            };
            let mut m = manager(ctx)?;
            pulse(&m, keys::ENTER)?;
            wait(&mut m, |m| session(m).is_some())?;
            let s = session(&m).unwrap();
            let old = s.id;
            let old_input = s.input.clone();
            let old_view = s.runtime.as_ref().unwrap().shared.view.clone();
            let old_handle = s.image.unwrap();
            let image = world(&m)
                .unwrap()
                .get::<Assets<Image>>()
                .unwrap()
                .get(old_handle)
                .unwrap();
            let weak = Arc::downgrade(&image);
            drop(image);
            let cam = *s.camera.lock().unwrap();
            let pointer = cam.world_to_screen([108., 84.])?;
            send(&m, charon_input::InputEvent::Pointer(pointer))?;
            for pressed in [true, false] {
                send(
                    &m,
                    charon_input::InputEvent::Button {
                        button: charon_input::Button::Mouse(0),
                        pressed,
                    },
                )?;
            }
            wait(&mut m, |m| {
                session(m)
                    .unwrap()
                    .runtime
                    .as_ref()
                    .unwrap()
                    .shared
                    .view
                    .lock()
                    .unwrap()
                    .map
                    .get(13, 10)
                    .is_none()
            })?;
            pulse(&m, Key('T' as u32))?;
            wait(&mut m, |m| !session(m).unwrap().flags.lock().unwrap().torch)?;
            pulse(&m, Key('R' as u32))?;
            wait(&mut m, |m| m.active_id() != Some(old))?;
            assert!(weak.upgrade().is_none());
            assert_eq!(
                *session(&m).unwrap().flags.lock().unwrap(),
                Flags::default()
            );
            assert!(
                old_input
                    .try_send(charon_input::InputEvent::Focus(true))
                    .is_err()
            );
            assert!(
                world(&m)
                    .unwrap()
                    .get::<Assets<Image>>()
                    .unwrap()
                    .get(old_handle)
                    .is_none()
            );
            assert!(
                session(&m)
                    .unwrap()
                    .runtime
                    .as_ref()
                    .unwrap()
                    .shared
                    .view
                    .lock()
                    .unwrap()
                    .map
                    .get(13, 10)
                    .is_some()
            );
            assert_eq!(
                session(&m).unwrap().camera.lock().unwrap().center(),
                [160., 96.]
            );
            let tick = old_view.lock().unwrap().ticks;
            std::thread::sleep(Duration::from_millis(20));
            assert_eq!(old_view.lock().unwrap().ticks, tick);
            pulse(&m, keys::TAB)?;
            wait(&mut m, |m| m.active_name() == Some("menu"))?;
            assert!(session(&m).is_none());
            assert!(m.request_switch(old, "tile-game").is_err());
            m.shutdown()?;
            println!(
                "M5 game144/compute{hz}, sand={coupled}: input pick, scene restart, asset release, worker shutdown and stale IDs OK"
            );
        }
    }
    Ok(())
}
