//! High-level optical configuration and revision-safe world sampling. CPU and GPU
//! fields use the same contract. This module has no renderer or physical world.
use crate::core::{Plugin, PluginResult, Step, World};
pub use charon_lighting as low_level;
pub use charon_lighting::{
    Geometry, LightField, Optical, OpticalGrid, PointLight, Prepared, Scene, Settings,
};
use std::time::Duration;

/// Resolution/update presets, independent of game and render clocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Low,
    High,
}
impl Quality {
    pub fn cell_size(self) -> f32 {
        match self {
            Self::Low => 4.,
            Self::High => 2.,
        }
    }
    pub fn update_hz(self) -> u32 {
        match self {
            Self::Low => 30,
            Self::High => 60,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightingConfig {
    geometry: Geometry,
    settings: Settings,
    update_hz: u32,
}
impl LightingConfig {
    pub fn new(geometry: Geometry, settings: Settings, update_hz: u32) -> Result<Self, String> {
        settings.steps(geometry)?;
        if !(1..=240).contains(&update_hz) {
            return Err("lighting update rate must be 1..240 Hz".into());
        }
        Ok(Self {
            geometry,
            settings,
            update_hz,
        })
    }
    /// Bounds use world units `[x,y,width,height]`. The last cell may extend beyond
    /// bounds when the extent is not a multiple of the preset's cell size.
    pub fn for_bounds(bounds: [f32; 4], quality: Quality) -> Result<Self, String> {
        if bounds.iter().any(|v| !v.is_finite()) || bounds[2] <= 0. || bounds[3] <= 0. {
            return Err("invalid world bounds".into());
        }
        let cell_size = quality.cell_size();
        Self::new(
            Geometry {
                width: (bounds[2] / cell_size).ceil() as u32,
                height: (bounds[3] / cell_size).ceil() as u32,
                origin: [bounds[0], bounds[1]],
                cell_size,
            },
            Settings::default(),
            quality.update_hz(),
        )
    }
    pub fn geometry(self) -> Geometry {
        self.geometry
    }
    pub fn settings(self) -> Settings {
        self.settings
    }
    pub fn update_hz(self) -> u32 {
        self.update_hz
    }
}
/// Own per scene/compute lane. Editing the low-level scene immediately makes old
/// samples unavailable until a current field is solved or accepted from a backend.
pub struct Lighting {
    scene: Scene,
    field: Option<LightField>,
    update_hz: u32,
}
impl Lighting {
    pub fn new(config: LightingConfig) -> Result<Self, String> {
        Self::from_scene(
            Scene::new(config.geometry, config.settings)?,
            config.update_hz,
        )
    }
    pub fn from_scene(scene: Scene, update_hz: u32) -> Result<Self, String> {
        LightingConfig::new(scene.geometry(), scene.settings(), update_hz)?;
        Ok(Self {
            scene,
            field: None,
            update_hz,
        })
    }
    pub fn scene(&self) -> &Scene {
        &self.scene
    }
    /// Invalidate even when the caller replaces the scene with another scene
    /// having the same revision. Outstanding backend jobs remain scene-scoped.
    pub fn scene_mut(&mut self) -> &mut Scene {
        self.field = None;
        &mut self.scene
    }
    pub fn update_hz(&self) -> u32 {
        self.update_hz
    }
    pub fn set_update_hz(&mut self, hz: u32) -> Result<(), String> {
        LightingConfig::new(self.scene.geometry(), self.scene.settings(), hz)?;
        self.update_hz = hz;
        Ok(())
    }
    pub fn set_layer(&mut self, id: u64, grid: OpticalGrid) -> Result<bool, String> {
        self.scene.set_layer(id, grid)
    }
    pub fn remove_layer(&mut self, id: u64) -> bool {
        self.scene.remove_layer(id)
    }
    pub fn set_light(&mut self, id: u64, light: PointLight) -> Result<bool, String> {
        self.scene.set_light(id, light)
    }
    pub fn remove_light(&mut self, id: u64) -> bool {
        self.scene.remove_light(id)
    }
    pub fn set_settings(&mut self, settings: Settings) -> Result<bool, String> {
        self.scene.set_settings(settings)
    }
    pub fn prepare(&self) -> Prepared {
        self.scene.prepare()
    }
    pub fn field(&self) -> Option<&LightField> {
        self.field
            .as_ref()
            .filter(|f| f.revision == self.scene.revision())
    }
    /// Nearest cell in world coordinates. None means outside, invalid coordinates,
    /// or pending computation; this never returns an obsolete light field.
    pub fn sample_world(&self, position: [f32; 2]) -> Option<[f32; 4]> {
        self.field()?.sample(position)
    }
    /// CPU reference path. Submit `prepare()` to a GPU backend instead for live GPU
    /// computation; `accept()` validates its readback without tying it to rendering.
    pub fn solve_cpu(&mut self) -> Result<bool, String> {
        if self.field().is_some() {
            return Ok(false);
        }
        self.accept(charon_lighting::solve_cpu(&self.scene.prepare())?)
    }
    /// False for a stale asynchronous result; malformed fields leave the cache intact.
    /// Backend fields must originate from this scene (revision is scoped to a scene).
    pub fn accept(&mut self, field: LightField) -> Result<bool, String> {
        if field.geometry != self.scene.geometry()
            || field.rgba.len() != field.geometry.len()
            || field
                .rgba
                .iter()
                .flatten()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err("invalid lighting backend field".into());
        }
        if field.revision != self.scene.revision() {
            return Ok(false);
        }
        self.scene.acknowledge(field.revision);
        self.field = Some(field);
        Ok(true)
    }
}
/// CPU reference plugin. Install in a compute lane; GPU clients should submit
/// prepared inputs through their own backend/queue instead of installing this.
pub struct CpuLightingPlugin {
    initial: Option<Lighting>,
    owns: bool,
    elapsed: Duration,
    first: bool,
}
impl CpuLightingPlugin {
    pub fn new(lighting: Lighting) -> Self {
        Self {
            initial: Some(lighting),
            owns: false,
            elapsed: Duration::ZERO,
            first: true,
        }
    }
}
impl Plugin for CpuLightingPlugin {
    fn name(&self) -> &'static str {
        "lighting"
    }
    fn init(&mut self, world: &mut World) -> PluginResult {
        if world.get::<Lighting>().is_some() {
            return Err("Lighting already installed".into());
        }
        world.insert(self.initial.take().ok_or("lighting already initialized")?);
        self.owns = true;
        Ok(())
    }
    fn fixed_update(&mut self, world: &mut World, step: Step) -> PluginResult {
        let lighting = world.get_mut::<Lighting>().ok_or("Lighting missing")?;
        let interval = Duration::from_secs_f64(1. / lighting.update_hz as f64);
        self.elapsed = self.elapsed.saturating_add(step.delta);
        if self.first || self.elapsed >= interval {
            self.first = false;
            self.elapsed =
                Duration::from_nanos((self.elapsed.as_nanos() % interval.as_nanos()) as u64);
            lighting.solve_cpu()?;
        }
        Ok(())
    }
    fn shutdown(&mut self, world: &mut World) -> PluginResult {
        if self.owns {
            world.remove::<Lighting>();
            self.owns = false;
        }
        Ok(())
    }
}
