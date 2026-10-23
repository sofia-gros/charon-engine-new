//! Fixed-rate execution of a simulation plugin, independent of rendering and sand.

use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, SyncSender, TrySendError},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// A compile-time plugin owns its state; consumers only see completed frames.
pub trait Simulation: Send + 'static {
    type Command: Send;
    type Frame: Send + Sync;
    fn step(&mut self, commands: Vec<Self::Command>) -> Self::Frame;
}

pub struct Snapshot<F> {
    pub tick: u64,
    pub work_ms: f64,
    pub late_ms: f64,
    pub frame: F,
    /// Wall-clock publication time for bounded presentation interpolation.
    pub published_at: Instant,
}

type FrameMailbox<F> = Arc<Mutex<Option<Arc<Snapshot<F>>>>>;

struct PendingInput<C> {
    sequence: u64,
    states: BTreeMap<u8, (u64, C)>,
    controls: Vec<(u64, C)>,
}

/// A bounded command queue and a latest-completed-frame mailbox.
/// Dropping the runtime stops and joins its simulation thread.
pub struct Runtime<S: Simulation> {
    sender: SyncSender<(u64, S::Command)>,
    pending: Arc<Mutex<PendingInput<S::Command>>>,
    latest: FrameMailbox<S::Frame>,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl<S: Simulation> Runtime<S> {
    pub fn start(mut simulation: S) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(128);
        let pending = Arc::new(Mutex::new(PendingInput {
            sequence: 0,
            states: BTreeMap::new(),
            controls: Vec::new(),
        }));
        let inputs = pending.clone();
        let latest = Arc::new(Mutex::new(None));
        let running = Arc::new(AtomicBool::new(true));
        let output = latest.clone();
        let alive = running.clone();
        let thread = thread::Builder::new()
            .name("charon-simulation".into())
            .spawn(move || {
                let epoch = Instant::now();
                let mut tick = 0_u64;
                while alive.load(Ordering::Acquire) {
                    // Absolute deadlines avoid accumulating sleep/rounding drift.
                    let deadline = epoch + Duration::from_secs_f64(tick as f64 / 60.0);
                    let now = Instant::now();
                    if now < deadline {
                        thread::park_timeout(deadline - now);
                        continue;
                    }
                    let start = Instant::now();
                    // Drain both input paths at one boundary. Sequence numbers
                    // retain ordering between accepted events and latest states.
                    // No input lock is held while the simulation does its work.
                    let mut commands: Vec<_> = {
                        let mut pending = inputs.lock().expect("input lock poisoned");
                        if !alive.load(Ordering::Acquire) {
                            break;
                        }
                        receiver
                            .try_iter()
                            .take(128)
                            .chain(std::mem::take(&mut pending.states).into_values())
                            .chain(std::mem::take(&mut pending.controls))
                            .collect()
                    };
                    commands.sort_unstable_by_key(|(sequence, _)| *sequence);
                    let frame = simulation.step(commands.into_iter().map(|(_, c)| c).collect());
                    let snapshot = Arc::new(Snapshot {
                        tick: tick + 1,
                        work_ms: start.elapsed().as_secs_f64() * 1000.0,
                        late_ms: start.saturating_duration_since(deadline).as_secs_f64() * 1000.0,
                        frame,
                        published_at: Instant::now(),
                    });
                    *output.lock().expect("snapshot lock poisoned") = Some(snapshot);
                    tick += 1;
                    // No ticks/accepted events are discarded; held states coalesce.
                    // Yield when behind rather than
                    // starving other threads; lateness remains visible to the client.
                    if start.elapsed() > Duration::from_secs_f64(1.0 / 60.0) {
                        thread::yield_now();
                    }
                }
            })?;
        Ok(Self {
            sender,
            pending,
            latest,
            running,
            thread: Some(thread),
        })
    }

    pub fn submit(&self, command: S::Command) -> Result<(), TrySendError<S::Command>> {
        let mut pending = self.pending.lock().expect("input lock poisoned");
        if !self.running.load(Ordering::Acquire) || self.is_finished() {
            return Err(TrySendError::Disconnected(command));
        }
        let sequence = pending.sequence;
        pending.sequence += 1;
        self.sender
            .try_send((sequence, command))
            .map_err(|error| match error {
                TrySendError::Full((_, c)) => TrySendError::Full(c),
                TrySendError::Disconnected((_, c)) => TrySendError::Disconnected(c),
            })
    }

