use super::*;
use crate::config::Capabilities;
use charon_input::{Button, InputEvent, Key, keys};
use charon_m5_sample::ui;
use charon_render::{DrawFrame, PassId, Quad, TextureId};
use charon_tracing::{Mode, Quality, TraceScene};
use charon_tracing_gpu::{LightingDevice, SwitchRequest, TracingSlot};
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
    last_scene_update: std::time::Instant,
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
            last_scene_update: std::time::Instant::now() - std::time::Duration::from_secs(1),
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
    let requested = state.pending.unwrap_or(state.active);
    let ready = match slot.active.as_ref() {
        Some(r) => r.tracer.is_idle()?,
        None => true,
    };
    let refresh = state.pending.is_some()
        || (ready
            && state.last_scene_update.elapsed()
                >= std::time::Duration::from_secs_f64(1. / state.prefs.update_hz as f64));
    if refresh {
        state.last_scene_update = std::time::Instant::now();
    }
    let enhanced_scene = if refresh && matches!(requested, Selection::Rtx | Selection::Path) {
        world
            .get::<scenes::Session>()
            .filter(|s| s.light.is_some() && s.runtime.is_some())
            .map(|s| crate::materials::scene(s, requested.mode()))
            .transpose()?
    } else {
        None
    };
    if refresh
        && let Some(session) = world.get_mut::<scenes::Session>()
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
                        }
                        Err(error) => {
                            state.pending = None;
                            state.status = format!("M5 CHANGE FAILED: {error}");
                            if let Some(runtime) = slot.active.as_ref() {
                                renderer.bind_texture(
                                    session.ids.light,
                                    runtime.tracer.texture().clone(),
                                    true,
                                )?;
                            }
                        }
                    }
                }
            } else {
                let mode = selection.mode();
                let scene = if let Some(scene) = enhanced_scene {
                    scene
                } else {
                    TraceScene::at_resolution(
                        &light.scene().prepare(),
                        if mode == Mode::LitePath {
                            8.
                        } else {
                            light.cell_size
                        },
                    )?
                };
                let recreate = changed
                    || slot
                        .active
                        .as_ref()
                        .is_none_or(|r| r.tracer.readback_geometry() != scene.input.geometry);
                if recreate {
                    let device = state.device.as_ref().ok_or("lighting device missing")?;
                    let mut prepared = TracingSlot { active: None };
                    let request = SwitchRequest {
                        mode,
                        platform: state.prefs.platform(),
                        quality: Quality::for_mode(mode),
                        hz: state.prefs.update_hz,
                    };
                    let candidate =
                        pollster::block_on(prepared.try_switch(device, request, scene.clone()))
                            .and_then(|changed| {
                                if !changed {
                                    return Err("UNSUPPORTED: FALSE".into());
                                }
                                let runtime =
                                    prepared.active.take().ok_or("tracing candidate missing")?;
                                renderer.bind_texture(
                                    session.ids.light,
                                    runtime.tracer.texture().clone(),
                                    true,
                                )?;
                                Ok(runtime)
                            });
                    match candidate {
                        Ok(runtime) => {
                            slot.switch(Some(runtime));
                            state.active = selection;
                            state.prefs.lighting = selection;
                            state.prefs.pending = None;
                            state.pending = None;
                            state.status = "LIGHTING CHANGED".into();
                            println!("LIGHTING SWITCH: {:?}", state.active);
                        }
                        Err(error) => {
                            state.status = format!("CHANGE FAILED: {error}");
                            state.pending = None;
                        }
                    }
                } else if let Some(runtime) = slot.active.as_mut() {
                    runtime.submit(scene)?;
                }
                if let Some(runtime) = slot.active.as_ref() {
                    renderer.bind_texture(
                        session.ids.light,
                        runtime.tracer.texture().clone(),
                        true,
                    )?;
                }
            }
            if let Some(shared) = PREFERENCES.get() {
                *shared.lock().unwrap() = state.prefs;
            }
        }
        let check = std::env::args().any(|a| a == "--gpu-check");
        light.compute_enabled = state.active == Selection::M5 || check;
        if check && state.active != state.prefs.requested() {
            return Err("requested lighting returned false / not applied; this is not an RTX validation success".into());
        }
        if check && !state.checked {
            if let Some(runtime) = slot.active.as_ref() {
                let field = runtime.tracer.readback()?;
                if field.rgba.iter().flatten().any(|v| !v.is_finite()) {
                    return Err("non-finite trace sample".into());
                }
                println!(
                    "RT sample: {:?}, samples={}, Tilemap/Sand common optics, field finite",
                    state.active,
                    runtime.tracer.samples()
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
    let hdr_active =
        matches!(state.active, Selection::Rtx | Selection::Path) && slot.active.is_some();
    renderer.set_hdr(hdr_active.then_some(charon_render::gpu::HdrSettings::default()))?;
    if let Some(session) = world.get_mut::<scenes::Session>()
        && let Some(light) = session.light.as_mut()
    {
        light.set_surface_composite(renderer, hdr_active)?;
    }
    ui::text(
        frame,
        500.,
        12.,
        &format!("{} / C SETTINGS", label(state.active)),
        [1., 0.8, 0.3, 1.],
    );
    if let Some(runtime) = slot.active.as_ref() {
        ui::text(
            frame,
            500.,
            frame.size[1].saturating_sub(20) as f32,
            &format!(
                "SPP {} / GPU {} MS",
                runtime.tracer.samples(),
                runtime
                    .tracer
                    .metrics()
                    .gpu_ms
                    .map_or("N/A".into(), |v| format!("{v:.1}"))
            ),
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
        if let Some(runtime) = slot.active.as_ref() {
            let m = runtime.tracer.metrics();
            ui::text(
                frame,
                48.,
                556.,
                &format!(
                    "CPU {:.1} / MESH {:.1} MS / BUFFERS {:.1} MB",
                    m.submit_ms,
                    m.mesh_ms,
                    m.estimated_bytes as f64 / 1048576.
                ),
                [0.7, 0.8, 0.9, 1.],
            );
        }
        for q in &mut frame.quads[overlay_start + 1..] {
            q.layer = 110;
        }
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
                    last_scene_update: std::time::Instant::now(),
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
