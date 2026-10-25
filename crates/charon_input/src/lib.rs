//! Host-neutral, tick-based input. OS adapters are optional, separate crates.
use std::collections::BTreeSet;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(pub u32);
pub mod keys {
    use super::Key;
    pub const ENTER: Key = Key(0x100);
    pub const ESCAPE: Key = Key(0x101);
    pub const TAB: Key = Key(0x102);
    pub const SPACE: Key = Key(0x103);
    pub const LEFT: Key = Key(0x104);
    pub const RIGHT: Key = Key(0x105);
    pub const UP: Key = Key(0x106);
    pub const DOWN: Key = Key(0x107);
    pub const SHIFT_LEFT: Key = Key(0x108);
    pub const SHIFT_RIGHT: Key = Key(0x109);
    pub const CONTROL_LEFT: Key = Key(0x10a);
    pub const CONTROL_RIGHT: Key = Key(0x10b);
    pub const ALT_LEFT: Key = Key(0x10c);
    pub const ALT_RIGHT: Key = Key(0x10d);
    pub const BACKSPACE: Key = Key(0x10e);
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Button {
    Key(Key),
    Mouse(u32),
}
impl From<Key> for Button {
    fn from(key: Key) -> Self {
        Self::Key(key)
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputEvent {
    Button { button: Button, pressed: bool },
    Repeat(Button),
    Pointer([f32; 2]),
    PointerLeft,
    Wheel([f32; 2]),
    Focus(bool),
    Resize([u32; 2]),
}
#[derive(Clone, Debug, PartialEq)]
pub struct InputState {
    held: BTreeSet<Button>,
    pressed: BTreeSet<Button>,
    released: BTreeSet<Button>,
    repeated: BTreeSet<Button>,
    pub pointer: Option<[f32; 2]>,
    pub wheel: [f32; 2],
    pub focused: bool,
    pub size: [u32; 2],
    pub tick: u64,
}
impl Default for InputState {
    fn default() -> Self {
        Self {
            held: BTreeSet::new(),
            pressed: BTreeSet::new(),
            released: BTreeSet::new(),
            repeated: BTreeSet::new(),
            pointer: None,
            wheel: [0.; 2],
            focused: true,
            size: [0, 0],
            tick: 0,
        }
    }
}
impl InputState {
    pub fn begin_tick(&mut self) {
        self.pressed.clear();
        self.released.clear();
        self.repeated.clear();
        self.wheel = [0.; 2];
        self.tick = self.tick.saturating_add(1);
    }
    pub fn held(&self, b: impl Into<Button>) -> bool {
        self.held.contains(&b.into())
    }
    pub fn pressed(&self, b: impl Into<Button>) -> bool {
        self.pressed.contains(&b.into())
    }
    pub fn released(&self, b: impl Into<Button>) -> bool {
        self.released.contains(&b.into())
    }
    pub fn repeated(&self, b: impl Into<Button>) -> bool {
        self.repeated.contains(&b.into())
    }
    pub fn held_buttons(&self) -> impl Iterator<Item = Button> + '_ {
        self.held.iter().copied()
    }
    pub fn apply(&mut self, event: InputEvent) -> Result<(), String> {
        match event {
            InputEvent::Button { button, pressed } => {
                if pressed {
                    if self.focused && self.held.insert(button) {
                        self.pressed.insert(button);
                    }
                } else if self.held.remove(&button) {
                    self.released.insert(button);
                }
            }
            InputEvent::Repeat(button) => {
                if self.focused && self.held.contains(&button) {
                    self.repeated.insert(button);
                }
            }
            InputEvent::Pointer(p) => {
                if p.iter().any(|v| !v.is_finite()) {
                    return Err("non-finite pointer".into());
                }
                if self.focused {
                    self.pointer = Some(p)
                }
            }
            InputEvent::PointerLeft => self.pointer = None,
            InputEvent::Wheel(w) => {
                if w.iter().any(|v| !v.is_finite()) {
                    return Err("non-finite wheel".into());
                }
                if self.focused {
                    for (k, value) in w.iter().enumerate() {
                        self.wheel[k] = (self.wheel[k] + value).clamp(-1024., 1024.);
                    }
                }
            }
            InputEvent::Focus(focused) => {
                self.focused = focused;
                if !focused {
                    self.released.extend(self.held.iter().copied());
                    self.held.clear();
                    self.pressed.clear();
                    self.repeated.clear();
                    self.pointer = None;
                    self.wheel = [0.; 2];
                }
            }
            InputEvent::Resize(size) => self.size = size,
        }
        Ok(())
    }
}
/// Scene boundary filter. Physically held buttons must be released before the
/// next scene accepts them, including OS repeat events after a transition.
#[derive(Default)]
pub struct InputBarrier {
    physical: BTreeSet<Button>,
    blocked: BTreeSet<Button>,
}
impl InputBarrier {
    pub fn scene_changed(&mut self) {
        self.blocked = self.physical.clone();
    }
    pub fn route(&mut self, event: InputEvent) -> Option<InputEvent> {
        match event {
            InputEvent::Button { button, pressed } => {
                if pressed {
                    self.physical.insert(button);
                } else {
                    self.physical.remove(&button);
                }
                if self.blocked.contains(&button) {
                    if !pressed {
                        self.blocked.remove(&button);
                    }
                    None
                } else {
                    Some(event)
                }
            }
            InputEvent::Focus(false) => {
                self.physical.clear();
                self.blocked.clear();
                Some(event)
            }
            _ => Some(event),
        }
    }
}
#[cfg(feature = "plugin")]
mod integration {
    use super::*;
    use charon_core::{Plugin, PluginResult, Step, World};
    use std::sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    };
    #[derive(Clone)]
    pub struct InputClient {
        tx: SyncSender<InputEvent>,
        snapshot: Arc<Mutex<Arc<InputState>>>,
    }
    impl InputClient {
        pub fn try_send(&self, event: InputEvent) -> Result<(), TrySendError<InputEvent>> {
            self.tx.try_send(event)
        }
        pub fn snapshot(&self) -> Arc<InputState> {
            self.snapshot.lock().unwrap().clone()
        }
    }
    pub struct InputPlugin {
        rx: Option<Receiver<InputEvent>>,
        snapshot: Arc<Mutex<Arc<InputState>>>,
        owns: bool,
    }
    impl InputPlugin {
        pub fn new(capacity: usize) -> Result<(Self, InputClient), String> {
            if capacity == 0 {
                return Err("input capacity must be positive".into());
            }
            let (tx, rx) = mpsc::sync_channel(capacity);
            let snapshot = Arc::new(Mutex::new(Arc::new(InputState::default())));
            Ok((
                Self {
                    rx: Some(rx),
                    snapshot: snapshot.clone(),
                    owns: false,
                },
                InputClient { tx, snapshot },
            ))
        }
    }
    impl Plugin for InputPlugin {
        fn name(&self) -> &'static str {
            "input"
        }
        fn init(&mut self, w: &mut World) -> PluginResult {
            if w.get::<InputState>().is_some() {
                return Err("InputState already exists".into());
            }
            w.insert(InputState::default());
            self.owns = true;
            Ok(())
        }
        fn fixed_update(&mut self, w: &mut World, _: Step) -> PluginResult {
            let state = w.get_mut::<InputState>().ok_or("InputState missing")?;
            state.begin_tick();
            for _ in 0..256 {
                match self.rx.as_ref().ok_or("input stopped")?.try_recv() {
                    Ok(event) => state.apply(event)?,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        state.apply(InputEvent::Focus(false))?;
                        break;
                    }
                }
            }
            *self
                .snapshot
                .lock()
                .map_err(|_| "input snapshot poisoned")? = Arc::new(state.clone());
            Ok(())
        }
        fn shutdown(&mut self, w: &mut World) -> PluginResult {
            self.rx.take();
            if self.owns {
                w.remove::<InputState>();
                self.owns = false;

                let mut cleared = InputState::default();
                cleared.apply(InputEvent::Focus(false))?;
                *self
                    .snapshot
                    .lock()
                    .map_err(|_| "input snapshot poisoned")? = Arc::new(cleared);
            }
            Ok(())
        }
    }
}
#[cfg(feature = "plugin")]
pub use integration::{InputClient, InputPlugin};
