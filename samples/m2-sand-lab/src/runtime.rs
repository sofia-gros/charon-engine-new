use charon_core::{App, Plugin, PluginResult, Step, Worker, WorkerConfig, World};
use charon_physics::sim::material::CellData;
use charon_sand::{FallingSandPlugin, Operation, Rect, Request, SandClient, Simulation};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc::TrySendError};

pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 200;
pub const GATE: Rect = Rect {
    x: 65,
    y: 105,
    w: 190,
    h: 1,
};
pub fn boundaries() -> [Operation; 4] {
    [
        Operation::Collider {
            rect: Rect {
                x: 0,
                y: 199,
                w: 320,
                h: 1,
            },
            solid: true,
        },
        Operation::Collider {
            rect: Rect {
                x: 0,
                y: 0,
                w: 1,
                h: 200,
            },
            solid: true,
        },
        Operation::Collider {
            rect: Rect {
                x: 319,
                y: 0,
                w: 1,
                h: 200,
            },
            solid: true,
        },
        Operation::Collider {
            rect: GATE,
            solid: true,
        },
    ]
}
pub fn initial_sim() -> Simulation {
    use charon_physics::sim::rules::{MaterialDefinition, MaterialRegistry, Movement};
    let mut registry = MaterialRegistry::default();
    registry
        .register(
            MaterialDefinition::new("Soil", Movement::Granular, 1600.0, [125, 84, 48, 255])
                .with_repose(38.0)
                .unwrap(),
        )
        .unwrap();
    registry
        .register(MaterialDefinition::new(
            "Cloud",
            Movement::Gas,
            0.7,
            [205, 222, 240, 255],
        ))
        .unwrap();
    registry
        .register(MaterialDefinition::new(
            "Nitrogen",
            Movement::Gas,
            1.165,
            [136, 180, 220, 255],
        ))
        .unwrap();
    let mut sim = Simulation::new(WIDTH, HEIGHT, 42).with_materials(registry);
    for (i, operation) in boundaries()
        .into_iter()
        .chain([
            Operation::Brush {
                x: 140,
                y: 20,
                radius: 12,
                cell: CellData::sand(17),
            },
            Operation::Brush {
                x: 230,
                y: 30,
                radius: 10,
                cell: CellData::water(31),
            },
        ])
        .enumerate()
    {
        sim.apply(Request {
            id: i as u64,
            expected_revision: None,
            operation,
        });
    }
    sim
}

#[derive(Clone, Copy)]
pub struct Brush {
    pub x: i32,
    pub y: i32,
    pub radius: u16,
    pub cell: CellData,
}
#[derive(Default)]
pub struct Controls {
    pub brush: Option<Brush>,
    pub reset: bool,
    pub pause: bool,
    pub gate: bool,
    pub release: bool,
    pub angle: Option<charon_physics::sim::material::Cell>,
}
#[derive(Default, Clone)]
pub struct Status {
    pub game_steps: u64,
    pub last_result: String,
    pub queue_full: bool,
}

