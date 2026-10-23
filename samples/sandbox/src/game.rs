//! Sample composition owns both plugins on the simulation thread.
use crate::materials::{palette, AIR, SAND, WALL};
use charon_platformer::Body;
use charon_runtime::Simulation;
use charon_sand::{Command as SandCommand, SandWorld};
#[derive(Clone, Copy, Debug)]
pub enum Command {
    Terrain(SandCommand),
    Left(bool),
    Right(bool),
    Jump,
    Dig(bool),
    ToggleSand,
    Pause,
    Step,
    Inspect(Option<(i32, i32)>),
}
#[derive(Clone, Copy, Debug)]
pub struct Inspection {
    pub x: i32,
    pub y: i32,
    pub material: Option<u8>,
    pub awake: Option<bool>,
}
pub struct Frame {
    pub terrain: charon_sand::Frame,
    pub player: Option<Body>,
    pub sand_solid: bool,
    pub paused: bool,
    pub simulation_tick: u64,
    pub inspection: Option<Inspection>,
}
pub struct Game {
    world: SandWorld,
    player: Option<Body>,
    sand_solid: bool,
    left: bool,
    right: bool,
    dig: bool,
    width: usize,
    height: usize,
    workers: usize,
    recorder: Option<crate::trace::Recorder>,
    paused: bool,
    jump_pending: bool,
    inspection: Option<(i32, i32)>,
}
impl Game {
    pub fn new(width: usize, height: usize, workers: usize, play: bool) -> Result<Self, String> {
        if play && (!(160..=1920).contains(&width) || !(120..=1080).contains(&height)) {
            return Err("play mode needs a grid of 160..1920 by 120..1080".into());
        }
        let world = if play {
            let mut cells = vec![AIR; width * height];
            for y in 0..height {
                cells[y * width] = WALL;
                cells[y * width + width - 1] = WALL;
            }
            cells[(height - 1) * width..].fill(WALL);
            let floor = height * 2 / 3;
            for y in floor - 6..floor {
                cells[y * width + width / 8..y * width + width * 7 / 8].fill(SAND);
            }
            for y in floor..floor + 4 {
                cells[y * width + width / 8..y * width + width * 7 / 8].fill(WALL);
            }
            SandWorld::from_material_cells(width, height, workers, cells, palette())?
        } else {
            SandWorld::with_materials(width, height, workers, palette())?
        };
        Ok(Self {
            world,
            player: play
                .then(|| Body::new(width as f32 / 4.0, (height * 2 / 3 - 18) as f32, 7.0, 12.0)),
            sand_solid: true,
            left: false,
            right: false,
            dig: false,
            width,
            height,
            workers,
            recorder: None,
            paused: false,
            jump_pending: false,
            inspection: None,
        })
    }
    pub fn record_to(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.recorder = Some(crate::trace::Recorder::new(
            path,
            self.width,
            self.height,
            self.workers,
            self.player.is_some(),
            self.material_fingerprint(),
        )?);
        Ok(())
    }
    pub fn material_fingerprint(&self) -> u64 {
        self.world.materials().fingerprint()
    }
    pub fn has_material(&self, id: u8) -> bool {
        self.world.materials().get(id).is_some()
    }
    pub fn signature(&self) -> u64 {
        let mut hash = self.world.checksum();
        let mut mix = |value: u64| {
            hash = (hash ^ value).wrapping_mul(0x100000001b3);
        };
        mix(u64::from(self.sand_solid));
        mix(u64::from(self.left));
        mix(u64::from(self.right));
        mix(u64::from(self.dig));
        if let Some(player) = self.player {
            mix(u64::from(player.x().to_bits()));
            mix(u64::from(player.y().to_bits()));
            mix(u64::from(player.vertical_velocity().to_bits()));
            mix(u64::from(player.grounded));
            mix(u64::from(player.embedded));
        }
        // Preserve signatures for existing unpaused v1 traces.
        if self.paused || self.jump_pending {
            mix(0x6465627567);
            mix(u64::from(self.paused));
            mix(u64::from(self.jump_pending));
            mix(self.world.tick());
        }
        hash
    }
    fn advance(&mut self, mut terrain: Vec<SandCommand>) -> charon_sand::Frame {
        if self.dig {
            if let Some(player) = self.player {
                for x in [
                    player.x().floor() as usize + 3,
                    (player.x() + player.width()).ceil() as usize - 4,
                ] {
                    terrain.push(SandCommand::Paint {
                        x,
                        y: (player.y() + player.height()).floor() as usize + 3,
                        material: AIR,
                    });
                }
            }
        }
        let frame = self.world.step(terrain);
        if let Some(player) = &mut self.player {
            let world = &self.world;
            let sand_solid = self.sand_solid;
            player.step(
                i8::from(self.right) - i8::from(self.left),
                self.jump_pending,
                |x, y| match world.material_at(x, y) {
                    Some(id) => {
                        world.materials().get(id).is_some_and(|m| m.blocks_actors)
                            && (id != SAND || sand_solid)
                    }
                    None => true,
                },
            );
        }
        self.jump_pending = false;
        frame
    }
}
impl Simulation for Game {
    type Command = Command;
    type Frame = Frame;
    fn step(&mut self, commands: Vec<Command>) -> Frame {
        let recorded = self.recorder.as_ref().map(|_| commands.clone());
        let mut terrain = Vec::new();
        let mut steps = 0;
        for command in commands {
            match command {
                Command::Terrain(SandCommand::Clear) if self.player.is_some() => {
                    let recorder = self.recorder.take();
                    let paused = self.paused;
                    let inspection = self.inspection;
                    *self = Self::new(self.width, self.height, self.workers, true)
                        .expect("validated scene dimensions");
                    self.recorder = recorder;
                    self.paused = paused;
                    self.inspection = inspection;
                    terrain.clear();
                    steps = 0;
                }
                Command::Terrain(command) => terrain.push(command),
                Command::Left(down) => self.left = down,
                Command::Right(down) => self.right = down,
                Command::Jump => self.jump_pending = true,
                Command::Dig(down) => self.dig = down,
                Command::ToggleSand => self.sand_solid = !self.sand_solid,
                Command::Pause => self.paused = !self.paused,
                Command::Step => {
                    self.paused = true;
                    steps += 1;
                }
                Command::Inspect(point) => self.inspection = point,
            }
        }
        let advances = if steps > 0 {
            steps
        } else {
            usize::from(!self.paused)
        };
        let frame = if advances == 0 {
            self.world.edit(terrain)
        } else {
            let mut frame = self.advance(terrain);
            for _ in 1..advances {
                frame = self.advance(vec![]);
            }
            frame
        };
        if let Some(commands) = recorded {
            let signature = self.signature();
            // A recording failure stops the simulation; the app reports the stopped thread.
            self.recorder
                .as_mut()
                .expect("recorder survives reset")
                .append(signature, &commands)
                .expect("input trace write failed");
        }
        Frame {
            terrain: frame,
            player: self.player,
            sand_solid: self.sand_solid,
            paused: self.paused,
            simulation_tick: self.world.tick(),
            inspection: self.inspection.map(|(x, y)| Inspection {
                x,
                y,
                material: self.world.material_at(x, y),
                awake: self.world.awake_at(x, y),
            }),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn game_defined_powder_uses_its_own_actor_policy_and_render_color() {
        use crate::materials::ASH;
        let mut game = Game::new(160, 120, 4, true).unwrap();
        let commands = vec![
            Command::Pause,
            Command::Terrain(SandCommand::Paint {
                x: 43,
                y: 77,
                material: ASH,
            }),
        ];
        let frame = game.step(commands);
        let material = frame.terrain.materials.get(ASH).unwrap();
        assert_eq!(material.key, "demo.ash");
        assert!(!material.blocks_actors);
        assert!(material.blocks_projectiles);
        let pixels = frame.terrain.image.to_rgba();
        assert_eq!(&pixels[(74 * 160 + 40) * 4..][..4], &material.color);
        game.step(vec![Command::Step]);
        assert!(
            game.player.unwrap().y() > 62.0,
            "actor did not pass through custom powder"
        );
        let reset = game.step(vec![Command::Terrain(SandCommand::Clear)]);
        assert_eq!(reset.terrain.materials.id("demo.ash"), Some(ASH));
    }

    #[test]
    fn pause_edits_inspection_and_each_step_preserve_the_physics_clock() {
        let mut game = Game::new(96, 96, 4, false).unwrap();
        let paint = SandCommand::Paint {
            x: 31,
            y: 10,
            material: SAND,
        };
        let frame = game.step(vec![
            Command::Pause,
            Command::Terrain(paint),
            Command::Inspect(Some((31, 7))),
        ]);
        assert!(frame.paused);
        assert_eq!(frame.simulation_tick, 0);
        assert_eq!(frame.inspection.unwrap().material, Some(SAND));
        assert_eq!(frame.inspection.unwrap().awake, Some(true));
        let before = game.world.checksum();
        for _ in 0..60 {
            let idle = game.step(vec![]);
            assert_eq!(idle.simulation_tick, 0);
            assert_eq!(game.world.checksum(), before);
            assert_eq!(idle.terrain.converted_cells, 0);
            assert_eq!(idle.terrain.workers_used, 0);
        }
        let mut reference = Game::new(96, 96, 1, false).unwrap();
        reference.step(vec![Command::Terrain(paint)]);
        let stepped = game.step(vec![Command::Step]);
        assert!(stepped.paused);
        assert_eq!(stepped.simulation_tick, 1);
        assert_eq!(game.world.checksum(), reference.world.checksum());
        let stepped = game.step(vec![Command::Step, Command::Step]);
        reference.step(vec![]);
        reference.step(vec![]);
        assert_eq!(stepped.simulation_tick, 3);
        assert_eq!(game.world.checksum(), reference.world.checksum());
        let outside = game.step(vec![Command::Inspect(Some((-1, 0)))]);
        assert!(outside.inspection.unwrap().material.is_none());
        assert_eq!(outside.simulation_tick, 3);
        let resumed = game.step(vec![Command::Pause, Command::Inspect(None)]);
        assert!(!resumed.paused);
        assert_eq!(resumed.simulation_tick, 4);
        assert!(resumed.inspection.is_none());
    }
    #[test]
    fn paused_jump_is_consumed_once_and_reset_stays_paused() {
        let mut game = Game::new(160, 120, 2, true).unwrap();
        game.step(vec![]);
        let start = game.step(vec![Command::Pause]).player.unwrap();
        for _ in 0..30 {
            game.step(vec![Command::Right(true), Command::Jump]);
        }
        assert_eq!(game.player.unwrap().x(), start.x());
        assert_eq!(game.player.unwrap().y(), start.y());
        let one = game.step(vec![Command::Step]).player.unwrap();
        let two = game.step(vec![Command::Step]).player.unwrap();
        assert!(one.y() < start.y());
        assert!(
            two.vertical_velocity() > one.vertical_velocity(),
            "jump repeated without a new press"
        );
        let reset = game.step(vec![Command::Terrain(SandCommand::Clear)]);
        assert!(reset.paused);
        assert_eq!(reset.simulation_tick, 0);
        let original = game.world.checksum();
        game.step(vec![Command::Dig(true)]);
        assert_eq!(
            game.world.checksum(),
            original,
            "held dig advanced while paused"
        );
        assert_eq!(game.world.tick(), 0);
        let frame = game.step(vec![Command::Step]);
        assert_ne!(game.world.checksum(), original);
        assert_eq!(frame.simulation_tick, 1);
        assert!(frame.player.unwrap().y() > reset.player.unwrap().y());
    }

    #[test]
    fn sand_policy_dig_reset_and_parallel_terrain_compose() {
        assert!(Game::new(usize::MAX, 270, 4, true).is_err());
        assert!(Game::new(480, usize::MAX, 4, true).is_err());
        let mut game = Game::new(480, 270, 4, true).unwrap();
        assert_eq!(game.world.material_at(-1, 0), None);
        assert_eq!(game.world.material_at(480, 0), None);
        assert_eq!(game.world.material_at(0, 270), None);
        assert_eq!(game.world.material_at(0, 0), Some(WALL));
        assert_eq!(game.world.material_at(120, 174), Some(SAND));
        let top = game.step(vec![]).player.unwrap();
        assert!(top.grounded);
        assert_eq!(top.y(), 162.0);
        let mut frame = game.step(vec![Command::ToggleSand]);
        for _ in 0..50 {
            frame = game.step(vec![]);
        }
        assert!(frame.player.unwrap().grounded);
        assert_eq!(frame.player.unwrap().y(), 168.0);
        game.step(vec![Command::Dig(true)]);
        for _ in 0..150 {
            frame = game.step(vec![]);
        }
        assert!(frame.player.unwrap().grounded);
        assert_eq!(frame.player.unwrap().y(), 257.0);
        let reset = game.step(vec![Command::Terrain(SandCommand::Clear)]);
        assert_eq!(reset.player.unwrap().y(), 162.0);
        assert!(reset.sand_solid);
        let start = reset.player.unwrap().x();
        for _ in 0..20 {
            frame = game.step(vec![Command::Right(true)]);
        }
        assert!(frame.player.unwrap().x() > start);
        game.step(vec![Command::Right(false)]);
        let x = game.step(vec![]).player.unwrap().x();
        assert_eq!(game.step(vec![]).player.unwrap().x(), x);
    }
}
