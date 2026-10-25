mod setup;
use charon_engine::{
    input::{Button, InputEvent, Key},
    scenes::SceneManager,
};
use charon_m5_sample::{
    host::{self, RunOptions},
    scenes, ui,
};
use charon_render::{DrawFrame, gpu::GpuRenderer};
#[derive(Default)]
struct Inspector {
    request: bool,
    #[cfg(feature = "lighting")]
    checked: bool,
    #[cfg(feature = "lighting")]
    reading: Option<(u64, [f32; 4])>,
}
#[derive(Clone, Copy)]
struct High(bool);
fn event(manager: &mut SceneManager, event: InputEvent) -> Result<(), String> {
    if event
        == (InputEvent::Button {
            button: Button::Key(Key('U' as u32)),
            pressed: true,
        })
        && let Some(w) = scenes::world_mut(manager)
        && let Some(i) = w.get_mut::<Inspector>()
    {
        i.request = true;
    }
    Ok(())
}
fn frame(
    manager: &mut SceneManager,
    _renderer: &mut GpuRenderer,
    frame: &mut DrawFrame,
) -> Result<(), String> {
    let w = scenes::world_mut(manager).ok_or("API scene world missing")?;
    let high = w.get::<High>().ok_or("API mode missing")?.0;
    let name = if high { "HIGH" } else { "LOW" };
    ui::text(
        frame,
        500.,
        16.,
        &format!("API {name} / U LIGHT SAMPLE"),
        [1., 0.8, 0.3, 1.],
    );
    #[cfg(feature = "lighting")]
    {
        let Some(mut inspector) = w.remove::<Inspector>() else {
            return Ok(());
        };
        let check = std::env::args().any(|a| a == "--gpu-check");
        if let Some(light) = w
            .get_mut::<scenes::Session>()
            .and_then(|s| s.light.as_mut())
        {
            if inspector.request || (check && !inspector.checked) {
                let field = light.readback()?;
                let value = if high {
                    use charon_engine::lighting::Lighting;
                    let mut api = Lighting::from_scene(light.scene().clone(), 30)?;
                    if !api.accept(field)? {
                        return Err("stale GPU sample in API lab".into());
                    }
                    api.sample_world([252., 132.])
                } else {
                    field.sample([252., 132.])
                }
                .ok_or("missing world light sample")?;
                if check && value[1] < 0.5 {
                    return Err("emitting ore world sample unexpectedly dark".into());
                }
                inspector.reading = Some((light.scene().revision(), value));
                inspector.request = false;
                inspector.checked = true;
                println!("M6 API {name}: world light at (252,132) = {value:?}");
            }
            if let Some((revision, value)) = inspector.reading {
                let text = if revision == light.scene().revision() {
                    format!(
                        "RGB {:.2} {:.2} {:.2} AT 252 132",
                        value[0], value[1], value[2]
                    )
                } else {
                    "LIGHT CHANGED / U RESAMPLE".into()
                };
                ui::text(
                    frame,
                    500.,
                    frame.size[1].saturating_sub(20) as f32,
                    &text,
                    [1., 0.8, 0.3, 1.],
                );
            }
        }
        w.insert(inspector);
    }
    #[cfg(not(feature = "lighting"))]
    ui::text(
        frame,
        500.,
        frame.size[1].saturating_sub(20) as f32,
        "LIGHTING NOT COMPILED",
        [1., 0.8, 0.3, 1.],
    );
    Ok(())
}
fn configure(app: &mut charon_engine::App, high: bool) {
    app.world_mut().insert(High(high));
    app.world_mut().insert(Inspector::default());
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let api = args
        .iter()
        .position(|a| a == "--api")
        .map(|i| args.get(i + 1).ok_or("missing --api value"))
        .transpose()?
        .map(String::as_str)
        .unwrap_or("high");
    if !["high", "low"].contains(&api) {
        return Err("--api must be high or low".into());
    }
    if args.iter().any(|a| a == "--smoke-test") {
        return setup::smoke();
    }
    host::run(RunOptions {
        title: if api == "high" {
            "Charon M6 / API Lab [HIGH]"
        } else {
            "Charon M6 / API Lab [LOW]"
        },
        ready_marker: "M6 READY: first frame presented",
        manager: if api == "high" {
            setup::high
        } else {
            setup::low
        },
        frame_hook: Some(frame),
        event_hook: Some(event),
    })
}
