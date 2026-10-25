use charon_core::{App, Plugin, PluginResult, Step, Worker, WorkerConfig, World};
use charon_tilemap::{TileDefinition, TileId, Tilemap};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};
pub const W: u32 = 40;
pub const H: u32 = 24;
pub const CELL: u32 = 8;
#[derive(Clone, Debug)]
pub enum Edit {
    Break(u32, u32),
    Place(u32, u32, TileId),
}
#[derive(Clone)]
pub struct View {
    pub map: Tilemap,
    pub ticks: u64,
    pub revision: u64,
    pub message: String,
    #[cfg(feature = "bridge")]
    pub sand: Option<charon_physics::sim::simulation::Snapshot>,
}
#[derive(Clone, Copy, Default)]
pub struct Controls {
    pub dx: f32,
    pub dy: f32,
}
#[derive(Clone, Copy)]
pub struct Player {
    pub position: [f32; 2],
    pub score: u32,
    pub ticks: u64,
}
impl Default for Player {
    fn default() -> Self {
        Self {
            position: [32., 80.],
            score: 0,
            ticks: 0,
        }
    }
}
#[derive(Clone)]
pub struct Shared {
    pub view: Arc<Mutex<Arc<View>>>,
    pub player: Arc<Mutex<Player>>,
    pub controls: Arc<Mutex<Controls>>,
}
struct Compute {
    map: Tilemap,
    #[cfg(feature = "bridge")]
    bridge: Option<charon_bridge::Bridge>,
    rx: Receiver<(u64, Edit)>,
    shared: Shared,
    ticks: u64,
    message: String,
}
pub fn initial_map() -> Tilemap {
    let mut map = Tilemap::new(W, H, [CELL, CELL]).unwrap();
    for (name, index) in [("Stone", 0), ("Sand block", 1), ("Water block", 2)] {
        map.register(TileDefinition {
            name: name.into(),
            solid: true,
            atlas_index: index,
            tint: [1.; 4],
        })
        .unwrap();
    }
    for y in 0..H {
        for x in 0..W {
            let id = if x == 0 || x == W - 1 || y == H - 1 || (y == 12 && (8..=30).contains(&x)) {
                Some(TileId(0))
            } else if (8..=12).contains(&x) && (8..12).contains(&y) {
                Some(TileId(1))
            } else if (23..=27).contains(&x) && (8..12).contains(&y) {
                Some(TileId(2))
            } else {
                None
            };
            if let Some(id) = id {
                map.set(x, y, Some(id)).unwrap();
            }
        }
    }
    map.drain_changes().for_each(drop);
    map
}
impl Compute {
    fn make(
        hz: u32,
        shared: Shared,
        rx: Receiver<(u64, Edit)>,
        coupled: bool,
    ) -> Result<Self, String> {
        let map = initial_map();
        #[cfg(feature = "bridge")]
        let bridge = if coupled {
            use charon_physics::sim::{material::CellData, simulation::Simulation};
            let mut bridge = charon_bridge::Bridge::new(
                map.clone(),
                Simulation::new((W * CELL) as usize, (H * CELL) as usize, 43),
            )?;
            bridge.set_conversion(TileId(1), Some(CellData::sand(19)))?;
            bridge.set_conversion(TileId(2), Some(CellData::water(12)))?;
            Some(bridge)
        } else {
            None
        };
        #[cfg(not(feature = "bridge"))]
        if coupled {
            return Err("bridge feature is not compiled".into());
        }
        Ok(Self {
            map,
            #[cfg(feature = "bridge")]
            bridge,
            rx,
            shared,
            ticks: 0,
            message: format!("COMPUTE {hz} HZ / READY"),
        })
    }
    fn apply(&mut self, id: u64, edit: Edit) {
        #[cfg(feature = "bridge")]
        if let Some(bridge) = &mut self.bridge {
            let edit = match edit {
                Edit::Break(x, y) => charon_bridge::Edit::Break { x, y },
                Edit::Place(x, y, tile) => charon_bridge::Edit::Place { x, y, tile },
            };
            let receipt = bridge.apply(charon_bridge::Command {
                id,
                expected_revision: None,
                edit,
            });
            self.message = format!(
                "EDIT {} {:?} CELLS {}",
                receipt.id, receipt.result, receipt.changed_cells
            );
            return;
        }
        let _ = id;
        let result = match edit {
            Edit::Break(x, y) => self.map.set(x, y, None),
            Edit::Place(x, y, tile) => {
                if self.map.get(x, y).is_some() {
                    Err("Occupied".into())
                } else {
                    self.map.set(x, y, Some(tile))
                }
            }
        };
        self.message = format!("EDIT {result:?}");
    }
    fn view(&self) -> View {
        #[cfg(feature = "bridge")]
        if let Some(bridge) = &self.bridge {
            let snap = bridge.snapshot();
            return View {
                map: snap.tiles,
                ticks: self.ticks,
                revision: snap.revision,
                message: self.message.clone(),
                sand: Some(snap.sand),
            };
        }
        View {
            map: self.map.clone(),
            ticks: self.ticks,
            revision: self.map.revision(),
            message: self.message.clone(),
            #[cfg(feature = "bridge")]
            sand: None,
        }
    }
}
impl Plugin for Compute {
    fn name(&self) -> &'static str {
        "world-compute"
    }
    fn fixed_update(&mut self, _: &mut World, _: Step) -> PluginResult {
        for _ in 0..64 {
            match self.rx.try_recv() {
                Ok((id, edit)) => self.apply(id, edit),
                Err(_) => break,
            }
        }
        #[cfg(feature = "bridge")]
        if let Some(bridge) = &mut self.bridge {
            bridge.step();
            bridge.drain_changes().for_each(drop);
        }
        self.map.drain_changes().for_each(drop);
        self.ticks += 1;
        *self.shared.view.lock().unwrap() = Arc::new(self.view());
        Ok(())
    }
}
struct Game {
    shared: Shared,
    rx: Receiver<Edit>,
    tx: SyncSender<(u64, Edit)>,
    pending: VecDeque<(u64, Edit)>,
    id: u64,
    player: Player,
    gem: usize,
}
pub const GEMS: [[f32; 2]; 3] = [[64., 80.], [128., 48.], [240., 48.]];
impl Plugin for Game {
    fn name(&self) -> &'static str {
        "gameplay"
    }
    fn fixed_update(&mut self, _: &mut World, step: Step) -> PluginResult {
        while self.pending.len() < 64 {
            match self.rx.try_recv() {
                Ok(edit) => {
                    self.id += 1;
                    self.pending.push_back((self.id, edit));
                }
                Err(_) => break,
            }
        }
        while let Some(command) = self.pending.pop_front() {
            match self.tx.try_send(command) {
                Ok(()) => {}
                Err(TrySendError::Full(c)) => {
                    self.pending.push_front(c);
                    break;
                }
                Err(_) => return Err("compute disconnected".into()),
            }
        }
        let controls = *self.shared.controls.lock().unwrap();
        let view = self.shared.view.lock().unwrap().clone();
        let distance = 60. * step.delta.as_secs_f32();
        for (axis, d) in [(0, controls.dx), (1, controls.dy)] {
            let mut next = self.player.position;
            next[axis] += d * distance;
            let fits = [(0., 0.), (5., 0.), (0., 5.), (5., 5.)]
                .iter()
                .all(|&(dx, dy)| {
                    let x = (next[0] + dx) / CELL as f32;
                    let y = (next[1] + dy) / CELL as f32;
                    x >= 0.
                        && y >= 0.
                        && x < W as f32
                        && y < H as f32
                        && view
                            .map
                            .get(x as u32, y as u32)
                            .is_none_or(|id| !view.map.definition(id).unwrap().solid)
                });
            if fits {
                self.player.position = next;
            }
        }
        if self.gem < GEMS.len() {
            let target = GEMS[self.gem];
            if (target[0] - self.player.position[0]).abs() < 7.
                && (target[1] - self.player.position[1]).abs() < 7.
            {
                self.gem += 1;
                self.player.score += 1;
            }
        }
        self.player.ticks += 1;
        *self.shared.player.lock().unwrap() = self.player;
        Ok(())
    }
}
pub struct Runtime {
    pub shared: Shared,
    pub edits: SyncSender<Edit>,
    game: Worker,
    compute: Worker,
}
impl Runtime {
    pub fn start(hz: u32, coupled: bool) -> Result<Self, Box<dyn std::error::Error>> {
        if !(30..=60).contains(&hz) {
            return Err("compute rate must be 30..60".into());
        }
        let shared = Shared {
            view: Arc::new(Mutex::new(Arc::new(View {
                map: initial_map(),
                ticks: 0,
                revision: 0,
                message: "STARTING".into(),
                #[cfg(feature = "bridge")]
                sand: None,
            }))),
            player: Arc::new(Mutex::new(Player::default())),
            controls: Arc::new(Mutex::new(Controls::default())),
        };
        let (tx, rx) = mpsc::sync_channel(64);
        let compute = Compute::make(hz, shared.clone(), rx, coupled)?;
        let compute = Worker::spawn(
            WorkerConfig::new("charon-compute", hz),
            move |delta| {
                let mut app = App::with_timing(delta, 8)?;
                app.add_plugin(compute)?;
                Ok(app)
            },
            (),
        )?;
        let (edits, rx) = mpsc::sync_channel(64);
        let game = Game {
            shared: shared.clone(),
            rx,
            tx,
            pending: VecDeque::new(),
            id: 0,
            player: Player::default(),
            gem: 0,
        };
        let game = Worker::spawn(
            WorkerConfig::new("charon-game", 144),
            move |delta| {
                let mut app = App::with_timing(delta, 8)?;
                app.add_plugin(game)?;
                Ok(app)
            },
            (),
        )?;
        Ok(Self {
            shared,
            edits,
            game,
            compute,
        })
    }
    pub fn check(&self) -> Result<(), String> {
        if self.game.is_finished() || self.compute.is_finished() {
            Err("worker exited unexpectedly".into())
        } else {
            Ok(())
        }
    }
    pub fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.game.join()?;
        self.compute.join()?;
        Ok(())
    }
}
pub fn smoke() -> Result<(), Box<dyn std::error::Error>> {
    for hz in [30, 60] {
        for coupled in [false, true] {
            if coupled && !cfg!(feature = "bridge") {
                continue;
            }
            let mut runtime = Runtime::start(hz, coupled)?;
            runtime.edits.try_send(Edit::Break(10, 10))?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                runtime.check()?;
                let view = runtime.shared.view.lock().unwrap().clone();
                if view.map.get(10, 10).is_none() {
                    #[cfg(feature = "bridge")]
                    if coupled {
                        let sand = view.sand.as_ref().ok_or("missing sand")?;
                        if sand
                            .cells
                            .iter()
                            .filter(|c| c.kind == charon_physics::sim::material::Cell::Sand)
                            .count()
                            != 64
                        {
                            return Err("conversion did not preserve 64 cells".into());
                        }
                    }
                    break;
                }
                if Instant::now() > deadline {
                    return Err("edit timed out".into());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            runtime.shutdown()?;
            println!(
                "M3 game144/render-independent/compute{hz}, bridge={coupled}: command, conversion, shutdown OK"
            );
        }
    }
    Ok(())
}
