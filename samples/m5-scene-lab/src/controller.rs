use charon_camera::{Camera2D, Rect};
use charon_core::{Plugin, PluginResult, Step, World};
use charon_input::{Button, InputState, Key, keys};
use charon_m3_sample::runtime::{Controls, Edit};
use charon_scene::SceneId;
use charon_tilemap::TileId;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, mpsc::SyncSender},
};
#[derive(Clone, Copy, Debug)]
pub enum Action {
    Menu,
    Restart,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Flags {
    pub enabled: bool,
    pub torch: bool,
    pub ore: bool,
    pub ao: bool,
    pub sky: bool,
    pub quality: bool,
}
impl Default for Flags {
    fn default() -> Self {
        Self {
            enabled: true,
            torch: true,
            ore: true,
            ao: true,
            sky: true,
            quality: false,
        }
    }
}
pub type EditSink = Arc<Mutex<Option<SyncSender<Edit>>>>;
pub fn viewport(size: [u32; 2]) -> Rect {
    Rect {
        x: 0.,
        y: 72.,
        width: size[0].max(1) as f32,
        height: size[1].saturating_sub(120).max(1) as f32,
    }
}
pub fn camera(size: [u32; 2]) -> Camera2D {
    let mut camera = Camera2D::new([160., 96.], 3., viewport(size)).unwrap();
    camera
        .fit(Rect {
            x: 0.,
            y: 0.,
            width: 320.,
            height: 192.,
        })
        .unwrap();
    camera
}
pub struct Controller {
    pub id: SceneId,
    #[cfg(feature = "bridge")]
    pub coupled: bool,
    pub controls: Arc<Mutex<Controls>>,
    pub view: Arc<Mutex<Arc<charon_m3_sample::runtime::View>>>,
    #[cfg(feature = "bridge")]
    pub player: Arc<Mutex<charon_m3_sample::runtime::Player>>,
    pub mirror: Arc<Mutex<Camera2D>>,
    pub flags: Arc<Mutex<Flags>>,
    pub selected: Arc<Mutex<TileId>>,
    pub edits: EditSink,
    pub actions: SyncSender<(SceneId, Action)>,
    pending: VecDeque<Edit>,
}
pub struct ControllerInit {
    pub id: SceneId,
    pub coupled: bool,
    pub mirror: Arc<Mutex<Camera2D>>,
    pub flags: Arc<Mutex<Flags>>,
    pub selected: Arc<Mutex<TileId>>,
    pub edits: EditSink,
    pub actions: SyncSender<(SceneId, Action)>,
}
impl Controller {
    pub fn new(shared: &charon_m3_sample::runtime::Shared, init: ControllerInit) -> Self {
        #[cfg(not(feature = "bridge"))]
        let _ = init.coupled;
        Self {
            id: init.id,
            #[cfg(feature = "bridge")]
            coupled: init.coupled,
            controls: shared.controls.clone(),
            view: shared.view.clone(),
            #[cfg(feature = "bridge")]
            player: shared.player.clone(),
            mirror: init.mirror,
            flags: init.flags,
            selected: init.selected,
            edits: init.edits,
            actions: init.actions,
            pending: VecDeque::new(),
        }
    }
}
impl Plugin for Controller {
    fn name(&self) -> &'static str {
        "scene-controls"
    }
    fn dependencies(&self) -> &[&'static str] {
        &["input", "camera"]
    }
    fn fixed_update(&mut self, w: &mut World, step: Step) -> PluginResult {
        let input = w.get::<InputState>().ok_or("input missing")?.clone();
        let key = |c: char| Key(c as u32);
        if !input.focused {
            *self.controls.lock().unwrap() = Controls::default();
            return Ok(());
        }
        let held = |b| input.held(b) as i8;
        *self.controls.lock().unwrap() = Controls {
            dx: (held(key('D')) - held(key('A'))) as f32,
            dy: (held(key('S')) - held(key('W'))) as f32,
        };
        let cam = w.get_mut::<Camera2D>().ok_or("camera missing")?;
        if input.size[0] > 0 && input.size[1] > 120 {
            cam.set_viewport(viewport(input.size))?;
        }
        cam.pan([
            (held(keys::RIGHT) - held(keys::LEFT)) as f32 * 90. * step.delta.as_secs_f32(),
            (held(keys::DOWN) - held(keys::UP)) as f32 * 90. * step.delta.as_secs_f32(),
        ])?;
        let zoom = (held(key('E')) - held(key('Q'))) as f32 * step.delta.as_secs_f32() * 1.5
            + input.wheel[1] * 0.15;
        if zoom != 0. {
            let anchor = input
                .pointer
                .filter(|&p| cam.viewport().contains(p))
                .unwrap_or(cam.viewport().center());
            cam.zoom_at(cam.zoom() * zoom.clamp(-5., 5.).exp(), anchor)?;
        }
        if input.pressed(keys::SPACE) {
            cam.fit(Rect {
                x: 0.,
                y: 0.,
                width: 320.,
                height: 192.,
            })?;
        }
        *self.mirror.lock().unwrap() = *cam;
        if input.pressed(keys::TAB) || input.pressed(keys::ESCAPE) {
            self.actions
                .try_send((self.id, Action::Menu))
                .map_err(|e| e.to_string())?;
        }
        if input.pressed(key('R')) {
            self.actions
                .try_send((self.id, Action::Restart))
                .map_err(|e| e.to_string())?;
        }
        for (c, id) in [('1', 0), ('2', 1), ('3', 2)] {
            if input.pressed(key(c)) {
                *self.selected.lock().unwrap() = TileId(id);
            }
        }
        {
            let mut guard = self.flags.lock().unwrap();
            let flags = &mut *guard;
            for (c, value) in [
                ('L', &mut flags.enabled),
                ('T', &mut flags.torch),
                ('O', &mut flags.ore),
                ('V', &mut flags.ao),
                ('K', &mut flags.sky),
                ('Z', &mut flags.quality),
            ] {
                if input.pressed(key(c)) {
                    *value = !*value;
                }
            }
        }
        let view = self.view.lock().unwrap().clone();
        if input.pressed(key('B')) {
            for (x, y, id) in view.map.occupied() {
                if id == TileId(1) || id == TileId(2) {
                    self.pending.push_back(Edit::Break(x, y));
                }
            }
        }
        if input.pressed(key('G')) {
            for y in 14..17 {
                self.pending.push_back(if view.map.get(19, y).is_some() {
                    Edit::Break(19, y)
                } else {
                    Edit::Place(19, y, TileId(0))
                });
            }
        }
        #[cfg(feature = "bridge")]
        if self.coupled && input.pressed(key('F')) {
            let player = *self.player.lock().unwrap();
            self.pending.push_back(Edit::Paint {
                x: player.position[0] as i32 + 12,
                y: player.position[1] as i32,
                radius: 2,
                cell: charon_physics::sim::material::CellData::fire(120),
            });
        }
        if let Some(p) = input.pointer.and_then(|p| cam.pick(p)) {
            let (x, y) = ((p[0] / 8.).floor(), (p[1] / 8.).floor());
            if x >= 0. && y >= 0. && x < 40. && y < 24. {
                if input.held(Button::Mouse(0)) || input.pressed(Button::Mouse(0)) {
                    self.pending.push_back(Edit::Break(x as u32, y as u32));
                } else if input.held(Button::Mouse(1)) || input.pressed(Button::Mouse(1)) {
                    self.pending.push_back(Edit::Place(
                        x as u32,
                        y as u32,
                        *self.selected.lock().unwrap(),
                    ));
                }
            }
        }
        if self.pending.len() > 256 {
            return Err("edit backlog exceeded, release input".into());
        }
        if let Some(tx) = self.edits.lock().unwrap().as_ref() {
            while let Some(edit) = self.pending.pop_front() {
                match tx.try_send(edit) {
                    Ok(()) => {}
                    Err(std::sync::mpsc::TrySendError::Full(e)) => {
                        self.pending.push_front(e);
                        break;
                    }
                    Err(e) => return Err(e.to_string()),
                }
            }
        }
        Ok(())
    }
}
