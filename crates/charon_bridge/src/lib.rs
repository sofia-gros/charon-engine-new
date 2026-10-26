//! Transactional, optional Tilemap/Falling Sand adapter. Owns both mutable worlds
//! on the compute lane. Neither underlying library depends on this crate.
use charon_core::{Plugin, PluginResult, Step, World};
use charon_physics::sim::{
    material::{Cell, CellData},
    simulation::{Operation, Outcome, Rect, Request, Simulation, Snapshot},
};
use charon_tilemap::{TileId, Tilemap};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
};
#[derive(Clone, Debug)]
pub enum Edit {
    Place {
        x: u32,
        y: u32,
        tile: TileId,
    },
    Break {
        x: u32,
        y: u32,
    },
    Paint {
        x: i32,
        y: i32,
        radius: u16,
        cell: CellData,
    },
}
#[derive(Clone, Debug)]
pub struct Command {
    pub id: u64,
    pub expected_revision: Option<u64>,
    pub edit: Edit,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResultKind {
    Applied,
    Duplicate,
    Stale,
    Occupied,
    Invalid,
    PlacementDenied,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditReceipt {
    pub id: u64,
    pub revision: u64,
    pub result: ResultKind,
    pub changed_cells: usize,
}
#[derive(Clone)]
pub struct BridgeSnapshot {
    pub tiles: Tilemap,
    pub sand: Snapshot,
    pub revision: u64,
}
/// Additional game restriction. Solid placement always rejects occupied cells,
/// regardless of this callback; it never deletes particles implicitly.
pub type PlacementRule = fn(TileId, &[CellData]) -> bool;
pub struct Bridge {
    tiles: Tilemap,
    sand: Simulation,
    conversion: HashMap<TileId, CellData>,
    placement: PlacementRule,
    revision: u64,
    last_id: Option<u64>,
    physics_id: u64,
}
impl Bridge {
    pub fn new(mut tiles: Tilemap, mut sand: Simulation) -> Result<Self, String> {
        let [w, h] = tiles.dimensions();
        let [tw, th] = tiles.tile_size();
        if u64::from(w) * u64::from(tw) > i32::MAX as u64
            || u64::from(h) * u64::from(th) > i32::MAX as u64
            || u64::from(w) * u64::from(tw) != sand.grid().w as u64
            || u64::from(h) * u64::from(th) != sand.grid().h as u64
            || !sand.snapshot().colliders.is_empty()
        {
            return Err("bridge requires matching extents and an unowned collision layer".into());
        }
        let mut physics_id = 0;
        for (x, y, id) in tiles.occupied() {
            if tiles.definition(id).unwrap().solid {
                physics_id += 1;
                let receipt = sand.apply(Request {
                    id: physics_id,
                    expected_revision: None,
                    operation: Operation::Collider {
                        rect: Rect {
                            x: (x * tw) as i32,
                            y: (y * th) as i32,
                            w: tw,
                            h: th,
                        },
                        solid: true,
                    },
                });
                if receipt.outcome != Outcome::Applied {
                    return Err(
                        "initial tile intersects particles or simulation request history".into(),
                    );
                }
            }
        }
        // A private request sequence must start above any existing request ID.
        // Constructor requires a fresh request stream; probe even for empty maps.
        physics_id += 1;
        if sand
            .apply(Request {
                id: physics_id,
                expected_revision: None,
                operation: Operation::SetPaused(false),
            })
            .outcome
            != Outcome::Applied
        {
            return Err("bridge requires a fresh simulation request stream".into());
        }
        tiles.drain_changes().for_each(drop);
        Ok(Self {
            tiles,
            sand,
            conversion: HashMap::new(),
            placement: |_, _| true,
            revision: 0,
            last_id: None,
            physics_id,
        })
    }
    pub fn tiles(&self) -> &Tilemap {
        &self.tiles
    }
    pub fn sand(&self) -> &Simulation {
        &self.sand
    }
    pub fn set_conversion(&mut self, tile: TileId, cell: Option<CellData>) -> Result<(), String> {
        let def = self.tiles.definition(tile).ok_or("unknown tile")?;
        if let Some(cell) = cell {
            if !def.solid
                || cell.kind == Cell::Air
                || !self.sand.grid().materials.contains(cell.kind)
            {
                return Err(
                    "conversion requires a solid tile and registered non-air material".into(),
                );
            }
            self.conversion.insert(tile, cell);
        } else {
            self.conversion.remove(&tile);
        }
        Ok(())
    }
    pub fn set_placement_rule(&mut self, rule: PlacementRule) {
        self.placement = rule;
    }
    fn rect(&self, x: u32, y: u32) -> Rect {
        let [w, h] = self.tiles.tile_size();
        Rect {
            x: (x * w) as i32,
            y: (y * h) as i32,
            w,
            h,
        }
    }
    pub fn occupancy(&self, x: u32, y: u32) -> Option<Vec<CellData>> {
        let [w, h] = self.tiles.dimensions();
        if x >= w || y >= h {
            return None;
        }
        let r = self.rect(x, y);
        let grid = self.sand.grid();
        Some(
            (r.y as usize..r.y as usize + r.h as usize)
                .flat_map(|py| {
                    (r.x as usize..r.x as usize + r.w as usize).map(move |px| grid.get(px, py))
                })
                .collect(),
        )
    }
    fn apply_physics(&mut self, operation: Operation) -> charon_physics::sim::simulation::Receipt {
        self.physics_id = self
            .physics_id
            .checked_add(1)
            .expect("bridge request IDs exhausted");
        self.sand.apply(Request {
            id: self.physics_id,
            expected_revision: None,
            operation,
        })
    }
    /// Call only on the compute lane, before step. Failure leaves both data
    /// layers unchanged. Request IDs are consumed even for rejected edits.
    pub fn apply(&mut self, command: Command) -> EditReceipt {
        let mut receipt = EditReceipt {
            id: command.id,
            revision: self.revision,
            result: ResultKind::Applied,
            changed_cells: 0,
        };
        if self.last_id.is_some_and(|id| command.id <= id) {
            receipt.result = ResultKind::Duplicate;
            return receipt;
        }
        self.last_id = Some(command.id);
        if command
            .expected_revision
            .is_some_and(|rev| rev != self.revision)
        {
            receipt.result = ResultKind::Stale;
            return receipt;
        }
        let operation = match command.edit {
            Edit::Paint { x, y, radius, cell } => Some(Operation::Brush { x, y, radius, cell }),
            Edit::Place { x, y, tile } => {
                let Some(cells) = self.occupancy(x, y) else {
                    receipt.result = ResultKind::Invalid;
                    return receipt;
                };
                let Some(def) = self.tiles.definition(tile) else {
                    receipt.result = ResultKind::Invalid;
                    return receipt;
                };
                if self.tiles.get(x, y).is_some() {
                    receipt.result = ResultKind::Occupied;
                    return receipt;
                }
                if !(self.placement)(tile, &cells) {
                    receipt.result = ResultKind::PlacementDenied;
                    return receipt;
                }
                if def.solid && cells.iter().any(|c| c.kind != Cell::Air) {
                    receipt.result = ResultKind::Occupied;
                    return receipt;
                }
                let op = def.solid.then(|| Operation::Collider {
                    rect: self.rect(x, y),
                    solid: true,
                });
                if let Some(op) = op {
                    let result = self.apply_physics(op);
                    if result.outcome != Outcome::Applied {
                        receipt.result = ResultKind::Invalid;
                        return receipt;
                    }
                }
                self.tiles.set(x, y, Some(tile)).expect("validated tile");
                receipt.changed_cells = 1;
                None
            }
            Edit::Break { x, y } => {
                if self.occupancy(x, y).is_none() {
                    receipt.result = ResultKind::Invalid;
                    return receipt;
                }
                let Some(tile) = self.tiles.get(x, y) else {
                    return receipt;
                };
                if self.tiles.definition(tile).unwrap().solid {
                    let rect = self.rect(x, y);
                    let op = if let Some(&cell) = self.conversion.get(&tile) {
                        Operation::Release { rect, cell }
                    } else {
                        Operation::Collider { rect, solid: false }
                    };
                    let result = self.apply_physics(op);
                    if result.outcome != Outcome::Applied {
                        receipt.result = ResultKind::Invalid;
                        return receipt;
                    }
                    receipt.changed_cells = result.changed;
                } else {
                    receipt.changed_cells = 1;
                }
                self.tiles.set(x, y, None).expect("validated tile");
                None
            }
        };
        if let Some(op) = operation {
            let result = self.apply_physics(op);
            receipt.changed_cells = result.changed;
            if result.outcome != Outcome::Applied {
                receipt.result = ResultKind::Invalid;
                return receipt;
            }
        }
        if receipt.changed_cells > 0 {
            self.revision = self.revision.saturating_add(1)
        }
        receipt.revision = self.revision;
        receipt
    }
    #[cfg(feature = "parallel")]
    pub fn set_cpu_pool(&mut self, pool: Arc<charon_tasks::TaskPool>) {
        self.sand.set_cpu_pool(pool);
    }
    #[cfg(feature = "parallel")]
    pub fn parallel_jobs(&self) -> usize {
        self.sand.parallel_jobs()
    }
    pub fn step(&mut self) {
        self.sand.step();
        self.revision = self.revision.saturating_add(1);
    }
    pub fn snapshot(&self) -> BridgeSnapshot {
        let mut tiles = self.tiles.clone();
        tiles.drain_changes().for_each(drop);
        BridgeSnapshot {
            tiles,
            sand: self.sand.snapshot(),
            revision: self.revision,
        }
    }
    /// Changes are local to one compute boundary; consumers can drain first.
    pub fn drain_changes(&mut self) -> impl Iterator<Item = charon_tilemap::Change> + '_ {
        self.tiles.drain_changes()
    }
}
#[derive(Clone)]
pub struct BridgeClient {
    tx: SyncSender<Command>,
    rx: Arc<Mutex<Receiver<EditReceipt>>>,
    snapshot: Arc<Mutex<Arc<BridgeSnapshot>>>,
}
impl BridgeClient {
    pub fn try_send(&self, c: Command) -> Result<(), TrySendError<Command>> {
        self.tx.try_send(c)
    }
    pub fn try_receipt(&self) -> Result<EditReceipt, TryRecvError> {
        self.rx.lock().unwrap().try_recv()
    }
    pub fn snapshot(&self) -> Arc<BridgeSnapshot> {
        self.snapshot.lock().unwrap().clone()
    }
}
pub struct BridgePlugin {
    bridge: Bridge,
    rx: Receiver<Command>,
    tx: SyncSender<EditReceipt>,
    pending: Option<EditReceipt>,
    snapshot: Arc<Mutex<Arc<BridgeSnapshot>>>,
}
impl BridgePlugin {
    pub fn new(bridge: Bridge, capacity: usize) -> Result<(Self, BridgeClient), String> {
        if capacity == 0 {
            return Err("capacity must be positive".into());
        }
        let (tx, rx) = mpsc::sync_channel(capacity);
        let (out, receipts) = mpsc::sync_channel(capacity);
        let snapshot = Arc::new(Mutex::new(Arc::new(bridge.snapshot())));
        Ok((
            Self {
                bridge,
                rx,
                tx: out,
                pending: None,
                snapshot: snapshot.clone(),
            },
            BridgeClient {
                tx,
                rx: Arc::new(Mutex::new(receipts)),
                snapshot,
            },
        ))
    }
}
impl Plugin for BridgePlugin {
    fn name(&self) -> &'static str {
        "tilemap-sand-bridge"
    }
    fn fixed_update(&mut self, _: &mut World, _: Step) -> PluginResult {
        for _ in 0..64 {
            if let Some(r) = self.pending.take() {
                match self.tx.try_send(r) {
                    Ok(()) => {}
                    Err(TrySendError::Full(r)) => {
                        self.pending = Some(r);
                        break;
                    }
                    Err(_) => return Err("bridge client disconnected".into()),
                }
            }
            match self.rx.try_recv() {
                Ok(c) => self.pending = Some(self.bridge.apply(c)),
                Err(TryRecvError::Empty) => break,
                Err(_) => return Err("bridge command sender disconnected".into()),
            }
        }
        self.bridge.step();
        self.bridge.drain_changes().for_each(drop);
        *self
            .snapshot
            .lock()
            .map_err(|_| "bridge snapshot poisoned")? = Arc::new(self.bridge.snapshot());
        Ok(())
    }
    fn shutdown(&mut self, _: &mut World) -> PluginResult {
        let (_, closed_rx) = mpsc::sync_channel(1);
        let (closed_tx, _) = mpsc::sync_channel(1);
        self.rx = closed_rx;
        self.tx = closed_tx;
        self.pending = None;
        Ok(())
    }
}
