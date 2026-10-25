//! Recommended sample host: game and compute each own an App on a worker;
//! window/frame rendering stays on the main thread. Exchange only small snapshots.
use crate::game::{
    Game, GamePlugin, Glow, Input, PulsePlugin, Score, ScorePlugin, Trace, required,
};
use charon_core::{App, Plugin, PluginResult, Step, Worker, WorkerConfig, WorkerHooks, World};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
struct GameSnapshot {
    game: Game,
    score: u32,
    steps: u64,
}
#[derive(Clone, Copy, Default)]
struct ComputeSnapshot {
    glow: Option<Glow>,
    steps: u64,
}
#[derive(Clone, Default)]
struct Exchange {
    input: Arc<Mutex<Input>>,
    game: Arc<Mutex<Option<GameSnapshot>>>,
    compute: Arc<Mutex<ComputeSnapshot>>,
}

#[derive(Default)]
pub struct Metrics {
    pub hz: u32,
    pub steps: u64,
    pub game_steps: u64,
}

struct GameTransfer {
    exchange: Exchange,
    steps: u64,
}
impl WorkerHooks for GameTransfer {
    fn before_tick(&mut self, world: &mut World) -> PluginResult {
        let mut incoming = self
            .exchange
            .input
            .lock()
            .map_err(|_| "input lock poisoned")?;
        let local = world.get_mut::<Input>().ok_or("input missing")?;
        local.dx = incoming.dx;
        local.dy = incoming.dy;
        local.reset |= std::mem::take(&mut incoming.reset);
        local.pause |= std::mem::take(&mut incoming.pause);
        Ok(())
    }
    fn after_tick(&mut self, world: &mut World) -> PluginResult {
        self.steps += 1;
        let snapshot = GameSnapshot {
            game: required::<Game>(world)?.clone(),
            score: required::<Score>(world)?.0,
            steps: self.steps,
        };
        *self
            .exchange
            .game
            .lock()
            .map_err(|_| "game snapshot lock poisoned")? = Some(snapshot);
        Ok(())
    }
}

struct ComputeTransfer {
    exchange: Exchange,
    steps: u64,
}
impl WorkerHooks for ComputeTransfer {
    fn after_tick(&mut self, world: &mut World) -> PluginResult {
        self.steps += 1;
        *self
            .exchange
            .compute
            .lock()
            .map_err(|_| "compute snapshot lock poisoned")? = ComputeSnapshot {
            glow: world.get::<Glow>().copied(),
            steps: self.steps,
        };
        Ok(())
    }
}

// M1 has no physics plugin yet. This plugin makes the independent compute clock
// observable even when the optional pulse calculation is omitted.
struct ComputeClock;
impl Plugin for ComputeClock {
    fn name(&self) -> &'static str {
        "compute-clock"
    }
}

struct SnapshotPlugin {
    exchange: Exchange,
    hz: u32,
}
impl Plugin for SnapshotPlugin {
    fn name(&self) -> &'static str {
        "snapshot"
    }
    fn init(&mut self, world: &mut World) -> PluginResult {
        world.insert(Game::default());
        world.insert(Score::default());
        world.insert(Metrics {
            hz: self.hz,
            ..Metrics::default()
        });
        Ok(())
    }
    fn frame_update(&mut self, world: &mut World, _: Step) -> PluginResult {
        let game = self
            .exchange
            .game
            .lock()
            .map_err(|_| "game snapshot lock poisoned")?
            .clone();
        let compute = *self
            .exchange
            .compute
            .lock()
            .map_err(|_| "compute snapshot lock poisoned")?;
        let mut game_steps = 0;
        if let Some(snapshot) = game {
            game_steps = snapshot.steps;
            world.insert(snapshot.game);
            world.insert(Score(snapshot.score));
        }
        if let Some(glow) = compute.glow {
            world.insert(glow);
        } else {
            world.remove::<Glow>();
        }
        world.insert(Metrics {
            hz: self.hz,
            steps: compute.steps,
            game_steps,
        });
        Ok(())
    }
    fn shutdown(&mut self, world: &mut World) -> PluginResult {
        world.remove::<Game>();
        world.remove::<Score>();
        world.remove::<Glow>();
        world.remove::<Metrics>();
        Ok(())
    }
}

