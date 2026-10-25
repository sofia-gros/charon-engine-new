//! Optional adapter. The sandengine library remains usable without this crate.
use charon_core::{Plugin, PluginResult, Step, World};
pub use sandengine::sim::simulation::{
    Contact, Operation, Outcome, Receipt, Rect, Request, Simulation, Snapshot,
};
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
};

pub struct SandState(pub Simulation);

/// Contact notifications are a bounded current-contact sample, not enter/exit events.
#[derive(Clone, Copy, Debug)]
pub struct ContactOverflow;

#[derive(Clone)]
pub struct SandClient {
    requests: SyncSender<Request>,
    receipts: Arc<Mutex<Receiver<Receipt>>>,
    snapshot: Arc<Mutex<Arc<Snapshot>>>,
}
impl SandClient {
    /// Full returns the original request so the caller can retry explicitly.
    pub fn try_send(&self, request: Request) -> Result<(), TrySendError<Request>> {
        self.requests.try_send(request)
    }
    pub fn try_receipt(&self) -> Result<Receipt, TryRecvError> {
        self.receipts
            .lock()
            .expect("receipt receiver lock poisoned")
            .try_recv()
    }
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot
            .lock()
            .expect("snapshot lock poisoned")
            .clone()
    }
}

pub struct FallingSandPlugin {
    simulation: Option<Simulation>,
    requests: Option<Receiver<Request>>,
    receipts: Option<SyncSender<Receipt>>,
    snapshot: Arc<Mutex<Arc<Snapshot>>>,
    pending: Option<Receipt>,
    owns_state: bool,
    contact_limit: usize,
}

impl FallingSandPlugin {
    /// Both queues are bounded. A full result queue stops further commands,
    /// while simulation ticks continue. Poll receipts to release backpressure.
    pub fn new(
        simulation: Simulation,
        capacity: usize,
    ) -> Result<(Self, SandClient), &'static str> {
        if capacity == 0 {
            return Err("queue capacity must be positive");
        }
        let (request_tx, request_rx) = mpsc::sync_channel(capacity);
        let (receipt_tx, receipt_rx) = mpsc::sync_channel(capacity);
        let snapshot = Arc::new(Mutex::new(Arc::new(simulation.snapshot())));
        let client = SandClient {
            requests: request_tx,
            receipts: Arc::new(Mutex::new(receipt_rx)),
            snapshot: snapshot.clone(),
        };
        Ok((
            Self {
                simulation: Some(simulation),
                requests: Some(request_rx),
                receipts: Some(receipt_tx),
                snapshot,
                pending: None,
                owns_state: false,
                contact_limit: 0,
            },
            client,
        ))
    }

    /// Opt in to per-tick Contact events; overflow is reported explicitly.
    pub fn with_contact_events(mut self, limit: usize) -> Self {
        self.contact_limit = limit;
        self
    }

    fn flush_pending(&mut self) -> PluginResult {
        if let Some(receipt) = self.pending.take() {
            match self
                .receipts
                .as_ref()
                .ok_or("receipt sender closed")?
                .try_send(receipt)
            {
                Ok(()) => {}
                Err(TrySendError::Full(receipt)) => self.pending = Some(receipt),
                Err(TrySendError::Disconnected(_)) => {
                    return Err("sand receipt receiver disconnected".into());
                }
            }
        }
        Ok(())
    }
}

impl Plugin for FallingSandPlugin {
    fn name(&self) -> &'static str {
        "falling-sand"
    }
    fn init(&mut self, world: &mut World) -> PluginResult {
        if world.get::<SandState>().is_some() {
            return Err("SandState already exists".into());
        }
        world.insert(SandState(
            self.simulation.take().ok_or("plugin already initialized")?,
        ));
        self.owns_state = true;
        Ok(())
    }
    fn fixed_update(&mut self, world: &mut World, _: Step) -> PluginResult {
        world.clear_events::<Receipt>();
        world.clear_events::<Contact>();
        world.clear_events::<ContactOverflow>();
        self.flush_pending()?;
        // Bound command work independently of the channel's configured capacity.
        for _ in 0..64 {
            if self.pending.is_some() {
                break;
            }
            let request = match self
                .requests
                .as_ref()
                .ok_or("request receiver closed")?
                .try_recv()
            {
                Ok(request) => request,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            };
            let receipt = world
                .get_mut::<SandState>()
                .ok_or("SandState missing")?
                .0
                .apply(request);
            world.send(receipt.clone());
            self.pending = Some(receipt);
            self.flush_pending()?;
        }
        let state = &mut world.get_mut::<SandState>().ok_or("SandState missing")?.0;
        state.step();
        let snapshot = Arc::new(state.snapshot());
        if self.contact_limit > 0 {
            let contacts: Vec<_> = state
                .contacts()
                .take(self.contact_limit.saturating_add(1))
                .collect();
            let overflow = contacts.len() > self.contact_limit;
            for contact in contacts.into_iter().take(self.contact_limit) {
                world.send(contact);
            }
            if overflow {
                world.send(ContactOverflow);
            }
        }
        let previous = {
            let mut target = self.snapshot.lock().map_err(|_| "snapshot lock poisoned")?;
            std::mem::replace(&mut *target, snapshot)
        };
        drop(previous);
        Ok(())
    }
    fn shutdown(&mut self, world: &mut World) -> PluginResult {
        if self.owns_state {
            world.remove::<SandState>();
            self.owns_state = false;
        }
        self.requests.take();
        self.receipts.take();
        self.pending.take();
        Ok(())
    }
}
