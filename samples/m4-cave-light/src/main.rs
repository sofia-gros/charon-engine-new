use charon_m3_sample::runtime;
#[cfg(feature = "lighting")]
mod illumination;
mod ui;
mod world;
use charon_assets::{Assets, Image, ShaderSource};
use charon_render::{
    Backend, DrawFrame, PassId, TextureId,
    gpu::GpuRenderer,
    winit::{
        application::ApplicationHandler,
        dpi::LogicalSize,
        event::{ElementState, MouseButton, WindowEvent},
        event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
        keyboard::{KeyCode, PhysicalKey},
        window::{Window, WindowId},
    },
};
use charon_tilemap::{Atlas, Sprite, TileId};
use runtime::{Controls, Edit, Runtime};
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};
const SIZE: [u32; 2] = [960, 696];
const TOP: f32 = 72.;
struct Lab {
    runtime: Runtime,
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    keys: HashSet<KeyCode>,
    mouse: [f32; 2],
    selected: TileId,
    next_frame: Instant,
    frames: u64,
    error: Option<String>,
    gpu_check: bool,
    compute_hz: u32,
    coupled: bool,
    #[cfg(feature = "lighting")]
    light: Option<illumination::Illumination>,
    lighting_hz: u32,
    start_lighting: bool,
    initial_cell_size: f32,
    atlas: Atlas,
    images: Assets<Image>,
    image_handle: Option<charon_assets::Handle<Image>>,
}
impl Lab {
    fn send(&mut self, edit: Edit) {
        if let Err(error) = self.runtime.edits.try_send(edit) {
            self.error = Some(format!("input queue: {error}"));
        }
    }
    fn key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Digit1 => self.selected = TileId(0),
            KeyCode::Digit2 => self.selected = TileId(1),
            KeyCode::Digit3 => self.selected = TileId(2),
            #[cfg(feature = "lighting")]
            KeyCode::KeyL
            | KeyCode::KeyT
            | KeyCode::KeyO
            | KeyCode::KeyV
            | KeyCode::KeyK
            | KeyCode::KeyQ => {
                if let Some(light) = &mut self.light {
                    match key {
                        KeyCode::KeyL => light.enabled = !light.enabled,
                        KeyCode::KeyT => light.torch = !light.torch,
                        KeyCode::KeyO => light.ore = !light.ore,
                        KeyCode::KeyV => light.ao = !light.ao,
                        KeyCode::KeyK => light.sky = !light.sky,
                        KeyCode::KeyQ => {
                            if let Some(renderer) = &mut self.renderer
                                && let Err(e) = light.quality(renderer)
                            {
                                self.error = Some(e)
                            }
                        }
                        _ => {}
                    }
                }
            }
            #[cfg(feature = "bridge")]
            KeyCode::KeyF if self.coupled => {
                let player = *self.runtime.shared.player.lock().unwrap();
                self.send(Edit::Paint {
                    x: player.position[0] as i32 + 12,
                    y: player.position[1] as i32,
                    radius: 2,
                    cell: charon_physics::sim::material::CellData::fire(120),
                });
            }
            KeyCode::KeyG => {
                let view = self.runtime.shared.view.lock().unwrap().clone();
                for y in 14..17 {
                    self.send(if view.map.get(19, y).is_some() {
                        Edit::Break(19, y)
                    } else {
                        Edit::Place(19, y, TileId(0))
                    });
                }
            }
            KeyCode::KeyB => {
                let view = self.runtime.shared.view.lock().unwrap().clone();
                let edits: Vec<_> = view
                    .map
                    .occupied()
                    .filter(|&(_, y, id)| {
                        if key == KeyCode::KeyB {
                            id == TileId(1) || id == TileId(2)
                        } else {
                            y == 12
                        }
                    })
                    .map(|(x, y, _)| Edit::Break(x, y))
                    .collect();
                for edit in edits {
                    self.send(edit)
                }
            }
            _ => {}
        }
    }
    fn draw(&mut self) -> Result<(), String> {
        self.runtime.check()?;
        let window = self.window.as_ref().ok_or("window missing")?;
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let view = self.runtime.shared.view.lock().unwrap().clone();
        let player = *self.runtime.shared.player.lock().unwrap();
        let mut frame = DrawFrame::new([size.width, size.height]);
        frame.quads.push(charon_render::Quad {
            rect: [0., TOP, size.width as f32, size.width as f32 / 320. * 192.],
            uv: [0., 0., 1., 1.],
            tint: [0.12, 0.17, 0.22, 1.],
            texture: TextureId(0),
            layer: -1,
            pass: PassId(1),
        });
        let scale = size.width as f32 / 320.;
        let origin = [0., TOP];
        charon_tilemap_render::append_tiles(
            &mut frame,
            &view.map,
            &self.atlas,
            origin,
            scale,
            0,
            PassId(1),
        )?;
        #[cfg(feature = "bridge")]
        if let Some(snapshot) = &view.sand {
            self.renderer
                .as_mut()
                .unwrap()
                .upload(TextureId(2), &charon_sand_render::image(snapshot)?)?;
            charon_sand_render::append(
                &mut frame,
                TextureId(2),
                [0., TOP, 320. * scale, 192. * scale],
                1,
                PassId(1),
            );
        }
        for position in runtime::GEMS.iter().skip(player.score as usize) {
            charon_tilemap_render::append_sprite(
                &mut frame,
                &Sprite {
                    position: [position[0] * scale, TOP + position[1] * scale],
                    size: [6. * scale, 6. * scale],
                    atlas_index: 3,
                    layer: 2,
                    tint: [1.; 4],
                },
                &self.atlas,
                PassId(1),
            )?;
        }
        charon_tilemap_render::append_sprite(
            &mut frame,
            &Sprite {
                position: [player.position[0] * scale, TOP + player.position[1] * scale],
                size: [6. * scale, 6. * scale],
                atlas_index: 3,
                layer: 3,
                tint: [0.4, 1., 0.9, 1.],
            },
            &self.atlas,
            PassId(1),
        )?;
        #[cfg(feature = "lighting")]
        if let Some(light) = &mut self.light {
            light.update(
                self.renderer.as_mut().unwrap(),
                &view,
                player,
                [size.width, size.height],
                self.lighting_hz,
            )?;
            charon_lighting_render::assign_pass(
                &mut frame,
                if light.enabled { PassId(2) } else { PassId(0) },
                99,
            );
        }
        let white = [0.88, 0.94, 1., 1.];
        let mint = [0.35, 0.95, 0.75, 1.];
        ui::text(&mut frame, 16., 12., "CHARON / M4 CAVE LIGHT", white);
        ui::text(
            &mut frame,
            500.,
            12.,
            if self.coupled {
                "TILEMAP + FALLING SAND"
            } else {
                "TILEMAP ONLY"
            },
            mint,
        );
        ui::text(
            &mut frame,
            16.,
            36.,
            &format!(
                "GAME 144 TICK {} / RENDER 60 / COMPUTE {} TICK {}",
                player.ticks, self.compute_hz, view.ticks
            ),
            mint,
        );
        ui::text(
            &mut frame,
            16.,
            56.,
            &format!(
                "GEMS {} / 3 / TILE {} / REV {} / G DOOR / B RELEASE / F FIRE",
                player.score,
                self.selected.0 + 1,
                view.revision
            ),
            white,
        );
        ui::text(
            &mut frame,
            16.,
            656.,
            "WASD MOVE / CLICK DIG-BUILD / T TORCH / O ORE / V AO / L LIGHT",
            white,
        );
        ui::text(
            &mut frame,
            16.,
            676.,
            &format!(
                "{} / LIGHT {} HZ / K SKY / Q QUALITY",
                view.message, self.lighting_hz
            ),
            mint,
        );
        self.renderer.as_mut().unwrap().render(&frame)?;
        self.frames += 1;
        if self.gpu_check && self.frames == 3 {
            #[cfg(feature = "lighting")]
            if let Some(light) = &mut self.light {
                light.verify()?;
            }

            let renderer = self.renderer.as_mut().unwrap();
            let result = pollster::block_on(renderer.register_pass(
                PassId(1),
                &ShaderSource("invalid shader".into()),
                [1.; 4],
            ));
            if result.is_ok() {
                return Err("invalid shader was accepted".into());
            }
            println!("GPU shader diagnostic OK: {}", result.unwrap_err());
            let _ = window.request_inner_size(LogicalSize::new(800., 680.));
        }
        Ok(())
    }
}
impl ApplicationHandler for Lab {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let setup = (|| -> Result<(), String> {
            let window = Arc::new(
                event_loop
                    .create_window(
                        Window::default_attributes()
                            .with_title("Charon M4 / Cave Light")
                            .with_inner_size(LogicalSize::new(SIZE[0], SIZE[1])),
                    )
                    .map_err(|e| e.to_string())?,
            );
            let mut renderer = pollster::block_on(GpuRenderer::new(window.clone()))?;
            let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
            let handle = self.images.load(base.join("atlas.png"), Image::decode)?;
            renderer.upload(
                TextureId(1),
                self.images
                    .get(handle)
                    .ok_or("image handle invalid")?
                    .as_ref(),
            )?;
            self.image_handle = Some(handle);
            let mut shaders = Assets::<ShaderSource>::default();
            let shader = shaders.load(base.join("world.wgsl"), ShaderSource::decode)?;
            pollster::block_on(renderer.register_pass(
                PassId(1),
                shaders.get(shader).ok_or("shader handle invalid")?.as_ref(),
                [1., 1., 1., 0.],
            ))?;
            #[cfg(feature = "lighting")]
            if self.start_lighting {
                self.light = Some(illumination::Illumination::new(
                    &mut renderer,
                    self.initial_cell_size,
                )?);
            }
            #[cfg(not(feature = "lighting"))]
            let _ = (self.start_lighting, self.initial_cell_size);
            println!("GPU: {}", renderer.adapter_name);
            self.renderer = Some(renderer);
            self.window = Some(window);
            Ok(())
        })();
        if let Err(error) = setup {
            self.error = Some(error);
            event_loop.exit();
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let result: Result<(), String> = (|| {
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::Focused(false) => {
                    self.keys.clear();
                    *self.runtime.shared.controls.lock().unwrap() = Controls::default();
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        if event.state == ElementState::Pressed {
                            self.keys.insert(code);
                            if !event.repeat {
                                self.key(code);
                            }
                        } else {
                            self.keys.remove(&code);
                        }
                        if code == KeyCode::Escape {
                            event_loop.exit();
                        }
                        *self.runtime.shared.controls.lock().unwrap() = Controls {
                            dx: (self.keys.contains(&KeyCode::KeyD) as i8
                                - self.keys.contains(&KeyCode::KeyA) as i8)
                                as f32,
                            dy: (self.keys.contains(&KeyCode::KeyS) as i8
                                - self.keys.contains(&KeyCode::KeyW) as i8)
                                as f32,
                        };
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    self.mouse = [position.x as f32, position.y as f32]
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button,
                    ..
                } => {
                    let scale = self.window.as_ref().unwrap().inner_size().width as f32 / 320.;
                    let x = self.mouse[0] / (8. * scale);
                    let y = (self.mouse[1] - TOP) / (8. * scale);
                    if x >= 0. && y >= 0. && x < runtime::W as f32 && y < runtime::H as f32 {
                        match button {
                            MouseButton::Left => self.send(Edit::Break(x as u32, y as u32)),
                            MouseButton::Right => {
                                self.send(Edit::Place(x as u32, y as u32, self.selected))
                            }
                            _ => {}
                        }
                    }
                }
                WindowEvent::RedrawRequested => {
                    self.draw()?;
                    if self.gpu_check && self.frames >= 12 {
                        println!(
                            "GPU initialization, atlas upload, custom shader, failure recovery, resize, draw OK"
                        );
                        event_loop.exit();
                    }
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.error = Some(error);
            event_loop.exit();
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now >= self.next_frame {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            self.next_frame = now + Duration::from_secs_f64(1. / 60.);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|s| s == "--smoke-test") {
        return world::smoke();
    }
    let hz = args
        .iter()
        .position(|s| s == "--compute-hz")
        .map(|i| {
            args.get(i + 1)
                .ok_or("missing compute rate")?
                .parse::<u32>()
                .map_err(|_| "invalid compute rate")
        })
        .transpose()?
        .unwrap_or(60);
    let coupled = cfg!(feature = "bridge") && !args.iter().any(|s| s == "--tilemap-only");
    let lighting_hz = args
        .iter()
        .position(|s| s == "--lighting-hz")
        .map(|i| {
            args.get(i + 1)
                .ok_or("missing lighting rate")?
                .parse::<u32>()
                .map_err(|_| "invalid lighting rate")
        })
        .transpose()?
        .unwrap_or(30);
    if !(10..=60).contains(&lighting_hz) {
        return Err("lighting rate must be 10..60 Hz".into());
    }
    let initial_cell_size = args
        .iter()
        .position(|s| s == "--light-cell-size")
        .map(|i| {
            args.get(i + 1)
                .ok_or("missing light cell size")?
                .parse::<f32>()
                .map_err(|_| "invalid light cell size")
        })
        .transpose()?
        .unwrap_or(4.);
    if ![1., 2., 4., 8.].contains(&initial_cell_size) {
        return Err("light cell size must be 1,2,4,8".into());
    }
    let runtime = Runtime::start_with_map(hz, coupled, world::map())?;
    let mut lab = Lab {
        runtime,
        window: None,
        renderer: None,
        keys: HashSet::new(),
        mouse: [0.; 2],
        selected: TileId(0),
        next_frame: Instant::now(),
        frames: 0,
        error: None,
        gpu_check: args.iter().any(|s| s == "--gpu-check"),
        #[cfg(feature = "lighting")]
        light: None,
        lighting_hz,
        initial_cell_size,
        start_lighting: !args.iter().any(|s| s == "--no-lighting"),
        compute_hz: hz,
        coupled,
        atlas: Atlas {
            texture: 1,
            size: [32, 8],
            tile_size: [8, 8],
        },
        images: Assets::default(),
        image_handle: None,
    };
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut lab)?;
    lab.runtime.shutdown()?;
    if let Some(handle) = lab.image_handle.take() {
        lab.images.unload(handle);
        if let Some(renderer) = &mut lab.renderer {
            renderer.remove_texture(TextureId(1));
        }
    }
    if let Some(error) = lab.error {
        return Err(error.into());
    }
    Ok(())
}
