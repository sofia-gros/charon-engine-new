//! Independent fixed-rate workers. Each worker owns its App and World; mutable
//! game state is never shared between callbacks running on different threads.
use crate::{App, CoreError, PluginResult, World};
use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub name: String,
    pub hz: u32,
    pub max_catch_up: u32,
}

impl WorkerConfig {
    pub fn new(name: impl Into<String>, hz: u32) -> Self {
        Self {
            name: name.into(),
            hz,
            max_catch_up: 8,
        }
    }
}

/// Hooks transfer commands/snapshots across a bounded channel or a short lock.
/// Hooks must not wait indefinitely: shutdown joins the owning worker.
pub trait WorkerHooks: Send + 'static {
    fn before_tick(&mut self, _world: &mut World) -> PluginResult {
        Ok(())
    }
    fn after_tick(&mut self, _world: &mut World) -> PluginResult {
        Ok(())
    }
}
impl WorkerHooks for () {}

#[derive(Debug, Clone, Copy, Default)]
pub struct WorkerReport {
    pub ticks: u64,
    pub dropped_time: Duration,
}

#[derive(Debug, Clone)]
pub enum WorkerError {
    InvalidConfig,
    Spawn(String),
    Core(CoreError),
    Hook {
        message: String,
        cleanup: Option<CoreError>,
    },
    Panicked,
}
impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => write!(
                f,
                "worker rate and catch-up budget must be positive and match the App timing"
            ),
            Self::Spawn(message) => write!(f, "worker could not start: {message}"),
            Self::Core(error) => write!(f, "worker core: {error}"),
            Self::Hook { message, cleanup } => {
                write!(f, "worker transfer failed: {message}; cleanup: {cleanup:?}")
            }
            Self::Panicked => write!(f, "worker panicked"),
        }
    }
}
impl std::error::Error for WorkerError {}
impl From<CoreError> for WorkerError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

pub struct Worker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<WorkerReport, WorkerError>>>,
    result: Option<Result<WorkerReport, WorkerError>>,
}

impl Worker {
    /// Build and initialize the App inside its owner thread. The factory receives
    /// the lane's fixed delta and must return an unstarted App with that timing.
    /// Thread-bound resources may be created inside the factory.
    pub fn spawn<F, H>(config: WorkerConfig, factory: F, mut hooks: H) -> Result<Self, WorkerError>
    where
        F: FnOnce(Duration) -> Result<App, CoreError> + Send + 'static,
        H: WorkerHooks,
    {
        if config.hz == 0 || config.max_catch_up == 0 {
            return Err(WorkerError::InvalidConfig);
        }
        let delta = Duration::from_secs_f64(1.0 / f64::from(config.hz));
        if delta.is_zero() {
            return Err(WorkerError::InvalidConfig);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let handle = thread::Builder::new()
            .name(config.name)
            .spawn(move || {
                let setup = (|| {
                    let mut app = factory(delta)?;
                    if app.fixed_delta() != delta {
                        return Err(WorkerError::InvalidConfig);
                    }
                    app.start()?;
                    Ok(app)
                })();
                let mut app = match setup {
                    Ok(app) => {
                        let _ = ready_tx.send(Ok(()));
                        app
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.clone()));
                        return Err(error);
                    }
                };
                let mut report = WorkerReport::default();
                let mut deadline = Instant::now();
                let budget = delta.saturating_mul(config.max_catch_up);
                while !signal.load(Ordering::Acquire) {
                    let now = Instant::now();
                    if now < deadline {
                        thread::park_timeout(deadline.duration_since(now));
                        continue;
                    }
                    let lag = now.duration_since(deadline);
                    if lag > budget {
                        report.dropped_time = report.dropped_time.saturating_add(lag);
                        deadline = now;
                    }
                    if let Err(message) = hooks.before_tick(app.world_mut()) {
                        return Err(WorkerError::Hook {
                            message,
                            cleanup: app.shutdown().err(),
                        });
                    }
                    app.tick()?;
                    if let Err(message) = hooks.after_tick(app.world_mut()) {
                        return Err(WorkerError::Hook {
                            message,
                            cleanup: app.shutdown().err(),
                        });
                    }
                    report.ticks += 1;
                    deadline += delta;
                }
                app.shutdown()?;
                Ok(report)
            })
            .map_err(|error| WorkerError::Spawn(error.to_string()))?;
        // Return only after startup validation/callbacks have succeeded.
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                handle: Some(handle),
                result: None,
            }),
            Ok(Err(error)) => {
                let _ = handle.join();
                Err(error)
            }
            Err(_) => {
                let _ = handle.join();
                Err(WorkerError::Panicked)
            }
        }
    }

    pub fn is_finished(&self) -> bool {
        self.handle.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = &self.handle {
            handle.thread().unpark();
        }
    }

    /// Request stop, wake the clock, and join. The same result is returned on
    /// repeated calls; callback cleanup is never repeated.
    pub fn join(&mut self) -> Result<WorkerReport, WorkerError> {
        self.stop();
        if let Some(handle) = self.handle.take() {
            self.result = Some(handle.join().unwrap_or(Err(WorkerError::Panicked)));
        }
        self.result
            .clone()
            .expect("worker always has a handle or a result")
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.join();
    }
}
