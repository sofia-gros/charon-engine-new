//! Rules for the prototype game. Core/projectiles do not know health or teams.
use charon_platformer::Body;
use charon_projectiles::{Projectiles, Shot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Ready,
    Playing,
    Defeated,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behavior {
    Turret,
    Chase,
}
#[derive(Clone, Copy)]
pub struct EnemyView {
    pub id: u64,
    pub rect: [f32; 4],
    pub health: u8,
    pub behavior: Behavior,
}
pub struct Frame {
    pub phase: Phase,
    pub health: u8,
    pub enemies: Vec<EnemyView>,
    pub shots: Vec<[f32; 2]>,
    pub kills: u32,

    pub rejected_shots: u64,
    pub despawned: u64,
    pub despawned_shots: u64,
    pub nav_expansions: usize,
}
struct Enemy {
    steer: i8,
    jump_next: bool,
    steer_ticks: u8,
    id: u64,
    spawn: u8,
    origin: [i64; 2],
    active: bool,
    behavior: Behavior,
    body: Body,
    health: u8,
    cooldown: u16,
}
pub struct Combat {
    planner_cursor: usize,
    nav_expansions: usize,
    seed: u64,
    region: Option<([i64; 2], [usize; 2])>,
    next_id: u64,
    despawned: u64,
    despawned_shots: u64,
    rejected_shots: u64,
    phase: Phase,
    health: u8,
    enemies: Vec<Enemy>,
    shots: Projectiles,
    invulnerable: u8,
    kills: u32,
}
pub fn contains(rect: [f32; 4], x: i32, y: i32) -> bool {
    x as f32 + 1. > rect[0]
        && (x as f32) < rect[0] + rect[2]
        && y as f32 + 1. > rect[1]
        && (y as f32) < rect[1] + rect[3]
}
fn rect(body: Body) -> [f32; 4] {
    [body.x(), body.y(), body.width(), body.height()]
}
impl Combat {
    pub fn new(width: usize, height: usize, ready: bool) -> Self {
        Self {
            planner_cursor: 0,
            nav_expansions: 0,
            seed: 0,
            region: None,
            next_id: 2,
            despawned: 0,
            despawned_shots: 0,
            rejected_shots: 0,
            phase: if ready { Phase::Ready } else { Phase::Playing },
            health: 3,
            enemies: vec![Enemy {
                steer: 0,
                jump_next: false,
                steer_ticks: 0,
                id: 1,
                spawn: 0,
                origin: [0, 0],
                active: true,
                behavior: Behavior::Turret,
                body: Body::new(
                    (width * 3 / 5) as f32,
                    (height * 2 / 3 - 12) as f32,
                    7.,
                    12.,
                ),
                health: 3,
                cooldown: 90,
            }],
            shots: Projectiles::default(),
            invulnerable: 0,
            kills: 0,
        }
    }
    /// Two game-owned prototype encounters. Global anchors stay integers;
    /// dormant bodies retain their bounded local coordinates and exact state.
    pub fn streamed(width: usize, height: usize, origin: [i64; 2], seed: u64) -> Self {
        let mut combat = Self::new(width, height, true);
        combat.enemies.clear();
        combat.seed = seed;
        combat.next_id = 1;
        combat.region = Some((origin, [width, height]));
        combat.spawn_nearby();
        combat
    }
    fn offset(old: [i64; 2], new: [i64; 2], size: [usize; 2]) -> Option<[f32; 2]> {
        let delta = [old[0].checked_sub(new[0])?, old[1].checked_sub(new[1])?];
        // Far-away dormant objects are never converted to floating point.
        if (0..2).any(|i| delta[i].unsigned_abs() > size[i] as u64 * 2) {
            return None;
        }
        Some(delta.map(|v| v as f32))
    }
    fn inside(position: [f32; 2], extent: [f32; 2], size: [usize; 2], margin: f32) -> bool {
        (0..2).all(|i| position[i] >= margin && position[i] + extent[i] < size[i] as f32 - margin)
    }
    fn spawn_nearby(&mut self) {
        let Some((origin, size)) = self.region else {
            return;
        };
        for (spawn, (x, behavior)) in [(-180., Behavior::Turret), (1100., Behavior::Chase)]
            .into_iter()
            .enumerate()
        {
            if self.enemies.len() >= 128 || self.enemies.iter().any(|e| e.spawn == spawn as u8) {
                continue;
            }
            let Some(offset) = Self::offset([0, 0], origin, size) else {
                continue;
            };
            let p = [
                x + offset[0],
                offset[1] + (crate::world_view::surface_y(x as i64, self.seed) - 12) as f32,
            ];
            if Self::inside(p, [7., 12.], size, 2.) {
                self.enemies.push(Enemy {
                    steer: 0,
                    jump_next: false,
                    steer_ticks: 0,
                    id: self.next_id,
                    spawn: spawn as u8,
                    origin,
                    active: true,
                    behavior,
                    body: Body::new(p[0], p[1], 7., 12.),
                    health: 3,
                    cooldown: 90,
                });
                self.next_id += 1;
            }
        }
    }
    pub fn rebase(&mut self, origin: [i64; 2]) {
        let Some((old, size)) = self.region else {
            return;
        };
        self.enemies.retain_mut(|enemy| {
            let keep = Self::offset(enemy.origin, origin, size).is_some_and(|offset| {
                let p = [enemy.body.x() + offset[0], enemy.body.y() + offset[1]];
                if !Self::inside(p, [enemy.body.width(), enemy.body.height()], size, 2.) {
                    return false;
                }
                enemy.body.translate(offset);
                enemy.origin = origin;
                true
            });
            if !keep && enemy.health > 0 {
                self.despawned += 1;
            }
            keep
        });
        let mut next = Projectiles::default();
        for shot in self.shots.shots() {
            if let Some(offset) = Self::offset(old, origin, size) {
                let mut shot = *shot;
                shot.position = [shot.position[0] + offset[0], shot.position[1] + offset[1]];
                if Self::inside(shot.position, [0., 0.], size, 2.) {
                    next.spawn(shot).expect("bounded live shots");
                    continue;
                }
            }
            self.despawned_shots += 1;
        }
        self.shots = next;
        self.region = Some((origin, size));
        self.spawn_nearby();
    }
    pub fn save_state(&self) -> Option<crate::world_save::CombatState> {
        use crate::world_save::{BodyState, CombatState, EnemyState, ShotState};
        let (origin, size) = self.region?;
        Some(CombatState {
            origin,
            size,
            phase: self.phase as u8,
            health: self.health,
            invulnerable: self.invulnerable,
            kills: self.kills,
            seed: self.seed,
            planner_cursor: self.planner_cursor,
            rejected: self.rejected_shots,
            next_id: self.next_id,
            despawned: self.despawned,
            despawned_shots: self.despawned_shots,
            enemies: self
                .enemies
                .iter()
                .map(|e| EnemyState {
                    steer: e.steer,
                    jump_next: e.jump_next,
                    steer_ticks: e.steer_ticks,
                    id: e.id,
                    spawn: e.spawn,
                    origin: e.origin,
                    active: e.active,
                    chase: e.behavior == Behavior::Chase,
                    body: BodyState::capture(e.body),
                    health: e.health,
                    cooldown: e.cooldown,
                })
                .collect(),
            shots: self
                .shots
                .shots()
                .iter()
                .copied()
                .map(ShotState::capture)
                .collect(),
        })
    }
    pub fn restore_state(state: &crate::world_save::CombatState) -> Result<Self, String> {
        if state.health > 3
            || state.invulnerable > 30
            || state.enemies.len() > 128
            || state.shots.len() > 1024
            || state.next_id == 0
        {
            return Err("invalid saved combat counts".into());
        }
        let phase = match state.phase {
            0 => Phase::Ready,
            1 => Phase::Playing,
            2 => Phase::Defeated,
            _ => return Err("invalid combat phase".into()),
        };
        if (phase == Phase::Defeated) != (state.health == 0) {
            return Err("combat phase and health disagree".into());
        }
        let mut enemies = Vec::new();
        for e in &state.enemies {
            if ![-1, 0, 1].contains(&e.steer)
                || e.steer_ticks > 6
                || e.id == 0
                || e.id >= state.next_id
                || e.spawn > 1
                || e.health > 3
                || e.cooldown > 90
                || !e.active
                || e.origin != state.origin
                || state
                    .enemies
                    .iter()
                    .filter(|other| other.id == e.id)
                    .count()
                    != 1
            {
                return Err("invalid saved enemy".into());
            }
            let body = e.body.restore()?;
            if e.active
                && !Self::inside(
                    [body.x(), body.y()],
                    [body.width(), body.height()],
                    state.size,
                    2.,
                )
            {
                return Err("active enemy outside saved region".into());
            }
            enemies.push(Enemy {
                steer: e.steer,
                jump_next: e.jump_next,
                steer_ticks: e.steer_ticks,
                id: e.id,
                spawn: e.spawn,
                origin: e.origin,
                active: e.active,
                behavior: if e.chase {
                    Behavior::Chase
                } else {
                    Behavior::Turret
                },
                body,
                health: e.health,
                cooldown: e.cooldown,
            });
        }
        let shots = crate::world_save::shots(&state.shots)?;
        Ok(Self {
            planner_cursor: state.planner_cursor,
            nav_expansions: 0,
            seed: state.seed,
            region: Some((state.origin, state.size)),
            next_id: state.next_id,
            despawned: state.despawned,
            despawned_shots: state.despawned_shots,
            rejected_shots: state.rejected,
            phase,
            health: state.health,
            enemies,
            shots,
            invulnerable: state.invulnerable,
            kills: state.kills,
        })
    }
    pub fn set_behavior(&mut self, behavior: Behavior) {
        for enemy in &mut self.enemies {
            enemy.behavior = behavior;
        }
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn begin(&mut self) {
        if self.phase == Phase::Ready {
            self.phase = Phase::Playing;
        }
    }
    pub fn colliders(&self) -> Vec<[f32; 4]> {
        self.enemies
            .iter()
            .filter(|e| e.health > 0 && e.active)
            .map(|e| rect(e.body))
            .collect()
    }
    /// Keep dead bodies addressable until the batch ends: simultaneous impacts
    /// on a killed enemy must not turn into terrain destruction requests.
    pub fn hit_enemy(&mut self, cell: [i32; 2]) -> bool {
        if let Some(enemy) = self
            .enemies
            .iter_mut()
            .find(|e| e.active && contains(rect(e.body), cell[0], cell[1]))
        {
            if enemy.health > 0 {
                enemy.health -= 1;
                if enemy.health == 0 {
                    self.kills += 1;
                }
            }
            true
        } else {
            false
        }
    }
    fn needs_path(enemy: &Enemy, player: Body, solid: &impl Fn(i32, i32) -> bool) -> bool {
        let dx = player.x() - enemy.body.x();
        let front = if dx < 0. {
            enemy.body.x() - 2.
        } else {
            enemy.body.x() + enemy.body.width() + 2.
        };
        let blocked = (enemy.body.y().floor() as i32
            ..(enemy.body.y() + enemy.body.height()).ceil() as i32)
            .any(|y| solid(front.floor() as i32, y));
        let gap = enemy.body.grounded
            && !solid(
                front.floor() as i32,
                (enemy.body.y() + enemy.body.height() + 1.).floor() as i32,
            );
        blocked || gap || (player.y() - enemy.body.y()).abs() > 12.
    }
    pub fn tick(
        &mut self,
        player: Body,
        actor_solid: impl Fn(i32, i32) -> bool,
        solid: impl Fn(i32, i32) -> bool,
    ) {
        if self.phase != Phase::Playing {
            return;
        }
        self.invulnerable = self.invulnerable.saturating_sub(1);
        self.nav_expansions = 0;
        let count = self.enemies.len();
        let planner = (0..count)
            .map(|i| (i + self.planner_cursor) % count)
            .find(|&i| {
                let enemy = &self.enemies[i];
                enemy.health > 0
                    && enemy.behavior == Behavior::Chase
                    && enemy.steer_ticks == 0
                    && Self::needs_path(enemy, player, &actor_solid)
            });
        if let Some(index) = planner {
            self.planner_cursor = (index + 1) % count;
        }
        for (index, enemy) in self.enemies.iter_mut().enumerate() {
            if enemy.health == 0 || !enemy.active {
                continue;
            }
            let dx = player.x() + player.width() / 2. - enemy.body.x() - enemy.body.width() / 2.;
            let direction = if enemy.behavior == Behavior::Chase && dx.abs() > 16. {
                if dx < 0. {
                    -1
                } else {
                    1
                }
            } else {
                0
            };
            let mut jump = false;
            let direction = if enemy.behavior == Behavior::Chase {
                if Self::needs_path(enemy, player, &actor_solid) {
                    if enemy.steer_ticks == 0 {
                        if planner == Some(index) {
                            let plan = charon_navigation::plan(
                                enemy.body,
                                [player.x(), player.y()],
                                &actor_solid,
                            );
                            self.nav_expansions = plan.expansions;
                            enemy.steer = plan.control.direction;
                            enemy.jump_next = plan.control.jump;
                            enemy.steer_ticks = 6;
                        } else {
                            enemy.steer = 0;
                        }
                    }
                    jump = enemy.jump_next;
                    enemy.jump_next = false;
                    enemy.steer_ticks = enemy.steer_ticks.saturating_sub(1);
                    enemy.steer
                } else {
                    enemy.steer_ticks = 0;
                    enemy.steer = direction;
                    direction
                }
            } else {
                0
            };
            enemy.body.step(direction, jump, &actor_solid);
            if let Some((_, size)) = self.region {
                if !Self::inside(
                    [enemy.body.x(), enemy.body.y()],
                    [enemy.body.width(), enemy.body.height()],
                    size,
                    2.,
                ) {
                    enemy.active = false;
                    continue;
                }
            }
            enemy.cooldown = enemy.cooldown.saturating_sub(1);
            if enemy.cooldown == 0 && !enemy.body.embedded {
                let aim = [
                    player.x() + player.width() / 2.,
                    player.y() + player.height() / 2.,
                ];
                let body = enemy.body;
                let position = [
                    if aim[0] < body.x() {
                        body.x() - 0.5
                    } else {
                        body.x() + body.width() + 0.5
                    },
                    body.y() + body.height() / 2.,
                ];
                let delta = [aim[0] - position[0], aim[1] - position[1]];
                let length = delta[0].hypot(delta[1]);
                let velocity = if length < 0.001 {
                    [-5., 0.]
                } else {
                    delta.map(|v| v / length * 5.)
                };
                // Live projectiles share a bounded budget.
                if self.shots.shots().len() >= 1024 {
                    self.rejected_shots += 1;
                    enemy.cooldown = 90;
                    continue;
                }
                self.shots
                    .spawn(Shot {
                        position,
                        velocity,
                        acceleration: [0., 0.],
                        remaining_ticks: if self.region.is_some() { 16 } else { 180 },
                    })
                    .expect("bounded prototype enemy fire");
                enemy.cooldown = 90;
            }
        }
        self.enemies.retain(|e| {
            if !e.active && e.health > 0 {
                self.despawned += 1;
            }
            e.active
        });
        if let Some((_, size)) = self.region {
            self.despawned_shots += self.shots.retain(|s| {
                Self::inside(
                    [s.position[0] + s.velocity[0], s.position[1] + s.velocity[1]],
                    [0., 0.],
                    size,
                    2.,
                )
            }) as u64;
        }
        let player_rect = rect(player);
        let hits = self
            .shots
            .step(|x, y| solid(x, y) || contains(player_rect, x, y));
        for hit in hits {
            if !solid(hit.cell[0], hit.cell[1])
                && contains(player_rect, hit.cell[0], hit.cell[1])
                && self.invulnerable == 0
            {
                self.health = self.health.saturating_sub(1);
                self.invulnerable = 30;
                if self.health == 0 {
                    self.phase = Phase::Defeated;
                }
            }
        }
    }
    pub fn frame(&self) -> Frame {
        Frame {
            phase: self.phase,
            health: self.health,
            enemies: self
                .enemies
                .iter()
                .filter(|e| e.health > 0 && e.active)
                .map(|e| EnemyView {
                    id: e.id,
                    rect: rect(e.body),
                    health: e.health,
                    behavior: e.behavior,
                })
                .collect(),
            shots: self.shots.shots().iter().map(|s| s.position).collect(),
            kills: self.kills,

            despawned: self.despawned,
            despawned_shots: self.despawned_shots,
            nav_expansions: self.nav_expansions,
            rejected_shots: self.rejected_shots,
        }
    }
    pub fn signature(&self) -> Vec<u64> {
        let mut values = vec![
            self.phase as u64,
            self.health as u64,
            self.invulnerable as u64,
            self.kills as u64,
        ];
        if let Some((origin, size)) = self.region {
            values.extend([
                0x776f726c646169,
                origin[0] as u64,
                origin[1] as u64,
                size[0] as u64,
                size[1] as u64,
                self.rejected_shots,
                self.next_id,
                self.despawned,
                self.despawned_shots,
            ]);
        }
        values.extend([0x6e6176, self.planner_cursor as u64]);
        for enemy in &self.enemies {
            values.extend([
                enemy.steer as u64,
                u64::from(enemy.jump_next),
                u64::from(enemy.steer_ticks),
            ]);
            if self.region.is_some() {
                values.extend([
                    enemy.id,
                    enemy.spawn as u64,
                    enemy.origin[0] as u64,
                    enemy.origin[1] as u64,
                    u64::from(enemy.active),
                ]);
            }
            if enemy.behavior == Behavior::Chase {
                values.push(0x6368617365);
            }
            values.extend([
                enemy.health as u64,
                enemy.cooldown as u64,
                u64::from(enemy.body.x().to_bits()),
                u64::from(enemy.body.y().to_bits()),
                u64::from(enemy.body.vertical_velocity().to_bits()),
                u64::from(enemy.body.grounded),
                u64::from(enemy.body.embedded),
            ]);
        }
        for shot in self.shots.shots() {
            values.extend(
                shot.position
                    .into_iter()
                    .chain(shot.velocity)
                    .map(|v| u64::from(v.to_bits())),
            );
            values.push(shot.remaining_ticks as u64);
        }
        values
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlap_keeps_health_but_outside_entities_despawn_and_memory_stays_bounded() {
        let origin = [-960, -540];
        let mut combat = Combat::streamed(1920, 1080, origin, 42);
        assert_eq!(combat.enemies.len(), 1);
        let id = combat.enemies[0].id;
        let y = combat.enemies[0].body.y() as i32 + 1;
        combat.hit_enemy([781, y]);
        combat.rebase([-448, -540]);
        let first = combat.enemies.iter().find(|e| e.id == id).unwrap();
        assert_eq!(first.health, 2);
        combat
            .shots
            .spawn(Shot {
                position: [100., 546.],
                velocity: [5., 0.],
                acceleration: [0., 0.],
                remaining_ticks: 16,
            })
            .unwrap();
        combat.rebase([64, -540]);
        assert!(!combat.enemies.iter().any(|e| e.id == id));
        assert!(combat.shots.shots().is_empty());
        assert!(combat.despawned > 0 && combat.despawned_shots > 0);
        for index in 0..600 {
            combat.rebase(if index % 2 == 0 {
                origin
            } else {
                [1_i64 << 54, -540]
            });
            assert!(combat.enemies.len() <= 2);
            assert!(combat.shots.shots().is_empty());
        }
        combat.rebase(origin);
        assert_eq!(combat.frame().enemies[0].health, 3);
        assert_ne!(combat.frame().enemies[0].id, id);
        let restored = Combat::restore_state(&combat.save_state().unwrap()).unwrap();
        assert_eq!(restored.signature(), combat.signature());
    }
    #[test]
    fn chase_uses_an_intermediate_ledge_with_a_bounded_planner_budget() {
        let solid = |x, y| y >= 100 || ((60..76).contains(&x) && y == 80) || (x < 45 && y >= 60);
        let player = Body::new(25., 48., 7., 12.);
        let mut combat = Combat::new(160, 120, false);
        combat.set_behavior(Behavior::Chase);
        combat.enemies[0].body = Body::new(82., 88., 7., 12.);
        combat.enemies[0]
            .body
            .restore_motion(0., true, false)
            .unwrap();
        combat.enemies[0].cooldown = 1000;
        let mut reached = false;
        let mut planned = false;
        for _ in 0..360 {
            combat.tick(player, solid, solid);
            assert!(combat.nav_expansions <= charon_navigation::MAX_EXPANSIONS);
            planned |= combat.nav_expansions > 0;
            let body = combat.enemies[0].body;
            assert!(!body.embedded);
            if body.x() < 45. && body.y() <= 49. {
                reached = true;
                break;
            }
        }
        assert!(
            planned && reached,
            "chaser did not climb via the intermediate ledge"
        );
    }
    #[test]
    fn hunters_follow_both_directions_jump_small_obstacles_and_respect_tall_walls() {
        for player_x in [20., 130.] {
            let player = Body::new(player_x, 68., 7., 12.);
            let mut combat = Combat::new(160, 120, false);
            combat.set_behavior(Behavior::Chase);
            let start = combat.frame().enemies[0].rect[0];
            for _ in 0..15 {
                combat.tick(player, |_, y| y >= 80, |_, y| y >= 80);
            }
            let end = combat.frame().enemies[0].rect[0];
            assert!((player_x - start).abs() > (player_x - end).abs());
        }
        for height in [4, 40] {
            let player = Body::new(20., 68., 7., 12.);
            let mut combat = Combat::new(160, 120, false);
            combat.set_behavior(Behavior::Chase);
            let solid = |x, y| y >= 80 || ((70..75).contains(&x) && y >= 80 - height);
            let mut highest = 68_f32;
            for _ in 0..80 {
                combat.tick(player, solid, solid);
                highest = highest.min(combat.enemies[0].body.y());
                assert!(!combat.enemies[0].body.embedded);
            }
            let x = combat.enemies[0].body.x();
            if height == 4 {
                assert!(highest < 63., "hunter never jumped at small obstacle");
                assert!(x < 70., "small obstacle was not traversed");
            } else {
                assert!(x >= 75., "hunter passed through a tall wall");
            }
        }
        let player = Body::new(20., 68., 7., 12.);
        let mut turret = Combat::new(160, 120, false);
        for _ in 0..15 {
            turret.tick(player, |_, y| y >= 80, |_, y| y >= 80);
        }
        assert_eq!(
            turret.frame().enemies[0].rect[0],
            96.,
            "stationary behavior was lost"
        );
    }
}
