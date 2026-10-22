//! Sample composition owns both plugins on the simulation thread.
use charon_platformer::Body;
use charon_runtime::Simulation;
use charon_sand::{Command as SandCommand, SandWorld, AIR, SAND, WALL};
#[derive(Clone, Copy, Debug)]
pub enum Command {
    Terrain(SandCommand),
    Left(bool),
    Right(bool),
    Jump,
    Dig(bool),
    ToggleSand,
}
pub struct Frame {
    pub terrain: charon_sand::Frame,
    pub player: Option<Body>,
    pub sand_solid: bool,
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
            SandWorld::from_cells(width, height, workers, cells)?
        } else {
            SandWorld::new(width, height, workers)?
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
        })
    }
    pub fn record_to(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.recorder = Some(crate::trace::Recorder::new(
            path,
            self.width,
            self.height,
            self.workers,
            self.player.is_some(),
        )?);
        Ok(())
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
        hash
    }
}
impl Simulation for Game {
    type Command = Command;
    type Frame = Frame;
    fn step(&mut self, commands: Vec<Command>) -> Frame {
        let recorded = self.recorder.as_ref().map(|_| commands.clone());
        let mut terrain = Vec::new();
        let mut jump = false;
        for command in commands {
            match command {
                Command::Terrain(SandCommand::Clear) if self.player.is_some() => {
                    let recorder = self.recorder.take();
                    *self = Self::new(self.width, self.height, self.workers, true)
                        .expect("validated scene dimensions");
                    self.recorder = recorder;
                    terrain.clear();
                    jump = false;
                }
                Command::Terrain(command) => terrain.push(command),
                Command::Left(down) => self.left = down,
                Command::Right(down) => self.right = down,
                Command::Jump => jump = true,
                Command::Dig(down) => self.dig = down,
                Command::ToggleSand => self.sand_solid = !self.sand_solid,
            }
        }
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
                jump,
                |x, y| match world.material_at(x, y) {
                    Some(AIR) => false,
                    Some(SAND) => sand_solid,
                    _ => true,
                },
            );
        }
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
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
