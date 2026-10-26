use super::*;
use crate::config::Capabilities;
use charon_input::{Button, InputEvent, Key, keys};
use charon_m5_sample::ui;
use charon_render::{DrawFrame, PassId, Quad, TextureId};
use charon_tracing::{Mode, Quality};
use charon_tracing_gpu::{LightingDevice, SwitchRequest, TracingSlot};
const DEMO_UI_PASS: PassId = PassId(u64::MAX);
const OPTIONS: [Selection; 4] = [
    Selection::Rtx,
    Selection::Path,
    Selection::LitePath,
    Selection::M5,
];
struct Settings {
    prefs: Preferences,
    capabilities: Capabilities,
    active: Selection,
    pending: Option<Selection>,
    open: bool,
    pointer: Option<[f32; 2]>,
    focused: bool,
    status: String,
    checked: bool,
    device: Option<LightingDevice>,
}
fn label(choice: Selection) -> &'static str {
    match choice {
        Selection::Rtx => "RTX HARDWARE",
        Selection::Path => "PATH TRACING",
        Selection::LitePath => "LITE PATH",
        Selection::M5 => "M5 LIGHTING",
    }
}
fn row_at(p: [f32; 2]) -> Option<usize> {
    if p[0] < 48. || p[0] > 850. {
        return None;
    }
    let row = ((p[1] - 176.) / 52.).floor();
    (0. ..4.).contains(&row).then_some(row as usize)
}
pub fn input(manager: &mut SceneManager, event: InputEvent) -> Result<bool, String> {
    let Some(world) = scenes::world_mut(manager) else {
        return Ok(false);
    };
    let Some(mut state) = world.remove::<Settings>() else {
        return Ok(false);
    };
    match event {
        InputEvent::Pointer(p) => state.pointer = Some(p),
        InputEvent::PointerLeft => state.pointer = None,
        InputEvent::Focus(f) => state.focused = f,
        _ => {}
    }
    let toggle = (event
        == InputEvent::Button {
            button: keys::TAB.into(),
            pressed: true,
        }
        && state.open)
        || event
            == InputEvent::Button {
                button: Button::Key(Key('C' as u32)),
                pressed: true,
            };
    let mut consumed = false;
    if toggle {
        state.open = !state.open;
        println!(
            "LIGHTING UI: {}",
            if state.open { "open" } else { "closed" }
        );
        consumed = true;
        if let Some(client) = world.get::<scenes::Frontend>() {
            client
                .0
                .try_send(InputEvent::Focus(if state.open {
                    false
                } else {
                    state.focused
                }))
                .map_err(|e| e.to_string())?;
        }
    } else if state.open {
        consumed = matches!(
            event,
            InputEvent::Button { .. } | InputEvent::Repeat(_) | InputEvent::Wheel(_)
        );
        let mut selected = None;
        if let InputEvent::Button {
            button,
            pressed: true,
        } = event
        {
            match button {
                Button::Key(Key(code)) if (b'1' as u32..=b'4' as u32).contains(&code) => {
                    selected = Some((code - b'1' as u32) as usize)
                }
                Button::Mouse(0) => selected = state.pointer.and_then(row_at),
                Button::Key(Key(code)) if code == 'S' as u32 => {
                    if let Some(path) = CONFIG_PATH.get().and_then(|p| p.as_ref()) {
                        let result = state
                            .prefs
                            .json()
                            .and_then(|text| std::fs::write(path, text).map_err(|e| e.to_string()));
                        state.status = match result {
                            Ok(()) => "CONFIG SAVED".into(),
                            Err(e) => format!("SAVE FAILED: {e}"),
                        };
                    } else {
                        state.status = "USE --CONFIG TO ENABLE SAVE".into();
                    }
                }
                _ => {}
            }
        }
        if let Some(row) = selected {
            let choice = OPTIONS[row];
            if choice == Selection::Rtx && state.capabilities.restart {
                state.prefs.pending = Some(choice);
                state.prefs.enable_rtx_device = true;
                state.status = "RESTART REQUIRED / FALSE / S SAVE".into();
                if let Some(shared) = PREFERENCES.get() {
                    *shared.lock().unwrap() = state.prefs;
                }
            } else if let Some(reason) = state.capabilities.unavailable(choice) {
                state.status = format!("FALSE / {reason}");
            } else {
                state.pending = Some(choice);
            }
        }
    }
    world.insert(state);
    Ok(consumed)
}
pub fn frame(
    manager: &mut SceneManager,
    renderer: &mut GpuRenderer,
    frame: &mut DrawFrame,
) -> Result<(), String> {
    let world = scenes::world_mut(manager).ok_or("lighting scene missing")?;
    if world.get::<Settings>().is_none() {
        // Passes execute in registration order. Reappend the demo overlay after
        // the newly created world's composition pass, including scene restarts.
        renderer.remove_pass(DEMO_UI_PASS);
        pollster::block_on(renderer.register_pass(
            DEMO_UI_PASS,
            &charon_assets::ShaderSource(charon_render::DEFAULT_SHADER.into()),
            [1., 0., 0., 0.],
        ))?;
    }
    let mut state = world.remove::<Settings>().unwrap_or_else(|| {
        let prefs = preferences();
        let device = LightingDevice::new(renderer.device().clone(), renderer.queue().clone())
            .with_adapter(renderer.adapter_features, &renderer.adapter_info);
        let probe_error = pollster::block_on(device.probe_rtx()).err();
        let capabilities = Capabilities::from_info(device.info(), prefs.platform());
        println!("LIGHTING DEVICE: {:?}", device.info());
        let unavailable = capabilities.unavailable(prefs.requested());
        Settings {
            prefs,
            capabilities,
            active: Selection::M5,
            pending: unavailable.is_none().then_some(prefs.requested()),
            open: false,
            pointer: None,
            focused: true,
            status: probe_error.unwrap_or_else(|| unavailable.unwrap_or("").to_string()),
            checked: false,
            device: Some(device),
        }
    });
    let mut slot = world
        .remove::<TracingSlot>()
        .ok_or("tracing plugin not installed")?;
    let result = draw(&mut state, &mut slot, world, renderer, frame);
    world.insert(slot);
    world.insert(state);
    result
}
fn draw(
    state: &mut Settings,
    slot: &mut TracingSlot,
    world: &mut charon_core::World,
    renderer: &mut GpuRenderer,
    frame: &mut DrawFrame,
) -> Result<(), String> {
    if let Some(session) = world.get_mut::<scenes::Session>()
        && let Some(light) = session.light.as_mut()
    {
        let selection = state.pending.unwrap_or(state.active);
        if let Some(reason) = state.capabilities.unavailable(selection) {
            state.status = reason.into();
            state.pending = None;
        } else {
            let changed = selection != state.active;
            if selection == Selection::M5 {
                if changed || state.pending.is_some() {
                    light.resume_inputs();
                    let shared = &session
                        .runtime
                        .as_ref()
                        .ok_or("world runtime missing")?
                        .shared;
                    let view = shared.view.lock().unwrap().clone();
                    let player = *shared.player.lock().unwrap();
                    light.update(renderer, &view, player, frame.size, 30)?;
                    let camera = *session.camera.lock().unwrap();
                    let domain = camera.project_rect(charon_camera::Rect {
                        x: 0.,
                        y: 0.,
                        width: 320.,
                        height: 192.,
                    })?;
                    renderer.set_parameters(session.ids.lit, domain.array())?;
                    let restored = light
                        .restore_legacy(renderer)
                        .and_then(|()| light.readback())
                        .and_then(|field| {
                            if field.rgba.iter().flatten().any(|v| !v.is_finite()) {
                                Err("M5 first dispatch was not finite".into())
                            } else {
                                Ok(())
                            }
                        });
                    match restored {
                        Ok(()) => {
                            slot.switch(None);
                            state.active = selection;
                            state.pending = None;
                            state.prefs.lighting = selection;
                            state.prefs.pending = None;
                            state.status = "APPLIED / TRUE / M5".into();
                            println!("LIGHTING SWITCH: M5");
                        }
                        Err(error) => {
                            state.pending = None;
                            state.status = format!("M5 CHANGE FAILED: {error}");
                            if let Some(texture) = slot.texture() {
                                renderer.bind_texture(session.ids.light, texture, true)?;
                            }
                        }
                    }
                }
            } else {
                let mode = selection.mode();
                let cell_size = if mode == Mode::LitePath {
                    8.
                } else {
                    light.cell_size
                };
                let geometry = charon_tracing::charon_lighting::Geometry {
                    width: (320. / cell_size) as u32,
                    height: (192. / cell_size) as u32,
                    origin: [0., 0.],
                    cell_size,
                };
                let recreate = changed || slot.geometry() != Some(geometry);
                if recreate {
                    let shared = session
                        .runtime
                        .as_ref()
                        .ok_or("world runtime missing")?
                        .shared
                        .clone();
                    let flags = session.flags.clone();
                    let pool = CPU_POOL.get().unwrap().clone();
                    let view = shared.view.lock().unwrap().clone();
                    let player = *shared.player.lock().unwrap();
                    let values = flags.lock().unwrap().clone();
                    let scene =
                        crate::lighting_input::build(view, player, values, geometry, &pool)?;
                    let device = state.device.as_ref().ok_or("lighting device missing")?;
                    let mut prepared = TracingSlot::default();
                    let request = SwitchRequest {
                        mode,
                        platform: state.prefs.platform(),
                        quality: Quality::for_mode(mode),
                        hz: state.prefs.update_hz,
                    };
                    let candidate = pollster::block_on(prepared.try_switch(device, request, scene))
                        .and_then(|applied| {
                            if !applied {
                                return Err("UNSUPPORTED: FALSE".into());
                            }
                            let mut runtime =
                                prepared.active.take().ok_or("tracing candidate missing")?;
                            runtime.tracer.set_cpu_pool(pool.clone());
                            let source =
                                crate::lighting_input::source(shared, flags, geometry, pool);
                            let worker = charon_tracing_gpu::TracingWorker::spawn(
                                runtime,
                                state.prefs.update_hz,
                                source,
                            )?;
                            renderer.bind_texture(
                                session.ids.light,
                                worker.texture().clone(),
                                true,
                            )?;
                            Ok(worker)
                        });
                    match candidate {
                        Ok(worker) => {
                            slot.commit_worker(worker);
                            state.active = selection;
                            state.prefs.lighting = selection;
                            state.prefs.pending = None;
                            state.pending = None;
                            state.status = "APPLIED / TRUE".into();
                            println!(
                                "LIGHTING SWITCH: {:?} on dedicated {}Hz worker",
                                state.active, state.prefs.update_hz
                            );
                        }
                        Err(error) => {
                            state.status = format!("CHANGE FAILED: {error}");
                            state.pending = None;
                        }
                    }
                }
                if let Some(worker) = slot.threaded.as_ref() {
                    worker.check()?;
                    worker.set_enabled(light.enabled);
                }
                if let Some(texture) = slot.texture() {
                    renderer.bind_texture(session.ids.light, texture, true)?;
                }
            }
            if let Some(shared) = PREFERENCES.get() {
                *shared.lock().unwrap() = state.prefs;
            }
        }
        let check = std::env::args().any(|a| a == "--gpu-check");
        light.compute_enabled = state.active == Selection::M5 || check;
        light.defer_inputs = state.active != Selection::M5 && !check;
        if check && state.active != state.prefs.requested() {
            return Err("requested lighting returned false / not applied; this is not an RTX validation success".into());
        }
        if check && !state.checked {
            if slot.texture().is_some() {
                let field = slot.readback()?;
                if field.rgba.iter().flatten().any(|v| !v.is_finite()) {
                    return Err("non-finite trace sample".into());
                }
                println!(
                    "RT sample: {:?}, samples={}, Tilemap/Sand common optics, field finite",
                    state.active,
                    slot.samples()
                );
            }
            state.checked = true;
        }
    }
    if world.get::<scenes::Session>().is_none()
        && let Some(selection) = state.pending.take()
    {
        state.capabilities.require(selection)?;
        state.prefs.pending = Some(selection);
        if let Some(shared) = PREFERENCES.get() {
            *shared.lock().unwrap() = state.prefs;
        }
        state.status = "APPLIES WHEN GAME STARTS".into();
    }
    let ui_start = frame.quads.len();
    ui::text(
        frame,
        500.,
        12.,
        &format!("{} / C SETTINGS", label(state.active)),
        [1., 0.8, 0.3, 1.],
    );
    if slot.texture().is_some() {
        let budget = *CPU_BUDGET.get().unwrap();
        ui::text(
            frame,
            500.,
            frame.size[1].saturating_sub(20) as f32,
            &format!("SAMPLES {} / CPU {}", slot.samples(), budget.logical_cpus),
            [1., 0.8, 0.3, 1.],
        );
    }
    if state.open {
        let overlay_start = frame.quads.len();
        frame.quads.push(Quad {
            rect: [32., 96., frame.size[0].saturating_sub(64) as f32, 480.],
            uv: [0., 0., 1., 1.],
            tint: [0.025, 0.035, 0.06, 0.98],
            texture: TextureId(0),
            pass: PassId(0),
            layer: 109,
        });
        // UI text at layer 100 follows the modal background; world input is suppressed.
        ui::text(
            frame,
            48.,
            120.,
            "LIGHTING / 1-4 OR CLICK / C CLOSE",
            [1., 1., 1., 1.],
        );
        for (row, choice) in OPTIONS.iter().enumerate() {
            let reason = state.capabilities.unavailable(*choice);
            let color = if reason.is_some() {
                [0.35, 0.35, 0.4, 1.]
            } else if *choice == state.active {
                [0.4, 1., 0.7, 1.]
            } else {
                [0.9, 0.9, 1., 1.]
            };
            ui::text(
                frame,
                48.,
                180. + row as f32 * 52.,
                &format!("{} {}", row + 1, label(*choice)),
                color,
            );
            if let Some(reason) = reason {
                ui::text(
                    frame,
                    400.,
                    180. + row as f32 * 52.,
                    reason,
                    [0.4, 0.4, 0.45, 1.],
                );
            }
        }
        ui::text(
            frame,
            48.,
            432.,
            "S SAVE CONFIG / CHANGES APPLY ON SUCCESS",
            [0.8, 0.85, 1., 1.],
        );
        let message: String = state.status.chars().take(68).collect();
        ui::text(frame, 48., 472., &message, [1., 0.8, 0.3, 1.]);
        if let Some(requested) = state.prefs.pending {
            ui::text(
                frame,
                48.,
                516.,
                &format!("PENDING {} / NOT APPLIED", label(requested)),
                [1., 0.7, 0.3, 1.],
            );
        }
        for q in &mut frame.quads[overlay_start + 1..] {
            q.layer = 110;
        }
    }
    for quad in &mut frame.quads[ui_start..] {
        quad.pass = DEMO_UI_PASS;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_scene::integration::{SceneLogic, ScopedScene};
    struct Empty;
    impl SceneLogic for Empty {}
    fn manager(platform: charon_tracing::Platform) -> SceneManager {
        let mut manager = SceneManager::new();
        manager
            .register("settings", move |_| {
                let mut app = App::new();
                let (plugin, client) = InputPlugin::new(16)?;
                app.add_plugin(plugin).map_err(|e| e.to_string())?;
                app.world_mut().insert(scenes::Frontend(client));
                app.world_mut().insert(Settings {
                    prefs: Preferences::default(),
                    capabilities: Capabilities {
                        platform,
                        compute: true,
                        rtx: false,
                        restart: false,
                        probe_failed: false,
                    },
                    active: Selection::M5,
                    pending: None,
                    open: false,
                    pointer: None,
                    focused: true,
                    status: String::new(),
                    checked: false,
                    device: None,
                });
                Ok(Box::new(ScopedScene::new(app, Empty)))
            })
            .unwrap();
        manager.switch_to("settings").unwrap();
        manager
    }
    fn press(key: char) -> InputEvent {
        InputEvent::Button {
            button: Key(key as u32).into(),
            pressed: true,
        }
    }
    #[test]
    fn unavailable_rtx_is_not_selected_and_modal_input_is_consumed() {
        let mut m = manager(charon_tracing::Platform::Pc);
        assert!(input(&mut m, press('C')).unwrap());
        assert!(input(&mut m, press('1')).unwrap());
        let state = scenes::world(&m).unwrap().get::<Settings>().unwrap();
        assert!(state.open);
        assert!(state.pending.is_none());
        assert_eq!(state.active, Selection::M5);
        assert!(input(&mut m, press('2')).unwrap());
        assert_eq!(
            scenes::world(&m)
                .unwrap()
                .get::<Settings>()
                .unwrap()
                .pending,
            Some(Selection::Path)
        );
        assert!(input(&mut m, press('C')).unwrap());
        assert!(!input(&mut m, press('W')).unwrap());
        m.shutdown().unwrap();
    }
    #[test]
    fn restart_required_request_is_pending_and_never_marked_applied() {
        let mut m = manager(charon_tracing::Platform::Pc);
        scenes::world_mut(&mut m)
            .unwrap()
            .get_mut::<Settings>()
            .unwrap()
            .capabilities
            .restart = true;
        input(&mut m, press('C')).unwrap();
        input(&mut m, press('1')).unwrap();
        let state = scenes::world(&m).unwrap().get::<Settings>().unwrap();
        assert_eq!(state.active, Selection::M5);
        assert!(state.pending.is_none());
        assert_eq!(state.prefs.pending, Some(Selection::Rtx));
        assert!(state.status.contains("FALSE"));
        m.shutdown().unwrap();
    }
    #[test]
    fn mobile_disables_full_path_and_clears_held_input_on_open() {
        let mut m = manager(charon_tracing::Platform::Mobile);
        let client = scenes::frontend(&m).unwrap();
        client.try_send(press('W')).unwrap();
        m.advance(std::time::Duration::from_millis(7)).unwrap();
        assert!(client.snapshot().held(Key('W' as u32)));
        input(&mut m, press('C')).unwrap();
        m.advance(std::time::Duration::from_millis(7)).unwrap();
        assert!(!client.snapshot().held(Key('W' as u32)));
        input(&mut m, press('2')).unwrap();
        assert!(
            scenes::world(&m)
                .unwrap()
                .get::<Settings>()
                .unwrap()
                .pending
                .is_none()
        );
        input(&mut m, press('3')).unwrap();
        assert_eq!(
            scenes::world(&m)
                .unwrap()
                .get::<Settings>()
                .unwrap()
                .pending,
            Some(Selection::LitePath)
        );
        m.shutdown().unwrap();
    }
}
