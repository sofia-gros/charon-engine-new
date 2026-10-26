//! A caller-selected lighting clock. All CPU preparation and GPU encoding run on
//! the owner thread. At most one GPU lighting submission is in flight.
use super::*;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
enum Command {
    Readback(mpsc::SyncSender<Result<LightField, String>>),
}
pub struct TracingWorker {
    texture: wgpu::Texture,
    geometry: Geometry,
    samples: Arc<AtomicU32>,
    enabled: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    tx: mpsc::SyncSender<Command>,
    handle: Option<JoinHandle<()>>,
}
impl TracingWorker {
    pub fn spawn(
        mut runtime: Tracing,
        hz: u32,
        mut source: impl FnMut() -> Result<Option<TraceScene>, String> + Send + 'static,
    ) -> Result<Self, String> {
        if !(1..=240).contains(&hz) {
            return Err("lighting worker clock must be 1..240Hz".into());
        }
        let texture = runtime.tracer.texture().clone();
        let geometry = runtime.tracer.readback_geometry();
        let samples = Arc::new(AtomicU32::new(runtime.tracer.samples()));
        let enabled = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::sync_channel(4);
        let count = samples.clone();
        let active = enabled.clone();
        let stopping = stop.clone();
        let failure = error.clone();
        let handle = thread::Builder::new()
            .name("charon-lighting".into())
            .spawn(move || {
                let result = (|| -> Result<(), String> {
                    let interval = Duration::from_secs_f64(1. / hz as f64);
                    let mut deadline = Instant::now();
                    let busy = Arc::new(AtomicBool::new(false));
                    while !stopping.load(Ordering::Acquire) {
                        let now = Instant::now();
                        if now < deadline {
                            thread::park_timeout(deadline - now);
                            continue;
                        }
                        deadline = now + interval;
                        runtime
                            .tracer
                            .device
                            .poll(wgpu::PollType::Poll)
                            .map_err(|e| e.to_string())?;
                        while let Ok(command) = rx.try_recv() {
                            match command {
                                Command::Readback(reply) => {
                                    let _ = reply.send(runtime.tracer.readback());
                                }
                            }
                        }
                        if busy.load(Ordering::Acquire) || !active.load(Ordering::Acquire) {
                            continue;
                        }
                        count.store(runtime.tracer.samples(), Ordering::Release);
                        if let Some(input) = source()? {
                            input.validate()?;
                            if input.input.geometry != geometry {
                                return Err(
                                    "worker source changed geometry; prepare a new backend".into(),
                                );
                            }
                            runtime.input = Some(input);
                        }
                        if let Some(input) = &runtime.input
                            && runtime.tracer.render(input)?
                        {
                            busy.store(true, Ordering::Release);
                            let signal = busy.clone();
                            runtime.tracer.queue.on_submitted_work_done(move || {
                                signal.store(false, Ordering::Release)
                            });
                        }
                    }
                    runtime
                        .tracer
                        .device
                        .poll(wgpu::PollType::Wait)
                        .map_err(|e| e.to_string())?;
                    Ok(())
                })();
                if let Err(error) = result {
                    *failure.lock().unwrap() = Some(error);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            texture,
            geometry,
            samples,
            enabled,
            stop,
            error,
            tx,
            handle: Some(handle),
        })
    }
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }
    pub fn samples(&self) -> u32 {
        self.samples.load(Ordering::Acquire)
    }
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }
    pub fn check(&self) -> Result<(), String> {
        if let Some(error) = self.error.lock().unwrap().clone() {
            Err(error)
        } else if self.handle.as_ref().is_none_or(|h| h.is_finished()) {
            Err("lighting worker stopped".into())
        } else {
            Ok(())
        }
    }
    pub fn readback(&self) -> Result<LightField, String> {
        self.check()?;
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx
            .try_send(Command::Readback(tx))
            .map_err(|e| e.to_string())?;
        if let Some(h) = &self.handle {
            h.thread().unpark();
        }
        rx.recv_timeout(Duration::from_secs(30))
            .map_err(|e| e.to_string())?
    }
}
impl Drop for TracingWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            h.thread().unpark();
            let _ = h.join();
        }
    }
}