pub struct ThreadedGame {
    pub render: App,
    exchange: Exchange,
    game: Worker,
    compute: Worker,
}

impl ThreadedGame {
    pub fn start(effects: bool, compute_hz: u32) -> Result<Self, Box<dyn std::error::Error>> {
        if !(30..=60).contains(&compute_hz) {
            return Err("compute rate must be 30..=60 Hz".into());
        }
        let exchange = Exchange::default();
        let game = Worker::spawn(
            WorkerConfig::new("charon-game", 144),
            |delta| {
                let mut app = App::with_timing(delta, 8)?;
                app.world_mut().insert(Trace::default());
                app.add_plugin(ScorePlugin)?;
                app.add_plugin(GamePlugin)?;
                Ok(app)
            },
            GameTransfer {
                exchange: exchange.clone(),
                steps: 0,
            },
        )?;
        let compute = Worker::spawn(
            WorkerConfig::new("charon-compute", compute_hz),
            move |delta| {
                let mut app = App::with_timing(delta, 8)?;
                app.world_mut().insert(Trace::default());
                app.add_plugin(ComputeClock)?;
                if effects {
                    app.add_plugin(PulsePlugin)?;
                }
                Ok(app)
            },
            ComputeTransfer {
                exchange: exchange.clone(),
                steps: 0,
            },
        )?;
        let mut render = App::new();
        render.world_mut().insert(Trace::default());
        render.add_plugin(crate::view::ViewPlugin::snapshot())?;
        render.add_plugin(SnapshotPlugin {
            exchange: exchange.clone(),
            hz: compute_hz,
        })?;
        render.start()?;
        Ok(Self {
            render,
            exchange,
            game,
            compute,
        })
    }

    pub fn set_input(&self, input: Input) -> PluginResult {
        let mut shared = self
            .exchange
            .input
            .lock()
            .map_err(|_| "input lock poisoned")?;
        shared.dx = input.dx;
        shared.dy = input.dy;
        shared.reset |= input.reset;
        shared.pause |= input.pause;
        Ok(())
    }

    pub fn frame(&mut self, elapsed: Duration) -> Result<(), Box<dyn std::error::Error>> {
        if self.game.is_finished() || self.compute.is_finished() {
            self.game.stop();
            self.compute.stop();
            self.game.join()?;
            self.compute.join()?;
            return Err("a worker stopped unexpectedly".into());
        }
        self.render.frame(elapsed)?;
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.game.stop();
        self.compute.stop();
        let game = self.game.join();
        let compute = self.compute.join();
        let render = self.render.shutdown();
        println!("Game worker: {game:?}; compute worker: {compute:?}");
        game?;
        compute?;
        render?;
        Ok(())
    }
}

pub fn smoke_test() -> Result<(), Box<dyn std::error::Error>> {
    for (effects, hz) in [(false, 30), (true, 45), (true, 60)] {
        let mut game = ThreadedGame::start(effects, hz)?;
        game.set_input(Input {
            dx: 1,
            ..Input::default()
        })?;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            game.frame(Duration::from_secs_f64(1.0 / 60.0))?;
            let metrics = required::<Metrics>(game.render.world())?;
            if metrics.game_steps >= 4 && metrics.steps >= 2 {
                break;
            }
            if std::time::Instant::now() > deadline {
                return Err("workers did not publish snapshots".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if required::<Game>(game.render.world())?.player == (2, 2) {
            return Err("input did not reach the game worker".into());
        }
        game.set_input(Input::default())?;
        if game.render.world().get::<Glow>().is_some() != effects {
            return Err("optional compute plugin mismatch".into());
        }
        game.shutdown()?;
    }
    println!(
        "Threaded smoke test passed: game 144 Hz, render independently driven, compute 30/45/60 Hz."
    );
    Ok(())
}
