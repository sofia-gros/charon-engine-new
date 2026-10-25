mod controller;
mod scenes;
mod ui;
use charon_input::{Button, InputBarrier, InputEvent, keys};
use charon_render::{
    Backend,
    gpu::GpuRenderer,
    winit::{
        application::ApplicationHandler,
        dpi::LogicalSize,
        event::WindowEvent,
        event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
        window::{Window, WindowId},
    },
};
use charon_scene::{SceneId, SceneManager};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
struct Check {
    phase: u32,
    since: u64,
    scene: Option<SceneId>,
    image: Option<Weak<charon_assets::Image>>,
    deadline: Instant,
}
struct Host {
    window: Option<Arc<Window>>,
    graphics: Option<scenes::Graphics>,
    manager: Option<SceneManager>,
    size: Rc<Cell<[u32; 2]>>,
    hz: u32,
    prefer_sand: bool,
    barrier: InputBarrier,
    pending: VecDeque<(SceneId, InputEvent)>,
    pointer: Option<[f32; 2]>,
    focused: bool,
    last_scene: Option<SceneId>,
    last_frame: Instant,
    next_frame: Instant,
    frames: u64,
    error: Option<String>,
    check: Option<Check>,
}
impl Host {
    fn enqueue(&mut self, event: InputEvent) -> Result<(), String> {
        if let Some(id) = self.manager.as_ref().and_then(SceneManager::active_id) {
            if self.pending.len() >= 1024 {
                return Err("OS input backlog exceeded".into());
            }
            self.pending.push_back((id, event));
        }
        self.flush()
    }
    fn flush(&mut self) -> Result<(), String> {
        let Some(manager) = &self.manager else {
            return Ok(());
        };
        while let Some((id, event)) = self.pending.pop_front() {
            if Some(id) != manager.active_id() {
                continue;
            }
            let Some(client) = scenes::frontend(manager) else {
                continue;
            };
            match client.try_send(event) {
                Ok(()) => {}
                Err(std::sync::mpsc::TrySendError::Full(e)) => {
                    self.pending.push_front((id, e));
                    break;
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
    fn scene_boundary(&mut self) -> Result<(), String> {
        let id = self.manager.as_ref().and_then(SceneManager::active_id);
        if id != self.last_scene {
            self.last_scene = id;
            self.pending.clear();
            self.barrier.scene_changed();
            self.enqueue(InputEvent::Focus(self.focused || self.check.is_some()))?;
            self.enqueue(InputEvent::Resize(self.size.get()))?;
            if let Some(p) = self.pointer {
                self.enqueue(InputEvent::Pointer(p))?;
            }
        }
        Ok(())
    }
    fn draw(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        self.flush()?;
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;
        let manager = self.manager.as_mut().ok_or("scene manager missing")?;
        manager.advance(elapsed)?;
        for event in manager.drain_events() {
            if let Some(error) = event.error {
                return Err(format!("scene {} {}: {error}", event.name, event.phase));
            }
        }
        if manager.active_id().is_none() {
            event_loop.exit();
            return Ok(());
        }
        self.scene_boundary()?;
        let size = self.size.get();
        if size.contains(&0) {
            return Ok(());
        }
        {
            let mut renderer = self.graphics.as_ref().unwrap().borrow_mut();
            let frame = scenes::draw(self.manager.as_mut().unwrap(), &mut renderer, size)?;
            renderer.render(&frame)?;
        }
        self.frames += 1;
        self.gpu_check(event_loop)
    }
    fn gpu_check(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let Some(mut check) = self.check.take() else {
            return Ok(());
        };
        if Instant::now() > check.deadline {
            return Err("M5 GPU scene verification timed out".into());
        }
        let manager = self.manager.as_mut().unwrap();
        match check.phase {
            0 if self.frames >= 2 => {
                scenes::send(manager, InputEvent::Focus(true))?;
                scenes::pulse(manager, keys::ENTER)?;
                check.phase = 1;
            }
            1 if scenes::session(manager).is_some() => {
                let s = scenes::session(manager).unwrap();
                check.scene = Some(s.id);
                let image = scenes::world(manager)
                    .unwrap()
                    .get::<charon_assets::Assets<charon_assets::Image>>()
                    .unwrap()
                    .get(s.image.unwrap())
                    .unwrap();
                check.image = Some(Arc::downgrade(&image));
                let camera = *s.camera.lock().unwrap();
                scenes::send(manager, InputEvent::Pointer(camera.viewport().center()))?;
                scenes::send(manager, InputEvent::Wheel([0., 1.]))?;
                scenes::send(
                    manager,
                    InputEvent::Button {
                        button: keys::RIGHT.into(),
                        pressed: true,
                    },
                )?;
                if let Some(window) = &self.window {
                    let _ = window.request_inner_size(LogicalSize::new(900., 660.));
                }
                check.since = self.frames;
                check.phase = 2;
            }
            2 if self.frames >= check.since + 6 => {
                scenes::send(
                    manager,
                    InputEvent::Button {
                        button: keys::RIGHT.into(),
                        pressed: false,
                    },
                )?;
                let s = scenes::session(manager).unwrap();
                let camera = *s.camera.lock().unwrap();
                if camera.center() == [160., 96.] && camera.zoom() == 3. {
                    return Err("camera input was not applied".into());
                }
                let pointer = camera.world_to_screen([108., 84.])?;
                scenes::send(manager, InputEvent::Pointer(pointer))?;
                for pressed in [true, false] {
                    scenes::send(
                        manager,
                        InputEvent::Button {
                            button: Button::Mouse(0),
                            pressed,
                        },
                    )?;
                }
                check.phase = 3;
            }
            3 => {
                if let Some(s) = scenes::session(manager)
                    && s.runtime
                        .as_ref()
                        .unwrap()
                        .shared
                        .view
                        .lock()
                        .unwrap()
                        .map
                        .get(13, 10)
                        .is_none()
                {
                    #[cfg(feature = "lighting")]
                    if let Some(light) = &mut scenes::world_mut(manager)
                        .unwrap()
                        .get_mut::<scenes::Session>()
                        .unwrap()
                        .light
                    {
                        light.verify()?;
                    }
                    scenes::pulse(manager, charon_input::Key('R' as u32))?;
                    check.phase = 4;
                }
            }
            4 if manager.active_id() != check.scene && scenes::session(manager).is_some() => {
                if check.image.as_ref().unwrap().upgrade().is_some() {
                    return Err("old scene CPU asset retained".into());
                }
                let s = scenes::session(manager).unwrap();
                if *s.camera.lock().unwrap() != controller::camera(self.size.get()) {
                    return Err("camera leaked across restart".into());
                }
                if s.runtime
                    .as_ref()
                    .unwrap()
                    .shared
                    .view
                    .lock()
                    .unwrap()
                    .map
                    .get(13, 10)
                    .is_none()
                {
                    return Err("world edit leaked across restart".into());
                }
                #[cfg(feature = "lighting")]
                if let Some(light) = &mut scenes::world_mut(manager)
                    .unwrap()
                    .get_mut::<scenes::Session>()
                    .unwrap()
                    .light
                {
                    light.verify()?;
                }
                scenes::pulse(manager, keys::TAB)?;
                check.phase = 5;
            }
            5 if manager.active_name() == Some("menu") => {
                let graphics = self.graphics.as_ref().unwrap().borrow();
                graphics
                    .device()
                    .poll(charon_render::gpu::wait_for_completion())
                    .map_err(|e| e.to_string())?;
                if graphics.resource_counts() != (1, 1) {
                    return Err(format!(
                        "scene GPU resources retained: {:?}",
                        graphics.resource_counts()
                    ));
                }
                println!(
                    "M5 GPU: pan/zoom, inverse picking, light alignment, scene restart, CPU assets and GPU cleanup OK"
                );
                event_loop.exit();
                return Ok(());
            }
            _ => {}
        }
        self.check = Some(check);
        Ok(())
    }
}
impl ApplicationHandler for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let result = (|| -> Result<(), String> {
            let window = Arc::new(
                event_loop
                    .create_window(
                        Window::default_attributes()
                            .with_title("Charon M5 / Scene Lab")
                            .with_inner_size(LogicalSize::new(960., 696.)),
                    )
                    .map_err(|e| e.to_string())?,
            );
            let mut renderer = pollster::block_on(GpuRenderer::new(window.clone()))?;
            if self.check.is_some() {
                renderer.upload(
                    charon_render::TextureId(100),
                    &charon_assets::Image::new(1, 1, vec![255, 0, 0, 255])?,
                )?;
                if renderer.allocate_texture_id()? == charon_render::TextureId(100) {
                    return Err("render allocator overwrote a host resource ID".into());
                }
                renderer.remove_texture(charon_render::TextureId(100));
            }
            println!("GPU: {}", renderer.adapter_name);
            let graphics = Rc::new(RefCell::new(renderer));
            let size = window.inner_size();
            self.size.set([size.width, size.height]);
            self.manager = Some(scenes::manager(scenes::Context {
                graphics: Some(graphics.clone()),
                size: self.size.clone(),
                hz: self.hz,
                prefer_sand: self.prefer_sand,
            })?);
            self.graphics = Some(graphics);
            self.window = Some(window);
            self.scene_boundary()?;
            Ok(())
        })();
        if let Err(e) = result {
            self.error = Some(e);
            event_loop.exit();
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let result = (|| -> Result<(), String> {
            if let Some(input) = charon_input_winit::translate(&event) {
                match input {
                    InputEvent::Resize(size) => self.size.set(size),
                    InputEvent::Pointer(p) => self.pointer = Some(p),
                    InputEvent::PointerLeft => self.pointer = None,
                    InputEvent::Focus(f) => self.focused = f,
                    _ => {}
                }
                if let Some(input) = self.barrier.route(input) {
                    self.enqueue(input)?;
                }
            }
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::RedrawRequested => self.draw(event_loop)?,
                _ => {}
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.error = Some(e);
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
    if args.iter().any(|a| a == "--smoke-test") {
        return scenes::smoke();
    }
    let hz = args
        .iter()
        .position(|a| a == "--compute-hz")
        .map(|i| {
            args.get(i + 1)
                .ok_or("missing compute rate")?
                .parse::<u32>()
                .map_err(|_| "invalid compute rate")
        })
        .transpose()?
        .unwrap_or(60);
    if !(30..=60).contains(&hz) {
        return Err("compute rate must be 30..60".into());
    }
    let now = Instant::now();
    let mut host = Host {
        window: None,
        graphics: None,
        manager: None,
        size: Rc::new(Cell::new([960, 696])),
        hz,
        prefer_sand: cfg!(feature = "bridge") && !args.iter().any(|a| a == "--tilemap-only"),
        barrier: InputBarrier::default(),
        pending: VecDeque::new(),
        pointer: None,
        focused: true,
        last_scene: None,
        last_frame: now,
        next_frame: now,
        frames: 0,
        error: None,
        check: args.iter().any(|a| a == "--gpu-check").then_some(Check {
            phase: 0,
            since: 0,
            scene: None,
            image: None,
            deadline: now + Duration::from_secs(30),
        }),
    };
    EventLoop::new()?.run_app(&mut host)?;
    if let Some(manager) = &mut host.manager {
        manager.shutdown()?;
    }
    if let Some(error) = host.error {
        return Err(error.into());
    }
    Ok(())
}
