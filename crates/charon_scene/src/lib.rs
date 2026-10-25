//! Scene ownership and lifecycle without a renderer, window or mandatory Core.
use std::{
    any::Any,
    collections::{BTreeMap, VecDeque},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
fn owner() -> u64 {
    let mut value = NEXT_OWNER.load(Ordering::Relaxed);
    loop {
        let next = value.checked_add(1).expect("scene owner IDs exhausted");
        match NEXT_OWNER.compare_exchange_weak(value, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return value,
            Err(n) => value = n,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SceneId {
    owner: u64,
    generation: u64,
}
impl SceneId {
    pub fn generation(self) -> u64 {
        self.generation
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Step {
    pub delta: Duration,
    pub index: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Directive {
    Continue,
    Switch(String),
    Stop,
}
pub type SceneResult = Result<(), String>;
/// stop/destroy must tolerate partial start. Both run once on every disposed
/// instance, even when another cleanup callback failed.
pub trait Scene: Any {
    fn start(&mut self, _: SceneId) -> SceneResult {
        Ok(())
    }
    fn fixed_update(&mut self, _: Step) -> Result<Directive, String> {
        Ok(Directive::Continue)
    }
    fn frame_update(&mut self, _: Step) -> SceneResult {
        Ok(())
    }
    fn stop(&mut self) -> SceneResult {
        Ok(())
    }
    fn destroy(&mut self) -> SceneResult {
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneEvent {
    pub id: SceneId,
    pub name: String,
    pub phase: &'static str,
    pub error: Option<String>,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct AdvanceReport {
    pub fixed_steps: u32,
    pub dropped_time: Duration,
}
struct Active {
    id: SceneId,
    name: String,
    scene: Box<dyn Scene>,
    accumulator: Duration,
    fixed: u64,
    frame: u64,
}
impl Active {
    fn dispose(&mut self) -> Vec<SceneEvent> {
        let mut events = Vec::new();
        for (phase, result) in [
            ("stop", self.scene.stop()),
            ("destroy", self.scene.destroy()),
        ] {
            events.push(SceneEvent {
                id: self.id,
                name: self.name.clone(),
                phase,
                error: result.err(),
            });
        }
        events
    }
}
type Factory = Box<dyn FnMut(SceneId) -> Result<Box<dyn Scene>, String>>;
pub struct SceneManager {
    owner: u64,
    next: u64,
    factories: BTreeMap<String, Factory>,
    active: Option<Active>,
    pending: Option<(SceneId, String)>,
    events: VecDeque<SceneEvent>,
    dropped_events: u64,
    delta: Duration,
    max_steps: u32,
}
impl Default for SceneManager {
    fn default() -> Self {
        Self::with_timing(Duration::from_secs_f64(1. / 144.), 8).unwrap()
    }
}
impl SceneManager {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_timing(delta: Duration, max_steps: u32) -> Result<Self, String> {
        if delta.is_zero() || max_steps == 0 {
            return Err("invalid scene timing".into());
        }
        Ok(Self {
            owner: owner(),
            next: 0,
            factories: BTreeMap::new(),
            active: None,
            pending: None,
            events: VecDeque::new(),
            dropped_events: 0,
            delta,
            max_steps,
        })
    }
    pub fn register(
        &mut self,
        name: impl Into<String>,
        factory: impl FnMut(SceneId) -> Result<Box<dyn Scene>, String> + 'static,
    ) -> SceneResult {
        let name = name.into();
        if name.trim().is_empty() || self.factories.contains_key(&name) {
            return Err("empty or duplicate scene name".into());
        }
        self.factories.insert(name, Box::new(factory));
        Ok(())
    }
    pub fn active_id(&self) -> Option<SceneId> {
        self.active.as_ref().map(|a| a.id)
    }
    pub fn active_name(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.name.as_str())
    }
    pub fn active_as<T: Scene>(&self) -> Option<&T> {
        let scene = self.active.as_ref()?.scene.as_ref();
        (scene as &dyn Any).downcast_ref()
    }
    pub fn active_as_mut<T: Scene>(&mut self) -> Option<&mut T> {
        let scene = self.active.as_mut()?.scene.as_mut();
        (scene as &mut dyn Any).downcast_mut()
    }
    fn event(&mut self, e: SceneEvent) {
        if self.events.len() == 256 {
            self.events.pop_front();
            self.dropped_events = self.dropped_events.saturating_add(1);
        }
        self.events.push_back(e);
    }
    pub fn drain_events(&mut self) -> impl Iterator<Item = SceneEvent> + '_ {
        self.events.drain(..)
    }
    pub fn dropped_events(&self) -> u64 {
        self.dropped_events
    }
    /// Candidate preparation is transactional: a failed start leaves the old scene
    /// running. Candidate and old resources must use separate ownership scopes.
    pub fn switch_to(&mut self, name: &str) -> SceneResult {
        let factory = self.factories.get_mut(name).ok_or("unknown scene")?;
        self.next = self.next.checked_add(1).ok_or("scene IDs exhausted")?;
        let id = SceneId {
            owner: self.owner,
            generation: self.next,
        };
        let scene = match factory(id) {
            Ok(scene) => scene,
            Err(error) => {
                self.event(SceneEvent {
                    id,
                    name: name.into(),
                    phase: "create",
                    error: Some(error.clone()),
                });
                return Err(error);
            }
        };
        self.event(SceneEvent {
            id,
            name: name.into(),
            phase: "create",
            error: None,
        });
        let mut next = Active {
            id,
            name: name.into(),
            scene,
            accumulator: Duration::ZERO,
            fixed: 0,
            frame: 0,
        };
        if let Err(error) = next.scene.start(id) {
            self.event(SceneEvent {
                id,
                name: name.into(),
                phase: "start",
                error: Some(error.clone()),
            });
            for e in next.dispose() {
                self.event(e);
            }
            return Err(error);
        }
        self.event(SceneEvent {
            id,
            name: name.into(),
            phase: "start",
            error: None,
        });
        if let Some(mut old) = self.active.take() {
            for e in old.dispose() {
                self.event(e);
            }
        }
        self.active = Some(next);
        self.pending = None;
        Ok(())
    }
    /// Late commands from a previous scene (or another manager) are rejected.
    pub fn request_switch(&mut self, from: SceneId, target: impl Into<String>) -> SceneResult {
        if self.active_id() != Some(from) {
            return Err("stale scene request".into());
        }
        let target = target.into();
        if !self.factories.contains_key(&target) {
            return Err("unknown scene".into());
        }
        self.pending = Some((from, target));
        Ok(())
    }
    pub fn advance(&mut self, elapsed: Duration) -> Result<AdvanceReport, String> {
        let mut report = AdvanceReport::default();
        if let Some(active) = self.active.as_mut() {
            let total = active.accumulator.saturating_add(elapsed);
            let budget = self.delta.saturating_mul(self.max_steps);
            report.dropped_time = total.saturating_sub(budget);
            active.accumulator = total.min(budget);
        }
        while let Some(active) = self.active.as_mut() {
            let directive = {
                if active.accumulator < self.delta || report.fixed_steps >= self.max_steps {
                    break;
                }
                active.accumulator -= self.delta;
                let step = Step {
                    delta: self.delta,
                    index: active.fixed,
                };
                active.fixed += 1;
                report.fixed_steps += 1;
                active.scene.fixed_update(step)
            };
            match directive {
                Ok(Directive::Continue) => {}
                Ok(Directive::Switch(name)) => {
                    let id = self.active_id().unwrap();
                    self.request_switch(id, name)?;
                    break;
                }
                Ok(Directive::Stop) => {
                    self.shutdown()?;
                    break;
                }
                Err(error) => {
                    let _ = self.shutdown();
                    return Err(error);
                }
            }
        }
        if let Some((from, target)) = self.pending.take()
            && self.active_id() == Some(from)
        {
            self.switch_to(&target)?;
        }
        if let Some(active) = &mut self.active {
            let step = Step {
                delta: elapsed,
                index: active.frame,
            };
            active.frame += 1;
            if let Err(error) = active.scene.frame_update(step) {
                let _ = self.shutdown();
                return Err(error);
            }
        }
        Ok(report)
    }
    pub fn shutdown(&mut self) -> SceneResult {
        self.pending = None;
        let mut errors = Vec::new();
        if let Some(mut active) = self.active.take() {
            for e in active.dispose() {
                if let Some(error) = &e.error {
                    errors.push(format!("{}: {error}", e.phase));
                }
                self.event(e);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}
impl Drop for SceneManager {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
#[cfg(feature = "plugin")]
pub mod integration;