struct GameCommands {
    client: SandClient,
    controls: Arc<Mutex<Controls>>,
    status: Arc<Mutex<Status>>,
    pending: VecDeque<Request>,
    next_id: u64,
    paused: bool,
}
impl GameCommands {
    fn push(&mut self, operation: Operation) {
        self.pending.push_back(Request {
            id: self.next_id,
            expected_revision: None,
            operation,
        });
        self.next_id += 1;
    }
}
impl Plugin for GameCommands {
    fn name(&self) -> &'static str {
        "sand-lab-game"
    }
    fn fixed_update(&mut self, _world: &mut World, _step: Step) -> PluginResult {
        {
            let mut status = self.status.lock().map_err(|_| "status lock poisoned")?;
            status.game_steps += 1;
            while let Ok(receipt) = self.client.try_receipt() {
                status.last_result = format!(
                    "{:?} / CHANGED {} / BLOCKED {}",
                    receipt.outcome, receipt.changed, receipt.blocked
                )
                .to_uppercase();
            }
        }
        if let Some(request) = self.pending.pop_front() {
            match self.client.try_send(request) {
                Ok(()) => {
                    self.status
                        .lock()
                        .map_err(|_| "status lock poisoned")?
                        .queue_full = false
                }
                Err(TrySendError::Full(request)) => {
                    self.pending.push_front(request);
                    self.status
                        .lock()
                        .map_err(|_| "status lock poisoned")?
                        .queue_full = true;
                    return Ok(());
                }
                Err(TrySendError::Disconnected(_)) => return Err("sand worker disconnected".into()),
            }
        }
        if !self.pending.is_empty() {
            return Ok(());
        }
        let operations = {
            let mut input = self.controls.lock().map_err(|_| "input lock poisoned")?;
            if std::mem::take(&mut input.reset) {
                self.paused = false;
                std::iter::once(Operation::Reset)
                    .chain(boundaries())
                    .collect::<Vec<_>>()
            } else if std::mem::take(&mut input.pause) {
                self.paused = !self.paused;
                vec![Operation::SetPaused(self.paused)]
            } else if let Some(kind) = input.angle.take() {
                let snapshot = self.client.snapshot();
                let current = snapshot.palette[usize::from(kind.id())]
                    .repose_degrees
                    .unwrap_or(32.0);
                let degrees = if current < 40.0 { 45.0 } else { 32.0 };
                vec![Operation::SetRepose { kind, degrees }]
            } else if std::mem::take(&mut input.release) {
                vec![Operation::Release {
                    rect: GATE,
                    cell: CellData::sand(0),
                }]
            } else if std::mem::take(&mut input.gate) {
                let snapshot = self.client.snapshot();
                let solid = !snapshot.colliders.contains(&(105 * WIDTH + 160));
                vec![Operation::Collider { rect: GATE, solid }]
            } else if let Some(b) = input.brush {
                vec![Operation::Brush {
                    x: b.x,
                    y: b.y,
                    radius: b.radius,
                    cell: b.cell,
                }]
            } else {
                Vec::new()
            }
        };
        for operation in operations {
            self.push(operation);
        }
        Ok(())
    }
}

pub struct Lab {
    pub client: SandClient,
    controls: Arc<Mutex<Controls>>,
    status: Arc<Mutex<Status>>,
    game: Worker,
    compute: Worker,
}
impl Lab {
    pub fn start(hz: u32) -> Result<Self, Box<dyn std::error::Error>> {
        if !(30..=60).contains(&hz) {
            return Err("compute rate must be 30..=60 Hz".into());
        }
        let (plugin, client) = FallingSandPlugin::new(initial_sim(), 64)?;
        let compute = Worker::spawn(
            WorkerConfig::new("sand-compute", hz),
            move |delta| {
                let mut app = App::with_timing(delta, 8)?;
                app.add_plugin(plugin)?;
                Ok(app)
            },
            (),
        )?;
        let controls = Arc::new(Mutex::new(Controls::default()));
        let status = Arc::new(Mutex::new(Status::default()));
        let c = controls.clone();
        let s = status.clone();
        let sand = client.clone();
        let game = Worker::spawn(
            WorkerConfig::new("sand-game", 144),
            move |delta| {
                let mut app = App::with_timing(delta, 8)?;
                app.add_plugin(GameCommands {
                    client: sand,
                    controls: c,
                    status: s,
                    pending: VecDeque::new(),
                    next_id: 100,
                    paused: false,
                })?;
                Ok(app)
            },
            (),
        )?;
        Ok(Self {
            client,
            controls,
            status,
            game,
            compute,
        })
    }
    pub fn input(&self, input: Controls) -> PluginResult {
        let mut shared = self.controls.lock().map_err(|_| "input lock poisoned")?;
        shared.brush = input.brush;
        shared.reset |= input.reset;
        shared.pause |= input.pause;
        shared.gate |= input.gate;
        shared.release |= input.release;
        if input.angle.is_some() {
            shared.angle = input.angle;
        }
        Ok(())
    }
    pub fn status(&self) -> Status {
        self.status.lock().expect("status lock poisoned").clone()
    }
    pub fn check(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.game.is_finished() || self.compute.is_finished() {
            self.shutdown()?;
            return Err("worker stopped unexpectedly".into());
        }
        Ok(())
    }
    pub fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.game.stop();
        self.compute.stop();
        let game = self.game.join();
        let compute = self.compute.join();
        game?;
        compute?;
        Ok(())
    }
}
