//! Game-owned cup geometry, speed and input policy, using reusable boundaries.
use crate::materials::{AIR, OIL, WALL, WATER};
use charon_sand::{Command, ObstacleId, SandWorld};
#[derive(Clone, Copy, Debug)]
pub struct Status {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub moves: u64,
    pub blocked: u64,
    pub water: usize,
    pub oil: usize,
}
pub struct Cup {
    id: ObstacleId,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    phase: u8,
    moves: u64,
    blocked: u64,
    error: Option<&'static str>,
}
impl Cup {
    pub fn create(
        world: &mut SandWorld,
        width: usize,
        height: usize,
    ) -> Result<Self, &'static str> {
        if width < 128 || height < 100 {
            return Err("CUP NEEDS GRID 128X100");
        }
        let cw = (width / 4).clamp(24, 96) as i32;
        let ch = (height / 4).clamp(20, 64) as i32;
        let x = (width as i32 - cw) / 2;
        let y = height as i32 / 2;
        world.apply_edits(vec![Command::Clear]);
        let mask: Vec<_> = (0..ch)
            .flat_map(|py| {
                (0..cw).filter_map(move |px| {
                    (px < 2 || px >= cw - 2 || py >= ch - 2).then_some([x + px, y + py])
                })
            })
            .collect();
        let id = world
            .add_obstacle(&mask, WALL)
            .map_err(|_| "CUP COULD NOT BE CREATED")?;
        world.apply_edits(vec![Command::FillRect {
            x: (x + 2) as usize,
            y: (y + ch / 2 - 2) as usize,
            width: (cw - 4) as usize,
            height: (ch / 2) as usize,
            material: WATER,
        }]);
        Ok(Self {
            id,
            x,
            y,
            width: cw,
            height: ch,
            phase: 0,
            moves: 0,
            blocked: 0,
            error: None,
        })
    }
    pub fn add_oil(&self, world: &mut SandWorld) {
        // Inject only into air near the mouth; never paint over existing water/oil.
        let mut edits = Vec::new();
        for y in self.y + 3..self.y + 3 + (self.height / 8).max(1) {
            for x in self.x + 2..self.x + self.width - 2 {
                if world.material_at(x, y) == Some(AIR) {
                    edits.push(Command::FillRect {
                        x: x as usize,
                        y: y as usize,
                        width: 1,
                        height: 1,
                        material: OIL,
                    });
                }
            }
        }
        world.apply_edits(edits);
    }
    pub fn step(&mut self, world: &mut SandWorld, direction: i8) -> Option<&'static str> {
        if direction == 0 {
            self.phase = 0;
            self.error = None;
            return self.error;
        }
        self.phase = (self.phase + 1) % 4;
        if self.phase != 0 {
            return self.error;
        }
        match world.move_obstacle(self.id, [direction as i32, 0]) {
            Ok(()) => {
                self.x += direction as i32;
                self.moves += 1;
                self.error = None;
                self.error
            }
            Err(error) => {
                self.blocked += 1;
                eprintln!("Cup move rejected: {error:?}");
                self.error = Some("CUP BLOCKED - LIQUID PRESERVED");
                self.error
            }
        }
    }
    pub fn status(&self, world: &SandWorld) -> Status {
        Status {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            moves: self.moves,
            blocked: self.blocked,
            water: world.material_count(WATER),
            oil: world.material_count(OIL),
        }
    }
    pub fn signature(&self) -> [u64; 4] {
        [self.x as u64, self.y as u64, self.phase as u64, self.moves]
    }
}