    /// Reserved queue for infrequent controls such as pause and single-step.
    /// Preserves every accepted event and arrival order across all input paths.
    /// Independent of the 128 ordinary events; bounded to 32 controls per drain.
    /// Full returns ownership to the caller for explicit handling, never coalesces.
    /// Commands apply only after the current simulation step returns.
    pub fn submit_control(&self, command: S::Command) -> Result<(), TrySendError<S::Command>> {
        let mut pending = self.pending.lock().expect("input lock poisoned");
        if !self.running.load(Ordering::Acquire) || self.is_finished() {
            return Err(TrySendError::Disconnected(command));
        }
        if pending.controls.len() == 32 {
            return Err(TrySendError::Full(command));
        }
        let sequence = pending.sequence;
        pending.sequence += 1;
        pending.controls.push((sequence, command));
        Ok(())
    }

    /// Latest value for one held-input slot, independent of the event queue.
    /// Up to 256 slots are bounded by u8; newer values replace unconsumed ones.
    /// Use stable, distinct slots for held states, never for one-shot actions.
    /// Intermediate values in one slot may be coalesced; accepted events retain
    /// FIFO order relative to the remaining states. No simulation work is locked.
    pub fn submit_state(
        &self,
        slot: u8,
        command: S::Command,
    ) -> Result<(), TrySendError<S::Command>> {
        let mut pending = self.pending.lock().expect("input lock poisoned");
        if !self.running.load(Ordering::Acquire) || self.is_finished() {
            return Err(TrySendError::Disconnected(command));
        }
        let sequence = pending.sequence;
        pending.sequence += 1;
        pending.states.insert(slot, (sequence, command));
        Ok(())
    }

    /// Request shutdown without waiting for the in-flight step. No new inputs are
    /// accepted. Queued inputs are abandoned at shutdown; the current step may
    /// finish and publish. Drop still joins, so plugin state is never detached.
    pub fn request_stop(&self) {
        let _pending = self.pending.lock().expect("input lock poisoned");
        self.running.store(false, Ordering::Release);
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }

