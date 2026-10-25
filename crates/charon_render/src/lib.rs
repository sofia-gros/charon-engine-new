//! Generic rendering packets and replaceable backend. No tile/particle types.
#[cfg(feature = "gpu")]
pub mod gpu;
use charon_core::{Plugin, PluginResult, Step, World};
#[cfg(feature = "gpu")]
pub use winit;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PassId(pub u64);
#[derive(Clone, Debug)]
pub struct Quad {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub tint: [f32; 4],
    pub texture: TextureId,
    pub layer: i32,
    pub pass: PassId,
}
#[derive(Clone, Debug)]
pub struct DrawFrame {
    pub size: [u32; 2],
    pub clear: [f64; 4],
    pub quads: Vec<Quad>,
    pub clips: std::collections::BTreeMap<PassId, [u32; 4]>,
}
impl DrawFrame {
    pub fn new(size: [u32; 2]) -> Self {
        Self {
            size,
            clear: [0.035, 0.06, 0.12, 1.],
            quads: Vec::new(),
            clips: Default::default(),
        }
    }
}
pub trait Backend {
    fn render(&mut self, frame: &DrawFrame) -> Result<(), String>;
}
/// The host populates DrawFrame before App::frame. GPU backend creation stays
/// on the platform's window thread; Core knows only this Plugin contract.
pub struct RenderingPlugin<B: Backend + 'static> {
    backend: B,
    size: [u32; 2],
    owns: bool,
}
impl<B: Backend + 'static> RenderingPlugin<B> {
    pub fn new(backend: B, size: [u32; 2]) -> Self {
        Self {
            backend,
            size,
            owns: false,
        }
    }
}
impl<B: Backend + 'static> Plugin for RenderingPlugin<B> {
    fn name(&self) -> &'static str {
        "rendering"
    }
    fn init(&mut self, world: &mut World) -> PluginResult {
        if world.get::<DrawFrame>().is_some() {
            return Err("DrawFrame already installed".into());
        }
        world.insert(DrawFrame::new(self.size));
        self.owns = true;
        Ok(())
    }
    fn frame_update(&mut self, world: &mut World, _: Step) -> PluginResult {
        self.backend
            .render(world.get::<DrawFrame>().ok_or("DrawFrame missing")?)
    }
    fn shutdown(&mut self, world: &mut World) -> PluginResult {
        if self.owns {
            world.remove::<DrawFrame>();
            self.owns = false;
        }
        Ok(())
    }
}
pub const DEFAULT_SHADER: &str = include_str!("quad.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    use charon_core::App;
    use std::{cell::RefCell, rc::Rc, time::Duration};
    struct Recorder(Rc<RefCell<Vec<usize>>>);
    impl Backend for Recorder {
        fn render(&mut self, frame: &DrawFrame) -> Result<(), String> {
            self.0.borrow_mut().push(frame.quads.len());
            Ok(())
        }
    }
    #[test]
    fn rendering_plugin_uses_only_generic_packets_and_releases_resource() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut app = App::new();
        app.add_plugin(RenderingPlugin::new(Recorder(calls.clone()), [64, 64]))
            .unwrap();
        app.start().unwrap();
        app.frame(Duration::from_secs_f64(1. / 60.)).unwrap();
        assert_eq!(*calls.borrow(), [0]);
        app.shutdown().unwrap();
        assert!(app.world().get::<DrawFrame>().is_none());
    }
    #[test]
    fn failed_init_does_not_remove_host_frame() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut app = App::new();
        app.world_mut().insert(DrawFrame::new([8, 8]));
        app.add_plugin(RenderingPlugin::new(Recorder(calls), [64, 64]))
            .unwrap();
        assert!(app.start().is_err());
        assert_eq!(app.world().get::<DrawFrame>().unwrap().size, [8, 8]);
    }
}
