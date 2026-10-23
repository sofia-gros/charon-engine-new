//! Sample composition owns both plugins on the simulation thread.
use crate::materials::{palette, AIR, SAND, WALL};
use charon_platformer::Body;
use charon_projectiles::{Projectiles, Shot};
use charon_runtime::Simulation;
use charon_sand::{Command as SandCommand, SandWorld};
#[derive(Clone, Copy, Debug)]
pub enum Command {
    Terrain(SandCommand),
    CupScene,
    DamScene,
    AddOil,
    Explore(u64),
    NewRun(u64),
    Begin,
    PlaceImage([f32; 2]),
    EraseImage([f32; 2]),
    Left(bool),
    Right(bool),
    Jump,
    Fire,
    FireAt([f32; 2]),
    Dig(bool),
    ToggleSand,
    Pause,
    Step,
    Inspect(Option<(i32, i32)>),
}
/// Shared by keyboard and focus-loss handling in the window client.
pub fn submit_input<S: Simulation<Command = Command>>(
    runtime: &charon_runtime::Runtime<S>,
    command: Command,
) -> Result<(), std::sync::mpsc::TrySendError<Command>> {
    match command {
        Command::Left(_) => runtime.submit_state(0, command),
        Command::Right(_) => runtime.submit_state(1, command),
        Command::Dig(_) => runtime.submit_state(2, command),
        Command::Inspect(_) => runtime.submit_state(3, command),
        Command::Pause | Command::Step => runtime.submit_control(command),
        _ => runtime.submit(command),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Inspection {
    pub x: i32,
    pub y: i32,
    pub material: Option<u8>,
    pub awake: Option<bool>,
}
pub struct Frame {
    pub combat: Option<crate::combat::Frame>,
    pub exploration_seed: Option<u64>,
    pub cup: Option<crate::cup::Status>,
    pub terrain: charon_sand::Frame,
    pub images: Vec<[f32; 2]>,
    pub image_error: Option<&'static str>,
    pub shots: Vec<[f32; 2]>,
    pub sparks: Vec<[f32; 2]>,
    pub impacts: u64,
    pub player: Option<Body>,
    pub previous_player: Option<[f32; 2]>,
    pub direction: i8,
    pub facing: i8,
    pub sand_solid: bool,
    pub paused: bool,
    pub simulation_tick: u64,
    pub inspection: Option<Inspection>,
}
impl Frame {
    pub fn state_label(&self) -> &'static str {
        match self.combat.as_ref().map(|c| c.phase) {
            Some(crate::combat::Phase::Ready) => "READY",
            Some(crate::combat::Phase::Defeated) => "DEFEATED",
            _ if self.paused => "PAUSED",
            _ => "RUNNING",
        }
    }
    pub fn player_position(&self, elapsed: std::time::Duration) -> Option<[f32; 2]> {
        let body = self.player?;
        let current = [body.x(), body.y()];
        Some(
            if self.paused
                || body.discontinuous
                || body.embedded
                || self
                    .combat
                    .as_ref()
                    .is_some_and(|c| c.phase != crate::combat::Phase::Playing)
            {
                current
            } else {
                self.previous_player.map_or(current, |previous| {
                    charon_runtime::interpolate(
                        previous,
                        current,
                        elapsed,
                        std::time::Duration::from_secs_f64(1. / 60.),
                    )
                })
            },
        )
    }
}
struct Spark {
    position: [f32; 2],
    velocity: [f32; 2],
    life: u8,
}
pub struct Game {
    combat: Option<crate::combat::Combat>,
    exploration_seed: Option<u64>,
    cup: Option<crate::cup::Cup>,
    world: SandWorld,
    images: Vec<[f32; 2]>,
    image_error: Option<&'static str>,
    projectiles: Projectiles,
    sparks: Vec<Spark>,
    fire_pending: bool,
    fire_target: Option<[f32; 2]>,
    impacts: u64,
    player: Option<Body>,
    previous_player: Option<[f32; 2]>,
    facing: i8,
    presentation_direction: i8,
    sand_solid: bool,
    left: bool,
    right: bool,
    dig: bool,
    width: usize,
    height: usize,
    workers: usize,
    recorder: Option<crate::trace::Recorder>,
    fluid_gpu: Option<std::sync::Arc<charon_fluid::gpu::GpuContext>>,
    paused: bool,
    jump_pending: bool,
    inspection: Option<(i32, i32)>,
}
impl Game {
    pub fn new(width: usize, height: usize, workers: usize, play: bool) -> Result<Self, String> {
        Self::with_materials(width, height, workers, play, palette())
    }
    pub fn with_materials(
        width: usize,
        height: usize,
        workers: usize,
        play: bool,
        materials: charon_sand::Materials,
    ) -> Result<Self, String> {
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
            SandWorld::from_material_cells(width, height, workers, cells, materials)?
        } else {
            SandWorld::with_materials(width, height, workers, materials)?
        };
        Ok(Self {
            combat: None,
            exploration_seed: None,
            cup: None,
            images: vec![],
            image_error: None,
            previous_player: None,
            facing: 1,
            presentation_direction: 0,
            world,
            projectiles: Projectiles::default(),
            sparks: vec![],
            fire_pending: false,
            fire_target: None,
            impacts: 0,
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
            fluid_gpu: None,
            paused: false,
            jump_pending: false,
            inspection: None,
        })
    }
    fn restart(&mut self, seed: Option<u64>) -> Result<(), String> {
        let mut next = Self::with_materials(
            self.width,
            self.height,
            self.workers,
            true,
            self.world.materials().clone(),
        )?;
        if let Some(seed) = seed {
            let (cells, spawn) = crate::scenes::cave(self.width, self.height, seed);
            next.world = SandWorld::from_material_cells(
                self.width,
                self.height,
                self.workers,
                cells,
                self.world.materials().clone(),
            )?;
            next.player = Some(Body::new(spawn[0], spawn[1], 7., 12.));
            next.exploration_seed = Some(seed);
            if self.combat.is_some() {
                next.prepare_combat(false);
            }
        }
        next.recorder = self.recorder.take();
        next.fluid_gpu = self.fluid_gpu.clone();
        next.paused = self.paused;
        next.inspection = self.inspection;
        *self = next;
        Ok(())
    }
    fn prepare_combat(&mut self, ready: bool) {
        let x = self.width * 3 / 5;
        self.world.apply_edits(vec![SandCommand::FillRect {
            x: x - 2,
            y: self.height * 2 / 3 - 16,
            width: 12,
            height: 16,
            material: AIR,
        }]);
        self.combat = Some(crate::combat::Combat::new(self.width, self.height, ready));
    }
    pub fn use_fluid_gpu(&mut self, gpu: std::sync::Arc<charon_fluid::gpu::GpuContext>) {
        self.fluid_gpu = Some(gpu);
    }
    pub fn fluid_backend(&self) -> String {
        self.fluid_gpu
            .as_ref()
            .map_or_else(|| "cpu".into(), |g| g.identity())
    }
    pub fn record_to(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.recorder = Some(crate::trace::Recorder::with_backend(
            path,
            self.width,
            self.height,
            self.workers,
            self.player.is_some(),
            self.material_fingerprint(),
            &self.fluid_backend(),
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
        if let Some(combat) = &self.combat {
            mix(0x636f6d626174);
            for value in combat.signature() {
                mix(value);
            }
        }
        if let Some(seed) = self.exploration_seed {
            mix(0x63617665);
            mix(seed);
        }
        if let Some(cup) = &self.cup {
            mix(0x637570);
            for value in cup.signature() {
                mix(value);
            }
        }
        if !self.images.is_empty() {
            mix(0x696d616765);
            mix(self.images.len() as u64);
            for position in &self.images {
                for value in position {
                    mix(u64::from(value.to_bits()));
                }
            }
        }
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
        if let Some(target) = self.fire_target {
            mix(0x61696d);
            for value in target {
                mix(u64::from(value.to_bits()));
            }
        }
        // Only authoritative projectile state enters replay checks; cosmetic sparks
        // have no collision and cannot alter terrain or actors.
        if self.fire_pending || self.impacts != 0 || !self.projectiles.shots().is_empty() {
            mix(0x73686f7473);
            mix(u64::from(self.fire_pending));
            mix(self.impacts);
            for shot in self.projectiles.shots() {
                for value in shot.position.into_iter().chain(shot.velocity) {
                    mix(u64::from(value.to_bits()));
                }
                mix(u64::from(shot.remaining_ticks));
            }
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
        self.previous_player = self.player.map(|p| [p.x(), p.y()]);
        self.presentation_direction = i8::from(self.right) - i8::from(self.left);
        if self.presentation_direction != 0 {
            self.facing = self.presentation_direction;
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
        let mut frame = if let Some(cup) = &mut self.cup {
            let start = std::time::Instant::now();
            self.world.apply_edits(terrain);
            if let Some(error) = cup.step(&mut self.world, self.presentation_direction) {
                self.image_error = Some(error);
            }
            let coupling_ms = start.elapsed().as_secs_f64() * 1000.;
            let mut frame = self.world.step(vec![]);
            frame.update_ms += coupling_ms;
            frame
        } else {
            self.world.step(terrain)
        };
        self.sparks.retain_mut(|spark| {
            for axis in 0..2 {
                spark.position[axis] += spark.velocity[axis];
            }
            spark.velocity[1] += 0.04;
            spark.life -= 1;
            spark.life > 0
        });
        if self.fire_pending {
            let direction = if self.exploration_seed.is_some() {
                self.facing as f32
            } else {
                1.
            };
            let position =
                self.player
                    .map_or([self.width as f32 / 4., self.height as f32 / 4.], |p| {
                        let side = self.fire_target.map_or(direction, |target| {
                            if target[0] < p.x() + p.width() / 2. {
                                -1.
                            } else {
                                1.
                            }
                        });
                        [
                            if side < 0. {
                                p.x() - 0.5
                            } else {
                                p.x() + p.width() + 0.5
                            },
                            p.y() + p.height() / 2.,
                        ]
                    });
            let velocity = self.fire_target.map_or([12. * direction, 0.], |target| {
                let delta = [target[0] - position[0], target[1] - position[1]];
                let length = delta[0].hypot(delta[1]);
                if length < 0.001 {
                    [12. * direction, 0.]
                } else {
                    delta.map(|v| v / length * 12.)
                }
            });
            self.projectiles
                .spawn(Shot {
                    position,
                    velocity,
                    remaining_ticks: 60,
                })
                .expect("one shot per tick with a 60 tick lifetime stays below capacity");
            self.fire_pending = false;
            self.fire_target = None;
        }
        let world = &self.world;
        let enemies = self
            .combat
            .as_ref()
            .map_or_else(Vec::new, |c| c.colliders());
        let impacts = self.projectiles.step(|x, y| {
            enemies.iter().any(|&r| crate::combat::contains(r, x, y))
                || world.material_at(x, y).is_none_or(|id| {
                    world
                        .materials()
                        .get(id)
                        .is_some_and(|m| m.blocks_projectiles)
                })
        });
        if !impacts.is_empty() {
            let mut destruction = Vec::new();
            for impact in impacts {
                self.impacts += 1;
                let terrain_hit = self
                    .world
                    .material_at(impact.cell[0], impact.cell[1])
                    .is_none_or(|id| {
                        self.world
                            .materials()
                            .get(id)
                            .is_some_and(|m| m.blocks_projectiles)
                    });
                let enemy_hit = !terrain_hit
                    && enemies
                        .iter()
                        .any(|&r| crate::combat::contains(r, impact.cell[0], impact.cell[1]));
                if enemy_hit {
                    if let Some(combat) = &mut self.combat {
                        combat.hit_enemy(impact.cell);
                    }
                } else {
                    destruction.push(SandCommand::Paint {
                        x: impact.cell[0].max(0) as usize,
                        y: impact.cell[1].max(0) as usize,
                        material: AIR,
                    });
                }
                // Sample visual effect, deliberately separate from physical cells.
                for velocity in [[-0.5, -0.8], [0.4, -0.9], [-0.8, -0.3], [0.7, -0.2]] {
                    self.sparks.push(Spark {
                        position: impact.position,
                        velocity,
                        life: 24,
                    });
                }
            }
            // Render destruction immediately without an extra physics tick. Preserve
            // the physics counters from the same step in the newly published frame.
            let edited = self.world.edit(destruction);
            frame.image = edited.image;
            frame.indices = edited.indices;
            frame.sand = edited.sand;
            frame.converted_cells += edited.converted_cells;
            frame.update_ms += edited.update_ms;
            frame.snapshot_ms += edited.snapshot_ms;
        }
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
        if let (Some(combat), Some(player)) = (&mut self.combat, self.player) {
            let world = &self.world;
            combat.tick(
                player,
                |x, y| {
                    world
                        .material_at(x, y)
                        .is_none_or(|id| world.materials().get(id).is_some_and(|m| m.blocks_actors))
                },
                |x, y| {
                    world.material_at(x, y).is_none_or(|id| {
                        world
                            .materials()
                            .get(id)
                            .is_some_and(|m| m.blocks_projectiles)
                    })
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
            if self
                .combat
                .as_ref()
                .is_some_and(|c| c.phase() != crate::combat::Phase::Playing)
                && !matches!(
                    command,
                    Command::NewRun(_)
                        | Command::Begin
                        | Command::Explore(_)
                        | Command::Terrain(SandCommand::Clear)
                        | Command::Inspect(_)
                        | Command::Pause
                        | Command::Step
                )
            {
                continue;
            }
            match command {
                Command::Terrain(SandCommand::Clear) if self.player.is_some() => {
                    self.restart(self.exploration_seed)
                        .expect("validated scene dimensions");
                    terrain.clear();
                    steps = 0;
                }
                Command::Begin => {
                    if let Some(combat) = &mut self.combat {
                        combat.begin();
                    }
                }
                command @ (Command::Explore(_) | Command::NewRun(_)) => {
                    let (seed, run) = match command {
                        Command::Explore(seed) => (seed, false),
                        Command::NewRun(seed) => (seed, true),
                        _ => unreachable!(),
                    };
                    match self.restart(Some(seed)) {
                        Ok(()) => {
                            self.combat = None;
                            if run {
                                self.prepare_combat(true);
                            }
                            terrain.clear();
                            steps = 0;
                        }
                        Err(_) => self.image_error = Some("EXPLORE NEEDS GRID 160X120 OR LARGER"),
                    }
                }
                command @ (Command::CupScene | Command::DamScene) => {
                    terrain.clear();
                    let create = if matches!(command, Command::CupScene) {
                        crate::cup::Cup::create
                    } else {
                        crate::cup::Cup::dam
                    };
                    match create(&mut self.world, self.width, self.height, self.workers) {
                        Ok(mut cup) => {
                            if let Some(gpu) = &self.fluid_gpu {
                                cup.use_gpu(gpu.clone());
                            }
                            self.combat = None;
                            self.exploration_seed = None;
                            self.cup = Some(cup);
                            self.player = None;
                            self.images.clear();
                            self.projectiles = Projectiles::default();
                            self.sparks.clear();
                            self.fire_pending = false;
                            self.fire_target = None;
                            self.impacts = 0;
                            self.left = false;
                            self.right = false;
                            self.image_error = None;
                        }
                        Err(error) => self.image_error = Some(error),
                    }
                }
                Command::AddOil => {
                    if let Some(cup) = &mut self.cup {
                        self.world.apply_edits(std::mem::take(&mut terrain));
                        cup.add_oil();
                    }
                }
                Command::Terrain(SandCommand::Clear) => {
                    self.cup = None;
                    self.images.clear();
                    self.image_error = None;
                    terrain.push(SandCommand::Clear);
                }
                Command::Terrain(_) if self.cup.is_some() => {
                    self.image_error = Some("CFD DEMO: R RESET BEFORE TERRAIN EDITS");
                }
                Command::Terrain(command) => terrain.push(command),
                Command::PlaceImage(position) => {
                    self.image_error = if position
                        .iter()
                        .any(|v| !v.is_finite() || v.abs() > 1_000_000.)
                    {
                        Some("INVALID IMAGE POSITION")
                    } else if self.images.len() >= 128 {
                        Some("IMAGE LIMIT 128")
                    } else {
                        self.images.push(position);
                        None
                    };
                }
                Command::EraseImage(point) => {
                    if let Some(index) = self.images.iter().rposition(|p| {
                        point[0] >= p[0]
                            && point[0] < p[0] + 8.
                            && point[1] >= p[1]
                            && point[1] < p[1] + 14.
                    }) {
                        self.images.remove(index);
                    }
                    self.image_error = None;
                }
                Command::Left(down) => self.left = down,
                Command::Right(down) => self.right = down,
                Command::Jump => self.jump_pending = true,
                Command::Fire => {
                    self.fire_pending = true;
                    self.fire_target = None;
                }
                Command::FireAt(target) => {
                    if target
                        .into_iter()
                        .all(|v| v.is_finite() && v.abs() <= 1_000_000.)
                    {
                        self.fire_pending = true;
                        self.fire_target = Some(target);
                    } else {
                        self.image_error = Some("INVALID AIM POSITION");
                    }
                }
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
        let advances = if self
            .combat
            .as_ref()
            .is_some_and(|c| c.phase() != crate::combat::Phase::Playing)
        {
            0
        } else if steps > 0 {
            steps
        } else {
            usize::from(!self.paused)
        };
        let mut frame = if advances == 0 {
            self.world.edit(terrain)
        } else {
            let mut frame = self.advance(terrain);
            for _ in 1..advances {
                if self
                    .combat
                    .as_ref()
                    .is_some_and(|c| c.phase() != crate::combat::Phase::Playing)
                {
                    break;
                }
                frame = self.advance(vec![]);
            }
            frame
        };
        if let Some(cup) = &mut self.cup {
            cup.compose(&mut frame);
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
            combat: self.combat.as_ref().map(|c| c.frame()),
            exploration_seed: self.exploration_seed,
            cup: self.cup.as_ref().map(|c| c.status(&self.world)),
            terrain: frame,
            images: self.images.clone(),
            image_error: self.image_error,
            shots: self
                .projectiles
                .shots()
                .iter()
                .map(|s| s.position)
                .collect(),
            sparks: self.sparks.iter().map(|s| s.position).collect(),
            impacts: self.impacts,
            player: self.player,
            previous_player: self.previous_player,
            direction: self.presentation_direction,
            facing: self.facing,
            sand_solid: self.sand_solid,
            paused: self.paused,
            simulation_tick: self.world.tick(),
            inspection: self.inspection.map(|(x, y)| Inspection {
                x,
                y,
                material: self
                    .cup
                    .as_ref()
                    .and_then(|c| c.material_at(x, y))
                    .or_else(|| self.world.material_at(x, y)),
                awake: self
                    .cup
                    .as_ref()
                    .filter(|c| c.material_at(x, y).is_some())
                    .map(|c| !c.status(&self.world).fluid.sleeping)
                    .or_else(|| self.world.awake_at(x, y)),
            }),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cfd_view_matches_both_image_planes_and_rejects_uncoupled_terrain_edits() {
        let mut game = Game::new(160, 120, 2, false).unwrap();
        let first = game.step(vec![Command::CupScene, Command::Pause]);
        let ids = first.terrain.indices.to_bytes();
        let rgba: Vec<_> = ids
            .iter()
            .flat_map(|&id| first.terrain.materials.get(id).unwrap().color)
            .collect();
        assert_eq!(first.terrain.image.to_rgba(), rgba);
        let cup = first.cup.unwrap();
        let signature = game.cup.as_ref().unwrap().signature();
        let frame = game.step(vec![Command::Terrain(SandCommand::FillRect {
            x: (cup.x + 3) as usize,
            y: (cup.y + cup.height - 4) as usize,
            width: 3,
            height: 1,
            material: WALL,
        })]);
        assert!(frame.image_error.is_some());
        assert_eq!(game.cup.as_ref().unwrap().signature(), signature);
        game.step(vec![Command::DamScene, Command::Pause]);
        assert_eq!(first.terrain.indices.to_bytes(), ids);
        assert_eq!(first.terrain.image.to_rgba(), rgba);
    }
    #[test]
    fn cup_shakes_with_immiscible_oil_preserves_counts_and_freezes_with_pause() {
        let mut game = Game::new(240, 160, 4, false).unwrap();
        let initial = game.step(vec![Command::CupScene]).cup.unwrap();
        let oil = game.step(vec![Command::AddOil]).cup.unwrap().oil;
        assert!(initial.water > 0 && oil > 0);
        let mut max_speed: f64 = 0.;
        for tick in 0..600 {
            let commands = match tick {
                0 => vec![Command::Right(true)],
                120 => vec![Command::Right(false), Command::Left(true)],
                240 => vec![Command::Left(false), Command::Right(true)],
                360 => vec![Command::Right(false), Command::Left(true)],
                480 => vec![Command::Left(false)],
                _ => vec![],
            };
            let frame = game.step(commands);
            let cup = frame.cup.unwrap();
            assert!(
                frame.image_error.is_none(),
                "tick={tick} {:?}",
                frame.image_error
            );
            assert_eq!((cup.water, cup.oil), (initial.water, oil));
            for (volume, expected) in cup.fluid.volume.into_iter().zip([initial.water, oil]) {
                assert!((volume - expected as f64).abs() < 1e-6);
            }
            assert_eq!(cup.blocked, 0);
            assert!(
                cup.fluid.max_divergence < 1e-3,
                "tick={tick} {:?}",
                cup.fluid
            );
            max_speed = max_speed.max(cup.fluid.max_speed);
        }
        assert!(max_speed > 2., "cup acceleration did not create flow");
        let frame = game.step(vec![Command::Pause, Command::Right(true)]);
        let x = frame.cup.unwrap().x;
        let clock = frame.simulation_tick;
        let signature = game.cup.as_ref().unwrap().signature();
        for _ in 0..8 {
            let frame = game.step(vec![]);
            assert_eq!((frame.cup.unwrap().x, frame.simulation_tick), (x, clock));
            assert_eq!(game.cup.as_ref().unwrap().signature(), signature);
        }
        game.step(vec![Command::Right(false), Command::Pause]);
        let mut settled = false;
        for tick in 0..2400 {
            let frame = game.step(vec![]);
            assert!(frame.image_error.is_none(), "settling {tick}");
            if frame.cup.unwrap().fluid.sleeping {
                settled = true;
                break;
            }
        }
        assert!(
            settled,
            "shaken cup did not settle: {:?}",
            game.cup.as_ref().unwrap().status(&game.world).fluid
        );
    }
    #[test]
    fn sea_acceptance_sand_displaces_water_over_bank_without_deleting_material() {
        use crate::materials::WATER;
        let mut game = Game::new(160, 120, 4, false).unwrap();
        let initial = game.step(
            crate::scenes::sea(160, 120)
                .into_iter()
                .map(Command::Terrain)
                .collect(),
        );
        let water = initial.terrain.liquid;
        let initial_sand = initial.terrain.sand;
        game.step(vec![Command::Terrain(SandCommand::FillRect {
            x: 12,
            y: 8,
            width: 48,
            height: 30,
            material: SAND,
        })]);
        for _ in 0..900 {
            let frame = game.step(vec![]);
            assert_eq!(
                (frame.terrain.liquid, frame.terrain.sand),
                (water, initial_sand + 1440)
            );
        }
        let overflow = (0..119)
            .flat_map(|y| (82..159).map(move |x| (x, y)))
            .filter(|&(x, y)| game.world.material_at(x, y) == Some(WATER))
            .count();
        assert!(
            overflow > 0,
            "water did not spill onto right-hand sand bank"
        );
    }
    #[test]
    fn free_images_keep_subcell_coordinates_without_changing_terrain_and_reset_cleanly() {
        let mut game = Game::new(96, 96, 1, false).unwrap();
        game.step(vec![Command::Pause]);
        let checksum = game.world.checksum();
        let frame = game.step(vec![Command::PlaceImage([12.25, 33.75])]);
        assert_eq!(frame.images, vec![[12.25, 33.75]]);
        assert_eq!(game.world.checksum(), checksum);
        assert_eq!(frame.simulation_tick, 0);
        game.step(vec![Command::EraseImage([13., 34.])]);
        assert!(game.images.is_empty());
        game.step(vec![Command::PlaceImage([1., 2.])]);
        assert!(game
            .step(vec![Command::Terrain(SandCommand::Clear)])
            .images
            .is_empty());
        assert!(game
            .step(vec![Command::PlaceImage([f32::NAN, 0.])])
            .image_error
            .is_some());
    }

    #[test]
    fn facing_matches_movement_stays_frozen_when_paused_and_resets_right() {
        let mut game = Game::new(160, 120, 1, true).unwrap();
        let right = game.step(vec![Command::Right(true)]);
        assert_eq!(right.facing, 1);
        let left = game.step(vec![Command::Right(false), Command::Left(true)]);
        assert_eq!(left.facing, -1);
        assert!(left.player.unwrap().x() < right.player.unwrap().x());
        let paused = game.step(vec![
            Command::Pause,
            Command::Left(false),
            Command::Right(true),
        ]);
        assert_eq!(paused.facing, -1, "paused input changed visible facing");
        let stepped = game.step(vec![Command::Step]);
        assert_eq!(stepped.facing, 1);
        game.step(vec![
            Command::Right(false),
            Command::Left(true),
            Command::Step,
        ]);
        let reset = game.step(vec![Command::Terrain(SandCommand::Clear)]);
        assert_eq!((reset.facing, reset.direction), (1, 0));
        assert!(reset.paused);
    }

    #[test]
    fn presentation_interpolates_only_between_physics_states_and_snaps_debug_actions() {
        use std::time::Duration;
        let mut game = Game::new(160, 120, 2, true).unwrap();
        game.step(vec![]);
        let start = game.player.unwrap().x();
        let moving = game.step(vec![Command::Right(true)]);
        let current = moving.player.unwrap().x();
        assert!(current > start);
        let shown = moving
            .player_position(Duration::from_secs_f64(1. / 120.))
            .unwrap()[0];
        assert!(shown > start && shown < current);
        let authoritative = game.signature();
        assert_eq!(
            moving.player_position(Duration::from_secs(3)).unwrap()[0],
            current
        );
        assert_eq!(game.signature(), authoritative);
        let paused = game.step(vec![Command::Pause]);
        assert_eq!(paused.player_position(Duration::ZERO).unwrap()[0], current);
        let stepped = game.step(vec![Command::Step]);
        assert_eq!(
            stepped.player_position(Duration::ZERO).unwrap()[0],
            stepped.player.unwrap().x()
        );
        let reset = game.step(vec![Command::Terrain(SandCommand::Clear)]);
        assert_eq!(reset.player_position(Duration::ZERO).unwrap()[0], 40.);
    }

    #[test]
    fn bullets_query_material_policy_destroy_on_hit_and_keep_effects_out_of_physics() {
        let mut game = Game::new(160, 120, 4, true).unwrap();
        game.step(vec![
            Command::Pause,
            Command::Terrain(SandCommand::Paint {
                x: 70,
                y: 68,
                material: WALL,
            }),
            Command::Fire,
        ]);
        assert!(
            game.projectiles.shots().is_empty(),
            "paused fire advanced physics"
        );
        let flying = game.step(vec![Command::Step]);
        assert_eq!(flying.shots.len(), 1);
        let hit = game.step(vec![Command::Step]);
        assert_eq!(hit.impacts, 1);
        assert!(hit.shots.is_empty());
        assert_eq!(hit.sparks.len(), 4);
        assert_eq!(game.world.material_at(67, 68), Some(AIR));
        let pixels = hit.terrain.image.to_rgba();
        assert_eq!(
            &pixels[(68 * 160 + 67) * 4..][..4],
            &game.world.materials().get(AIR).unwrap().color
        );
        let signature = game.signature();
        let frozen = game.step(vec![]);
        assert_eq!(frozen.sparks, hit.sparks);
        assert_eq!(signature, game.signature());
        game.sparks.clear();
        assert_eq!(
            signature,
            game.signature(),
            "visual particles entered authoritative state"
        );
        // Ash passes actors but independently blocks bullets. Frozen terrain keeps
        // this test focused on collision policy rather than powder settling.
        game.world.edit(vec![SandCommand::Paint {
            x: 90,
            y: 68,
            material: crate::materials::ASH,
        }]);
        let world = &game.world;
        let mut shots = Projectiles::default();
        shots
            .spawn(Shot {
                position: [80.5, 68.5],
                velocity: [20., 0.],
                remaining_ticks: 2,
            })
            .unwrap();
        assert_eq!(
            shots.step(|x, y| world.material_at(x, y).is_some_and(|id| world
                .materials()
                .get(id)
                .unwrap()
                .blocks_projectiles))[0]
                .cell,
            [87, 68]
        );
    }

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

    #[test]
    fn held_input_releases_survive_a_full_queue_after_a_stalled_step() {
        use std::{
            sync::mpsc,
            time::{Duration, Instant},
        };
        struct StalledGame {
            game: Game,
            entered: mpsc::SyncSender<()>,
            resume: mpsc::Receiver<()>,
            initial: bool,
            commands: usize,
        }
        impl Simulation for StalledGame {
            type Command = Command;
            type Frame = ([bool; 3], usize);
            fn step(&mut self, commands: Vec<Command>) -> Self::Frame {
                if self.initial {
                    self.initial = false;
                    self.entered.send(()).unwrap();
                    self.resume.recv_timeout(Duration::from_secs(2)).unwrap();
                }
                self.commands += commands.len();
                self.game.step(commands);
                (
                    [self.game.left, self.game.right, self.game.dig],
                    self.commands,
                )
            }
        }
        let mut game = Game::new(160, 120, 1, true).unwrap();
        game.step(vec![
            Command::Left(true),
            Command::Right(true),
            Command::Dig(true),
        ]);
        let (entered_tx, entered) = mpsc::sync_channel(1);
        let (resume, resume_rx) = mpsc::sync_channel(1);
        let runtime = charon_runtime::Runtime::start(StalledGame {
            game,
            entered: entered_tx,
            resume: resume_rx,
            initial: true,
            commands: 0,
        })
        .unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..128 {
            runtime.submit(Command::Inspect(None)).unwrap();
        }
        assert!(matches!(
            runtime.submit(Command::Inspect(None)),
            Err(mpsc::TrySendError::Full(_))
        ));
        // Exercise the same input route as keyboard/focus-loss handling while
        // a long step prevents the worker draining its ordinary queue.
        for command in [
            Command::Left(false),
            Command::Right(false),
            Command::Dig(false),
        ] {
            let _ = submit_input(&runtime, command);
        }
        resume.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(snapshot) = runtime.latest() {
                if snapshot.frame.1 >= 128 {
                    assert_eq!(
                        snapshot.frame.0, [false; 3],
                        "a rejected key release left controls held"
                    );
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "worker did not recover from the stalled step"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        for down in [true, false] {
            submit_input(&runtime, Command::Right(down)).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if runtime
                    .latest()
                    .is_some_and(|snapshot| snapshot.frame.0 == [false, down, false])
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "controls did not resume after saturation"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    #[test]
    fn pause_and_step_survive_a_saturated_queue_without_coalescing() {
        use std::{sync::mpsc, time::Duration};
        struct GatedGame {
            game: Game,
            gate: Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>,
            observed: mpsc::Sender<(bool, u64, Vec<&'static str>)>,
        }
        impl Simulation for GatedGame {
            type Command = Command;
            type Frame = ();
            fn step(&mut self, commands: Vec<Command>) {
                if let Some((entered, resume)) = self.gate.take() {
                    entered.send(()).unwrap();
                    resume.recv_timeout(Duration::from_secs(2)).unwrap();
                }
                let order: Vec<_> = commands
                    .iter()
                    .map(|c| match c {
                        Command::Pause => "pause",
                        Command::Step => "step",
                        Command::Right(_) => "right",
                        _ => "inspect",
                    })
                    .collect();
                let frame = self.game.step(commands);
                if !order.is_empty() {
                    let _ = self
                        .observed
                        .send((frame.paused, frame.simulation_tick, order));
                }
            }
        }
        for (controls, paused, tick) in [
            (vec![Command::Pause], true, 1),
            (vec![Command::Pause, Command::Pause], false, 2),
            (
                vec![Command::Pause, Command::Step, Command::Pause],
                false,
                2,
            ),
        ] {
            let (entered_tx, entered) = mpsc::sync_channel(1);
            let (resume, resume_rx) = mpsc::sync_channel(1);
            let (observed_tx, observed) = mpsc::channel();
            let runtime = charon_runtime::Runtime::start(GatedGame {
                game: Game::new(160, 120, 1, true).unwrap(),
                gate: Some((entered_tx, resume_rx)),
                observed: observed_tx,
            })
            .unwrap();
            entered.recv_timeout(Duration::from_secs(2)).unwrap();
            for _ in 0..128 {
                runtime.submit(Command::Inspect(None)).unwrap();
            }
            assert!(matches!(
                runtime.submit(Command::Inspect(None)),
                Err(mpsc::TrySendError::Full(_))
            ));
            let mut expected = vec!["inspect"; 128];
            for command in controls {
                expected.push(match command {
                    Command::Pause => "pause",
                    _ => "step",
                });
                // The window routes P/N through this helper. Do not hide Full.
                let _ = submit_input(&runtime, command);
                submit_input(&runtime, Command::Right(false)).unwrap();
            }
            // Latest held-state coalesces, but each control must remain ordered.
            expected.push("right");
            resume.send(()).unwrap();
            let actual = observed.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(
                actual,
                (paused, tick, expected),
                "pause/step was lost or reordered"
            );
            submit_input(&runtime, Command::Pause).unwrap();
            let next = observed.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(next.0, !paused, "pause/resume did not recover");
            if paused {
                assert!(next.1 > tick);
            }
        }
    }
    #[test]
    fn generated_cave_combines_movement_digging_building_and_seeded_retry() {
        for workers in [1, 4] {
            let mut game = Game::new(480, 270, workers, true).unwrap();
            let first = game.step(vec![Command::Explore(42), Command::Pause]);
            assert_eq!(first.exploration_seed, Some(42));
            assert_eq!(first.simulation_tick, 0);
            let spawn = first.player.unwrap();
            let initial = game.signature();
            let cells = game.world.checksum();
            game.step(vec![Command::Terrain(SandCommand::FillRect {
                x: 80,
                y: 160,
                width: 8,
                height: 8,
                material: WALL,
            })]);
            assert_eq!(game.world.material_at(80, 160), Some(WALL));
            game.step(vec![Command::Terrain(SandCommand::FillRect {
                x: 80,
                y: 160,
                width: 8,
                height: 8,
                material: AIR,
            })]);
            assert_eq!(game.world.material_at(80, 160), Some(AIR));
            let mut frame = game.step(vec![Command::Pause, Command::Right(true)]);
            for _ in 0..16 {
                frame = game.step(vec![]);
            }
            assert!(frame.player.unwrap().x() > spawn.x() + 10.);
            game.step(vec![Command::Right(false), Command::Dig(true)]);
            for _ in 0..20 {
                frame = game.step(vec![]);
            }
            assert!(
                frame.player.unwrap().y() > spawn.y() + 5.,
                "dig did not open ground below actor"
            );
            assert!(!frame.player.unwrap().embedded);
            let retried = game.step(vec![Command::Pause, Command::Terrain(SandCommand::Clear)]);
            assert!(retried.paused);
            assert_eq!(
                game.signature(),
                initial,
                "retry did not restore the seed and controls"
            );
            assert_eq!(game.world.checksum(), cells);
            game.step(vec![Command::Explore(43)]);
            assert_eq!(game.exploration_seed, Some(43));
            assert_ne!(game.world.checksum(), cells);
        }
    }
    #[test]
    fn aimed_shots_use_world_coordinates_and_wait_for_single_step() {
        for target in [[120., 74.], [5., 74.], [23., 40.]] {
            let mut game = Game::new(160, 120, 1, true).unwrap();
            game.step(vec![Command::Explore(42), Command::Pause]);
            let reserved = game.step(vec![Command::FireAt(target)]);
            assert!(reserved.shots.is_empty());
            assert_eq!(reserved.simulation_tick, 0);
            assert!(game.fire_pending);
            let first = game.step(vec![Command::Step]);
            assert_eq!(first.shots.len(), 1);
            let shot = game.projectiles.shots()[0];
            let side = if target[0] < 23.5 { -1. } else { 1. };
            let muzzle = [if side < 0. { 19.5 } else { 27.5 }, 74.];
            let delta = [target[0] - muzzle[0], target[1] - muzzle[1]];
            assert!((shot.velocity[0] * delta[1] - shot.velocity[1] * delta[0]).abs() < 0.001);
            assert!((shot.velocity[0].hypot(shot.velocity[1]) - 12.).abs() < 0.001);
            assert!(!game.fire_pending && game.fire_target.is_none());
            let frozen = game.step(vec![]);
            assert_eq!(frozen.shots, first.shots);
        }
        let mut game = Game::new(160, 120, 1, true).unwrap();
        game.step(vec![Command::Explore(42)]);
        let floor = game.world.material_at(26, 80);
        assert_eq!(floor, Some(WALL));
        let hit = game.step(vec![Command::FireAt([25., 105.])]);
        assert_eq!(hit.impacts, 1);
        assert!(hit.shots.is_empty());
        assert_eq!(game.world.material_at(26, 80), Some(AIR));
        let invalid = game.step(vec![Command::FireAt([f32::NAN, 0.])]);
        assert_eq!(invalid.image_error, Some("INVALID AIM POSITION"));
        assert!(!game.fire_pending);
    }
    #[test]
    fn run_starts_ends_freezes_and_retries_the_same_map_and_health() {
        use crate::combat::Phase;
        let mut game = Game::new(480, 270, 4, true).unwrap();
        let ready = game.step(vec![Command::NewRun(42)]);
        assert_eq!(ready.state_label(), "READY");
        assert_eq!(ready.combat.unwrap().phase, Phase::Ready);
        for _ in 0..5 {
            let frame = game.step(vec![Command::Right(true), Command::FireAt([290., 174.])]);
            assert_eq!(frame.simulation_tick, 0);
            assert!(frame.shots.is_empty());
        }
        let playing = game.step(vec![Command::Begin]);
        assert_eq!(playing.combat.unwrap().phase, Phase::Playing);
        let initial = game.signature();
        let paused = game.step(vec![Command::Pause]);
        let clock = paused.simulation_tick;
        let signature = game.signature();
        for _ in 0..10 {
            assert_eq!(game.step(vec![]).simulation_tick, clock);
        }
        assert_eq!(game.signature(), signature, "combat advanced while paused");
        game.step(vec![Command::Pause]);
        let mut defeated = None;
        for _ in 0..500 {
            let frame = game.step(vec![]);
            if frame.combat.as_ref().unwrap().phase == Phase::Defeated {
                defeated = Some(frame);
                break;
            }
        }
        let end = defeated.expect("enemy fire never ended the run");
        assert_eq!(end.state_label(), "DEFEATED");
        assert_eq!(end.combat.unwrap().health, 0);
        let end_signature = game.signature();
        for _ in 0..20 {
            let frame = game.step(vec![
                Command::Right(true),
                Command::FireAt([290., 174.]),
                Command::Terrain(SandCommand::Paint {
                    x: 60,
                    y: 168,
                    material: WALL,
                }),
            ]);
            assert_eq!(frame.simulation_tick, end.simulation_tick);
        }
        assert_eq!(
            game.signature(),
            end_signature,
            "defeated game still accepted play inputs"
        );
        let retry = game.step(vec![Command::Terrain(SandCommand::Clear)]);
        assert_eq!(retry.combat.unwrap().health, 3);
        assert_eq!(game.signature(), initial, "retry failed to restore the run");
    }

    #[test]
    fn ranged_combat_respects_terrain_and_defeats_enemies() {
        use crate::combat::Phase;
        for blocked in [true, false] {
            let mut game = Game::new(480, 270, 1, true).unwrap();
            game.step(vec![Command::NewRun(42), Command::Begin]);
            if blocked {
                game.step(vec![Command::Terrain(SandCommand::FillRect {
                    x: 120,
                    y: 148,
                    width: 8,
                    height: 32,
                    material: WALL,
                })]);
                game.step(vec![Command::FireAt([291., 174.])]);
                for _ in 0..30 {
                    game.step(vec![]);
                }
                assert_eq!(
                    game.combat.as_ref().unwrap().frame().enemies[0].health,
                    3,
                    "player shot passed through cover"
                );
                // Restore the cover after the player's deliberate terrain damage.
                game.step(vec![Command::Terrain(SandCommand::FillRect {
                    x: 120,
                    y: 148,
                    width: 8,
                    height: 32,
                    material: WALL,
                })]);
                for _ in 0..400 {
                    game.step(vec![]);
                }
                let combat = game.combat.as_ref().unwrap().frame();
                assert_eq!(
                    (combat.health, combat.phase),
                    (3, Phase::Playing),
                    "enemy shots passed through cover"
                );
            } else {
                for _ in 0..3 {
                    game.step(vec![Command::FireAt([291., 174.])]);
                }
                for _ in 0..40 {
                    game.step(vec![]);
                }
                let combat = game.combat.as_ref().unwrap().frame();
                assert!(combat.enemies.is_empty());
                assert_eq!((combat.kills, combat.health), (1, 3));
            }
        }
    }
    #[test]
    fn a_batch_of_single_steps_stops_at_the_defeat_boundary() {
        let mut reference = Game::new(480, 270, 1, true).unwrap();
        reference.step(vec![Command::NewRun(42), Command::Begin]);
        let defeat_tick = loop {
            let frame = reference.step(vec![]);
            if frame.combat.unwrap().phase == crate::combat::Phase::Defeated {
                break frame.simulation_tick;
            }
            assert!(frame.simulation_tick < 500);
        };
        let mut game = Game::new(480, 270, 4, true).unwrap();
        game.step(vec![Command::NewRun(42), Command::Begin]);
        while game.world.tick() < defeat_tick - 5 {
            game.step(vec![]);
        }
        let frame = game.step(vec![Command::Step; 8]);
        assert_eq!(frame.combat.unwrap().phase, crate::combat::Phase::Defeated);
        assert_eq!(
            frame.simulation_tick, defeat_tick,
            "queued steps advanced past game end"
        );
    }
}
