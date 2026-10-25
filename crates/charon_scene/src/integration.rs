//! Optional Core scopes. Each scene has its own App/World and owned plugins.
use super::*;
use charon_core::{App, Plugin, PluginResult, World};
pub trait SceneLogic: 'static {
    fn start(&mut self, _: SceneId, _: &mut World) -> SceneResult {
        Ok(())
    }
    fn fixed_update(&mut self, _: &mut World, _: Step) -> Result<Directive, String> {
        Ok(Directive::Continue)
    }
    fn frame_update(&mut self, _: &mut World, _: Step) -> SceneResult {
        Ok(())
    }
    fn stop(&mut self, _: &mut World) -> SceneResult {
        Ok(())
    }
    fn destroy(&mut self, _: &mut World) -> SceneResult {
        Ok(())
    }
}
pub struct ScopedScene {
    app: App,
    logic: Box<dyn SceneLogic>,
}
impl ScopedScene {
    pub fn new(app: App, logic: impl SceneLogic) -> Self {
        Self {
            app,
            logic: Box::new(logic),
        }
    }
    pub fn world(&self) -> &World {
        self.app.world()
    }
    pub fn world_mut(&mut self) -> &mut World {
        self.app.world_mut()
    }
}
impl Scene for ScopedScene {
    fn start(&mut self, id: SceneId) -> SceneResult {
        self.app.start().map_err(|e| e.to_string())?;
        self.logic.start(id, self.app.world_mut())
    }
    fn fixed_update(&mut self, step: Step) -> Result<Directive, String> {
        if self.app.fixed_delta() != step.delta {
            return Err("scene App and manager fixed delta must match".into());
        }
        self.app.tick().map_err(|e| e.to_string())?;
        self.logic.fixed_update(self.app.world_mut(), step)
    }
    fn frame_update(&mut self, step: Step) -> SceneResult {
        self.app.frame(step.delta).map_err(|e| e.to_string())?;
        self.logic.frame_update(self.app.world_mut(), step)
    }
    fn stop(&mut self) -> SceneResult {
        let a = self.logic.stop(self.app.world_mut());
        let b = self.app.shutdown().map_err(|e| e.to_string());
        match (a, b) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(e), Ok(())) | (Ok(()), Err(e)) => Err(e),
            (Err(a), Err(b)) => Err(format!("{a}; {b}")),
        }
    }
    fn destroy(&mut self) -> SceneResult {
        self.logic.destroy(self.app.world_mut())
    }
}
/// Host-thread plugin; commands remain tagged with their originating scene ID.
pub struct ScenesPlugin {
    initial: Option<SceneManager>,
    owns: bool,
}
impl ScenesPlugin {
    pub fn new(manager: SceneManager) -> Self {
        Self {
            initial: Some(manager),
            owns: false,
        }
    }
}
impl Plugin for ScenesPlugin {
    fn name(&self) -> &'static str {
        "scenes"
    }
    fn init(&mut self, w: &mut World) -> PluginResult {
        if w.get::<SceneManager>().is_some() {
            return Err("SceneManager already exists".into());
        }
        w.insert(self.initial.take().ok_or("scenes already initialized")?);
        self.owns = true;
        Ok(())
    }
    fn fixed_update(&mut self, w: &mut World, step: charon_core::Step) -> PluginResult {
        w.get_mut::<SceneManager>()
            .ok_or("SceneManager missing")?
            .advance(step.delta)?;
        Ok(())
    }
    fn shutdown(&mut self, w: &mut World) -> PluginResult {
        if self.owns {
            self.owns = false;
            if let Some(mut manager) = w.remove::<SceneManager>() {
                manager.shutdown()?;
            }
        }
        Ok(())
    }
}
