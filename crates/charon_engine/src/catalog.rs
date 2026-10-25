use crate::{AppBuilder, Startup};
use charon_scene::{Scene, SceneId, SceneManager};
/// Builds scene factories with the same timing as their manager. Factories run at
/// each creation/restart; never reuse a stopped App or its resource handles.
pub struct SceneCatalog {
    manager: SceneManager,
    startup: Startup,
}
impl SceneCatalog {
    pub fn new(startup: Startup) -> Result<Self, String> {
        Ok(Self {
            manager: SceneManager::with_timing(
                startup.rates().delta(crate::Lane::Game),
                startup.max_catch_up(),
            )?,
            startup,
        })
    }
    pub fn scene(
        mut self,
        name: impl Into<String>,
        mut factory: impl FnMut(SceneId, AppBuilder) -> Result<Box<dyn Scene>, String> + 'static,
    ) -> Result<Self, String> {
        let startup = self.startup;
        self.manager.register(name, move |id| {
            factory(id, AppBuilder::new(startup).map_err(|e| e.to_string())?)
        })?;
        Ok(self)
    }
    /// Escape hatch for direct registrations and manager configuration.
    pub fn manager_mut(&mut self) -> &mut SceneManager {
        &mut self.manager
    }
    pub fn build(mut self, initial: &str) -> Result<SceneManager, String> {
        self.manager.switch_to(initial)?;
        Ok(self.manager)
    }
}
