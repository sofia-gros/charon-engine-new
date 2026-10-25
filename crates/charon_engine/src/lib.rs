//! Optional composition layer. The default build depends only on `charon_core`.
//! No window or world is created implicitly. See the crate README for feature selection.
//!
//! ```
//! use charon_engine::{AppBuilder, Startup};
//! let mut app = AppBuilder::new(Startup::default())?.build();
//! app.start()?;
//! app.tick()?;
//! app.shutdown()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
pub use charon_core as core;
pub use charon_core::{App, CoreError, Plugin, World};
use std::time::Duration;

#[cfg(feature = "assets")]
pub use charon_assets as assets;
#[cfg(feature = "bridge")]
pub use charon_bridge as bridge;
#[cfg(feature = "camera")]
pub use charon_camera as camera;
#[cfg(feature = "input")]
pub use charon_input as input;
#[cfg(feature = "physics")]
pub use charon_physics as physics;
#[cfg(feature = "render")]
pub use charon_render as render;
#[cfg(feature = "sand")]
pub use charon_sand as sand;
#[cfg(feature = "scenes")]
pub use charon_scene as scenes;
#[cfg(feature = "tilemap")]
pub use charon_tilemap as tilemap;
#[cfg(feature = "scenes")]
mod catalog;
#[cfg(feature = "lighting")]
pub mod lighting;
#[cfg(feature = "scenes")]
pub use catalog::SceneCatalog;
pub mod bundles;

/// Independent target rates. These are scheduling targets, not measured performance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rates {
    game: u32,
    render: u32,
    compute: u32,
}
impl Default for Rates {
    fn default() -> Self {
        Self {
            game: 144,
            render: 60,
            compute: 60,
        }
    }
}
impl Rates {
    pub fn new(game: u32, render: u32, compute: u32) -> Result<Self, String> {
        if [game, render, compute]
            .iter()
            .any(|v| !(1..=1000).contains(v))
        {
            return Err("target rates must be 1..1000 Hz".into());
        }
        Ok(Self {
            game,
            render,
            compute,
        })
    }
    pub fn hz(self, lane: Lane) -> u32 {
        match lane {
            Lane::Game => self.game,
            Lane::Render => self.render,
            Lane::Compute => self.compute,
        }
    }
    pub fn delta(self, lane: Lane) -> Duration {
        Duration::from_secs_f64(1. / self.hz(lane) as f64)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Game,
    Render,
    Compute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Startup {
    rates: Rates,
    max_catch_up: u32,
}
impl Default for Startup {
    fn default() -> Self {
        Self {
            rates: Rates::default(),
            max_catch_up: 8,
        }
    }
}
impl Startup {
    pub fn new(rates: Rates, max_catch_up: u32) -> Result<Self, String> {
        if !(1..=1024).contains(&max_catch_up) {
            return Err("catch-up budget must be 1..1024".into());
        }
        Ok(Self {
            rates,
            max_catch_up,
        })
    }
    pub fn rates(self) -> Rates {
        self.rates
    }
    pub fn max_catch_up(self) -> u32 {
        self.max_catch_up
    }
    pub fn app(self, lane: Lane) -> Result<AppBuilder, CoreError> {
        AppBuilder::for_lane(self, lane)
    }
    /// Build the app in its owner thread. The returned worker joins on shutdown/drop.
    /// Rendering stays on the platform's event-loop thread; it is deliberately not spawned here.
    pub fn spawn<H: core::WorkerHooks>(
        self,
        lane: Lane,
        name: impl Into<String>,
        configure: impl FnOnce(AppBuilder) -> Result<AppBuilder, CoreError> + Send + 'static,
        hooks: H,
    ) -> Result<core::Worker, core::WorkerError> {
        if lane == Lane::Render {
            return Err(core::WorkerError::InvalidConfig);
        }
        let mut config = core::WorkerConfig::new(name, self.rates.hz(lane));
        config.max_catch_up = self.max_catch_up;
        core::Worker::spawn(
            config,
            move |_| Ok(configure(self.app(lane)?)?.build()),
            hooks,
        )
    }
}
/// Owns the unstarted low-level app. Methods consume the builder so failed bundles
/// cannot leave a partially installed app available for starting.
pub struct AppBuilder {
    app: App,
}
impl AppBuilder {
    pub fn new(startup: Startup) -> Result<Self, CoreError> {
        Self::for_lane(startup, Lane::Game)
    }
    pub fn for_lane(startup: Startup, lane: Lane) -> Result<Self, CoreError> {
        Ok(Self {
            app: App::with_timing(startup.rates.delta(lane), startup.max_catch_up)?,
        })
    }
    pub fn plugin(mut self, plugin: impl Plugin) -> Result<Self, CoreError> {
        self.app.add_plugin(plugin)?;
        Ok(self)
    }
    pub fn boxed_plugin(mut self, plugin: Box<dyn Plugin>) -> Result<Self, CoreError> {
        self.app.add_boxed_plugin(plugin)?;
        Ok(self)
    }
    pub fn bundle(self, bundle: impl bundles::PluginSet) -> Result<Self, CoreError> {
        bundle.install(self)
    }
    pub fn resource<T: 'static>(mut self, value: T) -> Self {
        self.app.world_mut().insert(value);
        self
    }
    pub fn app(&self) -> &App {
        &self.app
    }
    pub fn app_mut(&mut self) -> &mut App {
        &mut self.app
    }
    pub fn build(self) -> App {
        self.app
    }
}
