use super::*;
use charon_core::{Plugin, PluginResult, Step, World};
use std::time::Duration;
/// A compute-lane resource. Caller owns submitted snapshots and GPU scheduling.
pub struct Tracing {
    pub tracer: GpuTrace,
    input: Option<TraceScene>,
    hz: u32,
    elapsed: Duration,
}
impl Tracing {
    pub fn new(tracer: GpuTrace, hz: u32) -> Result<Self, String> {
        if !(1..=120).contains(&hz) {
            return Err("tracing frequency must be 1..120Hz".into());
        }
        Ok(Self {
            tracer,
            input: None,
            hz,
            elapsed: Duration::ZERO,
        })
    }
    pub fn submit(&mut self, input: TraceScene) -> Result<(), String> {
        input.validate()?;
        if input.input.geometry != self.tracer.geometry {
            return Err("trace input geometry mismatch".into());
        }
        self.input = Some(input);
        Ok(())
    }
    pub fn tick(&mut self, delta: Duration) -> Result<(), String> {
        self.elapsed = self.elapsed.saturating_add(delta);
        let interval = Duration::from_secs_f64(1. / self.hz as f64);
        if self.elapsed >= interval {
            self.elapsed =
                Duration::from_nanos((self.elapsed.as_nanos() % interval.as_nanos()) as u64);
            if self.tracer.is_idle()?
                && let Some(input) = &self.input
            {
                self.tracer.render(input)?;
            }
        }
        Ok(())
    }
}
pub struct TracingSlot {
    pub active: Option<Tracing>,
}
/// Explicit caller-supplied rendering parameters; no persistence or preference policy.
pub struct SwitchRequest {
    pub mode: Mode,
    pub platform: Platform,
    pub quality: Quality,
    pub hz: u32,
}
impl TracingSlot {
    /// Unsupported requests return false and retain the active backend. Preparation
    /// errors also retain it, but report an error instead of masquerading as support.
    pub async fn try_switch(
        &mut self,
        device: &LightingDevice,
        request: SwitchRequest,
        scene: TraceScene,
    ) -> Result<bool, String> {
        if request.mode == Mode::Rtx
            && request.mode.validate(request.platform).is_ok()
            && !device.probe_rtx().await?
        {
            return Ok(false);
        }
        if !device.supports(request.mode, request.platform) {
            return Ok(false);
        }
        if request.mode == Mode::Legacy {
            self.active = None;
            return Ok(true);
        }
        let tracer = device
            .prepare(
                scene.input.geometry,
                request.mode,
                request.platform,
                request.quality,
            )
            .await?;
        let mut candidate = Tracing::new(tracer, request.hz)?;
        device
            .raw_device()
            .push_error_scope(wgpu::ErrorFilter::Validation);
        candidate.submit(scene.clone())?;
        let checked = candidate
            .tracer
            .render(&scene)
            .and_then(|_| candidate.tracer.readback())
            .and_then(|field| {
                if field.rgba.iter().flatten().any(|v| !v.is_finite()) {
                    Err("first tracing dispatch produced non-finite output".into())
                } else {
                    Ok(())
                }
            });
        let validation = device.raw_device().pop_error_scope().await;
        if let Some(error) = validation {
            return Err(error.to_string());
        }
        checked?;
        self.active = Some(candidate);
        Ok(true)
    }

    /// Commit a backend prepared by the caller. Preparing/validating a candidate
    /// never modifies the active backend. None disables tracing for another plugin.
    pub fn switch(&mut self, candidate: Option<Tracing>) -> Option<Tracing> {
        std::mem::replace(&mut self.active, candidate)
    }
}
pub struct TracingPlugin {
    initial: Option<TracingSlot>,
    owns: bool,
}
impl TracingPlugin {
    pub fn idle() -> Self {
        Self {
            initial: Some(TracingSlot { active: None }),
            owns: false,
        }
    }
    pub fn new(runtime: Tracing) -> Self {
        Self {
            initial: Some(TracingSlot {
                active: Some(runtime),
            }),
            owns: false,
        }
    }
}
impl Plugin for TracingPlugin {
    fn name(&self) -> &'static str {
        "tracing"
    }
    fn init(&mut self, w: &mut World) -> PluginResult {
        if w.get::<TracingSlot>().is_some() {
            return Err("Tracing already installed".into());
        }
        w.insert(self.initial.take().ok_or("tracing already initialized")?);
        self.owns = true;
        Ok(())
    }
    fn fixed_update(&mut self, w: &mut World, step: Step) -> PluginResult {
        if let Some(runtime) = w
            .get_mut::<TracingSlot>()
            .ok_or("Tracing slot missing")?
            .active
            .as_mut()
        {
            runtime.tick(step.delta)?;
        }
        Ok(())
    }
    fn shutdown(&mut self, w: &mut World) -> PluginResult {
        if self.owns {
            w.remove::<TracingSlot>();
            self.owns = false;
        }
        Ok(())
    }
}