    pub fn latest(&self) -> Option<Arc<Snapshot<S::Frame>>> {
        self.latest.lock().expect("snapshot lock poisoned").clone()
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl<S: Simulation> Drop for Runtime<S> {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

/// Interpolate completed states with one tick of presentation delay. Never
/// extrapolate when rendering or simulation stalls. Does not mutate game state.
pub fn interpolate<const N: usize>(
    previous: [f32; N],
    current: [f32; N],
    elapsed: Duration,
    period: Duration,
) -> [f32; N] {
    let alpha = if period.is_zero() {
        1.
    } else {
        (elapsed.as_secs_f32() / period.as_secs_f32()).min(1.)
    };
    std::array::from_fn(|i| previous[i] + (current[i] - previous[i]) * alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_is_bounded_without_changing_authoritative_positions() {
        let before = [2., 4.];
        let after = [8., 10.];
        let period = Duration::from_millis(16);
        assert_eq!(interpolate(before, after, Duration::ZERO, period), before);
        assert_eq!(
            interpolate(before, after, Duration::from_millis(8), period),
            [5., 7.]
        );
        assert_eq!(
            interpolate(before, after, Duration::from_secs(2), period),
            after
        );
        assert_eq!(
            interpolate(before, after, Duration::ZERO, Duration::ZERO),
            after
        );
        assert_eq!(after, [8., 10.]);
    }
    struct Counter(u64);
    impl Simulation for Counter {
        type Command = u64;
        type Frame = u64;
        fn step(&mut self, commands: Vec<u64>) -> u64 {
            self.0 += commands.into_iter().sum::<u64>();
            self.0
        }
    }

    #[test]
    fn plugin_runs_without_sand_and_publishes_immutable_frames() {
        let runtime = Runtime::start(Counter(0)).unwrap();
        runtime.submit(7).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let first = loop {
            if let Some(frame) = runtime.latest() {
                if frame.frame == 7 {
                    break frame;
                }
            }
            assert!(
                Instant::now() < deadline,
                "simulation did not consume input"
            );
            thread::sleep(Duration::from_millis(2));
        };
        runtime.submit(5).unwrap();
        loop {
            if runtime.latest().is_some_and(|frame| frame.frame == 12) {
                break;
            }
            assert!(Instant::now() < deadline, "second input not consumed");
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(first.frame, 7, "published snapshot was mutated");
        assert!(first.tick < runtime.latest().unwrap().tick);
    }

    #[test]
    fn latest_held_states_stay_bounded_and_ordered_with_a_saturated_event_queue() {
        struct Recorder {
            commands: Vec<u32>,
            gate: Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>,
        }
        impl Simulation for Recorder {
            type Command = u32;
            type Frame = Vec<u32>;
            fn step(&mut self, commands: Vec<u32>) -> Vec<u32> {
                if let Some((entered, resume)) = self.gate.take() {
                    entered.send(()).unwrap();
                    resume.recv_timeout(Duration::from_secs(2)).unwrap();
                }
                self.commands.extend(commands);
                self.commands.clone()
            }
        }
        let (entered_tx, entered) = mpsc::sync_channel(1);
        let (resume, resume_rx) = mpsc::sync_channel(1);
        let runtime = Runtime::start(Recorder {
            commands: vec![],
            gate: Some((entered_tx, resume_rx)),
        })
        .unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        runtime.submit_state(1, 999).unwrap();
        runtime.submit(0).unwrap();
        runtime.submit_state(2, 1000).unwrap();
        for control in 12000..12032 {
            runtime.submit_control(control).unwrap();
        }
        assert!(matches!(
            runtime.submit_control(12032),
            Err(TrySendError::Full(12032))
        ));
        for event in 1..128 {
            runtime.submit(event).unwrap();
        }
        assert!(matches!(runtime.submit(128), Err(TrySendError::Full(128))));
        for value in 2000..12000 {
            runtime.submit_state(1, value).unwrap();
        }
        resume.send(()).unwrap();
        let expected: Vec<_> = [0, 1000]
            .into_iter()
            .chain(12000..12032)
            .chain(1..128)
            .chain([11999])
            .collect();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(snapshot) = runtime.latest() {
                if snapshot.frame.len() >= expected.len() {
                    assert_eq!(snapshot.frame, expected);
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "accepted inputs were not consumed"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn stop_bypasses_full_queues_and_joins_only_the_in_flight_step() {
        struct Gated {
            entered: mpsc::SyncSender<()>,
            resume: mpsc::Receiver<()>,
            calls: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl Simulation for Gated {
            type Command = u32;
            type Frame = ();
            fn step(&mut self, _: Vec<u32>) {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.entered.send(()).unwrap();
                self.resume.recv_timeout(Duration::from_secs(2)).unwrap();
            }
        }
        let (entered_tx, entered) = mpsc::sync_channel(1);
        let (resume, resume_rx) = mpsc::sync_channel(1);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let runtime = Runtime::start(Gated {
            entered: entered_tx,
            resume: resume_rx,
            calls: calls.clone(),
        })
        .unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        for i in 0..128 {
            runtime.submit(i).unwrap();
        }
        for i in 0..32 {
            runtime.submit_control(i).unwrap();
        }
        runtime.request_stop();
        assert!(
            !runtime.is_finished(),
            "stop must not detach or pretend the step finished"
        );
        assert!(matches!(
            runtime.submit(999),
            Err(TrySendError::Disconnected(999))
        ));
        assert!(matches!(
            runtime.submit_state(0, 999),
            Err(TrySendError::Disconnected(999))
        ));
        assert!(matches!(
            runtime.submit_control(999),
            Err(TrySendError::Disconnected(999))
        ));
        let (done_tx, done) = mpsc::sync_channel(1);
        let join = thread::spawn(move || {
            drop(runtime);
            done_tx.send(()).unwrap();
        });
        assert!(matches!(
            done.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        resume.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(2)).unwrap();
        join.join().unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "shutdown drained more work"
        );
    }

    #[test]
    fn finite_stall_recovers_without_skipping_simulation_ticks() {
        struct Gated {
            entered: mpsc::SyncSender<()>,
            resume: mpsc::Receiver<()>,
            calls: u64,
        }
        impl Simulation for Gated {
            type Command = u64;
            type Frame = u64;
            fn step(&mut self, _: Vec<u64>) -> u64 {
                self.calls += 1;
                if self.calls <= 3 {
                    self.entered.send(()).unwrap();
                    self.resume.recv_timeout(Duration::from_secs(2)).unwrap();
                }
                self.calls
            }
        }
        let (entered_tx, entered) = mpsc::sync_channel(1);
        let (resume, resume_rx) = mpsc::sync_channel(1);
        let runtime = Runtime::start(Gated {
            entered: entered_tx,
            resume: resume_rx,
            calls: 0,
        })
        .unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        // Deliberately stall longer than three fixed periods, once.
        assert!(matches!(
            entered.recv_timeout(Duration::from_millis(60)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        resume.send(()).unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let first = runtime.latest().unwrap();
        assert_eq!((first.tick, first.frame), (1, 1));
        assert!(first.work_ms >= 50.);
        resume.send(()).unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let late = runtime.latest().unwrap();
        assert_eq!((late.tick, late.frame), (2, 2));
        assert!(late.late_ms >= 30., "overload must remain visible");
        resume.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(snapshot) = runtime.latest() {
                assert_eq!(snapshot.tick, snapshot.frame, "physics ticks were skipped");
                if snapshot.tick >= 8 && snapshot.late_ms < 20. {
                    break;
                }
            }
            assert!(Instant::now() < deadline, "finite backlog did not recover");
            thread::sleep(Duration::from_millis(1));
        }
    }
}
