//! Small plugin runtime with independent fixed-rate workers and no platform or game dependencies.
//!
//! The host owns the clock and calls [`App::advance`], or drives fixed and frame
//! stages explicitly. A plugin is compiled into the application; dynamic loading
//! is outside M1. See `docs/architecture.md` in the repository for the contract.

mod worker;
mod world;
pub use worker::{Worker, WorkerConfig, WorkerError, WorkerHooks, WorkerReport};
pub use world::World;

use std::error::Error;
use std::fmt;
use std::time::Duration;

pub type PluginResult = Result<(), String>;

/// All callbacks run in resolved dependency order, except shutdown (reverse).
/// An error terminates the app and triggers best-effort cleanup.
pub trait Plugin: 'static {
    fn name(&self) -> &'static str;

    /// Required plugins, also ordered before this plugin in every stage.
    fn dependencies(&self) -> &[&'static str] {
        &[]
    }

    /// Ordering constraints for plugins that may be absent.
    fn after(&self) -> &[&'static str] {
        &[]
    }

    fn before(&self) -> &[&'static str] {
        &[]
    }

    fn init(&mut self, _world: &mut World) -> PluginResult {
        Ok(())
    }

    fn fixed_update(&mut self, _world: &mut World, _step: Step) -> PluginResult {
        Ok(())
    }

    fn frame_update(&mut self, _world: &mut World, _step: Step) -> PluginResult {
        Ok(())
    }

    /// Must tolerate partial initialization: also called on the plugin whose
    /// init failed. Previously initialized dependencies remain alive until later.
    fn shutdown(&mut self, _world: &mut World) -> PluginResult {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub delta: Duration,
    /// Zero-based index within this stage.
    pub index: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Configuring,
    Running,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Init,
    Fixed,
    Frame,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginFailure {
    pub plugin: &'static str,
    pub phase: Phase,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    InvalidTiming,
    InvalidName,
    InvalidState(State),
    DuplicatePlugin(&'static str),
    MissingDependency {
        plugin: &'static str,
        dependency: &'static str,
    },
    DependencyCycle(Vec<&'static str>),
    Callback {
        failure: PluginFailure,
        cleanup_errors: Vec<PluginFailure>,
    },
    Shutdown(Vec<PluginFailure>),
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTiming => {
                write!(f, "fixed delta and maximum fixed steps must be positive")
            }
            Self::InvalidName => write!(f, "plugin name must not be empty"),
            Self::InvalidState(state) => write!(f, "operation unavailable in state {state:?}"),
            Self::DuplicatePlugin(name) => write!(f, "duplicate plugin: {name}"),
            Self::MissingDependency { plugin, dependency } => {
                write!(f, "plugin {plugin} requires {dependency}")
            }
            Self::DependencyCycle(names) => write!(f, "dependency/order cycle blocks: {names:?}"),
            Self::Callback {
                failure,
                cleanup_errors,
            } => write!(
                f,
                "{} {:?} failed: {}; cleanup errors: {cleanup_errors:?}",
                failure.plugin, failure.phase, failure.message,
            ),
            Self::Shutdown(errors) => write!(f, "plugin shutdown errors: {errors:?}"),
        }
    }
}

impl Error for CoreError {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameReport {
    pub fixed_steps: u32,
    /// Time discarded to bound catch-up work. The host can surface this metric.
    pub dropped_time: Duration,
    pub interpolation: f64,
}

pub struct App {
    world: World,
    plugins: Vec<Box<dyn Plugin>>,
    order: Vec<usize>,
    initialized: Vec<usize>,
    state: State,
    fixed_delta: Duration,
    max_fixed_steps: u32,
    accumulator: Duration,
    fixed_index: u64,
    frame_index: u64,
}

impl Default for App {
    fn default() -> Self {
        Self::with_timing(Duration::from_secs_f64(1.0 / 144.0), 8).expect("default timing is valid")
    }
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_timing(fixed_delta: Duration, max_fixed_steps: u32) -> Result<Self, CoreError> {
        if fixed_delta.is_zero() || max_fixed_steps == 0 {
            return Err(CoreError::InvalidTiming);
        }
        Ok(Self {
            world: World::new(),
            plugins: Vec::new(),
            order: Vec::new(),
            initialized: Vec::new(),
            state: State::Configuring,
            fixed_delta,
            max_fixed_steps,
            accumulator: Duration::ZERO,
            fixed_index: 0,
            frame_index: 0,
        })
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    pub fn fixed_delta(&self) -> Duration {
        self.fixed_delta
    }

    pub fn add_plugin<P: Plugin>(&mut self, plugin: P) -> Result<&mut Self, CoreError> {
        self.add_boxed_plugin(Box::new(plugin))
    }
    pub fn add_boxed_plugin(&mut self, plugin: Box<dyn Plugin>) -> Result<&mut Self, CoreError> {
        if self.state != State::Configuring {
            return Err(CoreError::InvalidState(self.state));
        }
        if plugin.name().trim().is_empty() {
            return Err(CoreError::InvalidName);
        }
        if self.plugins.iter().any(|item| item.name() == plugin.name()) {
            return Err(CoreError::DuplicatePlugin(plugin.name()));
        }
        self.plugins.push(plugin);
        Ok(self)
    }

    /// Empty until a successful dependency resolution at start.
    pub fn plugin_order(&self) -> Vec<&'static str> {
        self.order.iter().map(|&i| self.plugins[i].name()).collect()
    }

    pub fn start(&mut self) -> Result<(), CoreError> {
        if self.state != State::Configuring {
            return Err(CoreError::InvalidState(self.state));
        }
        // Validate the entire graph before invoking any plugin callbacks.
        self.order = self.resolve_order()?;
        for slot in 0..self.order.len() {
            let i = self.order[slot];
            self.initialized.push(i);
            if let Err(message) = self.plugins[i].init(&mut self.world) {
                return Err(self.fail(i, Phase::Init, message));
            }
        }
        self.state = State::Running;
        Ok(())
    }

    /// Run one fixed stage. Does not modify the accumulator used by advance.
    pub fn tick(&mut self) -> Result<(), CoreError> {
        self.run_stage(Phase::Fixed, self.fixed_delta)
    }

    /// Run one frame stage without advancing fixed time.
    pub fn frame(&mut self, delta: Duration) -> Result<(), CoreError> {
        self.run_stage(Phase::Frame, delta)
    }

    /// Host-driven bounded accumulator: fixed stages, then one frame stage.
    /// Excess elapsed time is dropped rather than creating an unbounded backlog.
    pub fn advance(&mut self, elapsed: Duration) -> Result<FrameReport, CoreError> {
        if self.state != State::Running {
            return Err(CoreError::InvalidState(self.state));
        }
        let total = self.accumulator.saturating_add(elapsed);
        let budget = self.fixed_delta.saturating_mul(self.max_fixed_steps);
        let dropped_time = total.saturating_sub(budget);
        self.accumulator = total.min(budget);
        let mut fixed_steps = 0;
        while self.accumulator >= self.fixed_delta && fixed_steps < self.max_fixed_steps {
            self.tick()?;
            self.accumulator -= self.fixed_delta;
            fixed_steps += 1;
        }
        self.frame(elapsed)?;
        Ok(FrameReport {
            fixed_steps,
            dropped_time,
            interpolation: self.accumulator.as_secs_f64() / self.fixed_delta.as_secs_f64(),
        })
    }

    /// Cleanup continues after errors. Repeated shutdown is a no-op.
    pub fn shutdown(&mut self) -> Result<(), CoreError> {
        if matches!(self.state, State::Stopped | State::Failed) {
            return Ok(());
        }
        let errors = self.cleanup();
        self.state = if errors.is_empty() {
            State::Stopped
        } else {
            State::Failed
        };
        if errors.is_empty() {
            Ok(())
        } else {
            Err(CoreError::Shutdown(errors))
        }
    }

    fn run_stage(&mut self, phase: Phase, delta: Duration) -> Result<(), CoreError> {
        if self.state != State::Running {
            return Err(CoreError::InvalidState(self.state));
        }
        let index = if phase == Phase::Fixed {
            self.fixed_index
        } else {
            self.frame_index
        };
        for slot in 0..self.order.len() {
            let i = self.order[slot];
            let step = Step { delta, index };
            let result = if phase == Phase::Fixed {
                self.plugins[i].fixed_update(&mut self.world, step)
            } else {
                self.plugins[i].frame_update(&mut self.world, step)
            };
            if let Err(message) = result {
                return Err(self.fail(i, phase, message));
            }
        }
        if phase == Phase::Fixed {
            self.fixed_index = self.fixed_index.saturating_add(1);
        } else {
            self.frame_index = self.frame_index.saturating_add(1);
        }
        Ok(())
    }

    fn fail(&mut self, i: usize, phase: Phase, message: String) -> CoreError {
        let failure = PluginFailure {
            plugin: self.plugins[i].name(),
            phase,
            message,
        };
        let cleanup_errors = self.cleanup();
        self.state = State::Failed;
        CoreError::Callback {
            failure,
            cleanup_errors,
        }
    }

    fn cleanup(&mut self) -> Vec<PluginFailure> {
        let mut errors = Vec::new();
        while let Some(i) = self.initialized.pop() {
            if let Err(message) = self.plugins[i].shutdown(&mut self.world) {
                errors.push(PluginFailure {
                    plugin: self.plugins[i].name(),
                    phase: Phase::Shutdown,
                    message,
                });
            }
        }
        errors
    }

    fn resolve_order(&self) -> Result<Vec<usize>, CoreError> {
        let n = self.plugins.len();
        let mut edges = vec![Vec::new(); n];
        let mut indegree = vec![0usize; n];
        let find = |name: &str| self.plugins.iter().position(|plugin| plugin.name() == name);
        let mut edge = |from: usize, to: usize| {
            if !edges[from].contains(&to) {
                edges[from].push(to);
                indegree[to] += 1;
            }
        };
        for (i, plugin) in self.plugins.iter().enumerate() {
            for &dependency in plugin.dependencies() {
                let from = find(dependency).ok_or(CoreError::MissingDependency {
                    plugin: plugin.name(),
                    dependency,
                })?;
                edge(from, i);
            }
            for &name in plugin.after() {
                if let Some(from) = find(name) {
                    edge(from, i);
                }
            }
            for &name in plugin.before() {
                if let Some(to) = find(name) {
                    edge(i, to);
                }
            }
        }
        let mut order = Vec::with_capacity(n);
        let mut done = vec![false; n];
        while order.len() < n {
            // Registration order is the stable tie-break for unconstrained nodes.
            let Some(i) = (0..n).find(|&i| !done[i] && indegree[i] == 0) else {
                return Err(CoreError::DependencyCycle(
                    (0..n)
                        .filter(|&i| !done[i])
                        .map(|i| self.plugins[i].name())
                        .collect(),
                ));
            };
            done[i] = true;
            order.push(i);
            for &to in &edges[i] {
                indegree[to] -= 1;
            }
        }
        Ok(order)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Explicit shutdown reports failures; Drop is only a best-effort fallback.
        let _ = self.cleanup();
    }
}
