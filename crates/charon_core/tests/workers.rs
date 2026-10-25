use charon_core::{
    App, CoreError, Plugin, PluginResult, Step, Worker, WorkerConfig, WorkerError, WorkerHooks,
    World,
};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, ThreadId};
use std::time::Duration;

type Record = (ThreadId, Duration);
struct Clock(mpsc::Sender<Record>);
impl Plugin for Clock {
    fn name(&self) -> &'static str {
        "clock"
    }
    fn fixed_update(&mut self, _: &mut World, step: Step) -> PluginResult {
        self.0
            .send((thread::current().id(), step.delta))
            .map_err(|e| e.to_string())
    }
}

#[test]
fn independent_workers_own_state_and_run_at_separate_fixed_deltas() {
    let host = thread::current().id();
    let (game_tx, game_rx) = mpsc::channel();
    let (compute_tx, compute_rx) = mpsc::channel();
    let mut game = Worker::spawn(
        WorkerConfig::new("test-game", 144),
        move |delta| {
            let mut app = App::with_timing(delta, 8)?;
            // App resources can be thread-bound: created inside the worker factory.
            app.world_mut()
                .insert(std::rc::Rc::new(std::cell::Cell::new(0)));
            app.add_plugin(Clock(game_tx))?;
            Ok(app)
        },
        (),
    )
    .unwrap();
    let mut compute = Worker::spawn(
        WorkerConfig::new("test-compute", 30),
        move |delta| {
            let mut app = App::with_timing(delta, 8)?;
            app.add_plugin(Clock(compute_tx))?;
            Ok(app)
        },
        (),
    )
    .unwrap();
    let (game_id, game_delta) = game_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let (compute_id, compute_delta) = compute_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_ne!(game_id, host);
    assert_ne!(compute_id, host);
    assert_ne!(game_id, compute_id);
    assert_eq!(game_delta, Duration::from_secs_f64(1.0 / 144.0));
    assert_eq!(compute_delta, Duration::from_secs_f64(1.0 / 30.0));
    assert_eq!(
        game_rx.recv_timeout(Duration::from_secs(3)).unwrap().0,
        game_id
    );
    game.stop();
    compute.stop();
    let game_report = game.join().unwrap();
    assert!(game_report.ticks >= 2);
    assert!(compute.join().unwrap().ticks >= 1);
    assert_eq!(game.join().unwrap().ticks, game_report.ticks);
}

struct Cleanup(Arc<Mutex<Vec<ThreadId>>>);
impl Plugin for Cleanup {
    fn name(&self) -> &'static str {
        "cleanup"
    }
    fn shutdown(&mut self, _: &mut World) -> PluginResult {
        self.0.lock().unwrap().push(thread::current().id());
        Ok(())
    }
}
struct BrokenTransfer;
impl WorkerHooks for BrokenTransfer {
    fn after_tick(&mut self, _: &mut World) -> PluginResult {
        Err("transfer failed".into())
    }
}

#[test]
fn transfer_failure_is_reported_and_cleans_up_on_owner_thread() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let cleanup = log.clone();
    let mut worker = Worker::spawn(
        WorkerConfig::new("broken", 60),
        move |delta| {
            let mut app = App::with_timing(delta, 8)?;
            app.add_plugin(Cleanup(cleanup))?;
            Ok(app)
        },
        BrokenTransfer,
    )
    .unwrap();
    // Joining also requests stop. Wait for the failed tick via a cleanup signal.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !worker.is_finished() {
        assert!(std::time::Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        matches!(worker.join(), Err(WorkerError::Hook { message, cleanup: None }) if message == "transfer failed")
    );
    assert_eq!(log.lock().unwrap().len(), 1);
    assert_ne!(log.lock().unwrap()[0], thread::current().id());
}

#[test]
fn startup_errors_and_invalid_clock_are_reported_before_returning() {
    assert!(matches!(
        Worker::spawn(WorkerConfig::new("zero", 0), |_| Ok(App::new()), ()),
        Err(WorkerError::InvalidConfig)
    ));
    assert!(matches!(
        Worker::spawn(
            WorkerConfig::new("too-fast", u32::MAX),
            |_| Ok(App::new()),
            ()
        ),
        Err(WorkerError::InvalidConfig)
    ));
    assert!(matches!(
        Worker::spawn(
            WorkerConfig::new("factory", 30),
            |_| Err(CoreError::InvalidName),
            ()
        ),
        Err(WorkerError::Core(CoreError::InvalidName))
    ));
    assert!(matches!(
        Worker::spawn(WorkerConfig::new("mismatch", 30), |_| Ok(App::new()), ()),
        Err(WorkerError::InvalidConfig)
    ));
}

#[test]
fn drop_requests_stop_and_joins_before_resources_go_away() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let cleanup = log.clone();
    {
        let _worker = Worker::spawn(
            WorkerConfig::new("drop", 1),
            move |delta| {
                let mut app = App::with_timing(delta, 8)?;
                app.add_plugin(Cleanup(cleanup))?;
                Ok(app)
            },
            (),
        )
        .unwrap();
    }
    assert_eq!(log.lock().unwrap().len(), 1);
}
