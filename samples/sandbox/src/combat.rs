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
    pub sleeping: usize,
    pub sleeping_shots: usize,
    pub rejected_shots: u64,
}
struct Enemy {
    origin: [i64; 2],
    active: bool,
    behavior: Behavior,
    body: Body,
    health: u8,
    cooldown: u16,
}
pub struct Combat {
    region: Option<([i64; 2], [usize; 2])>,
    parked_shots: Vec<([i64; 2], Shot)>,
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
            region: None,
            parked_shots: vec![],
            rejected_shots: 0,
            phase: if ready { Phase::Ready } else { Phase::Playing },
            health: 3,
            enemies: vec![Enemy {
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
    pub fn streamed(width: usize, height: usize, origin: [i64; 2]) -> Self {
        let mut combat = Self::new(width, height, true);
        combat.enemies.clear();
        for (x, behavior) in [(-180., Behavior::Turret), (1100., Behavior::Chase)] {
            combat.enemies.push(Enemy {
                origin: [0, 0],
                active: false,
                behavior,
                body: Body::new(x, 0., 7., 12.),
                health: 3,
                cooldown: 90,
            });
        }
        combat.region = Some(([0, 0], [width, height]));
        combat.rebase(origin);
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
    pub fn rebase(&mut self, origin: [i64; 2]) {
        let Some((old, size)) = self.region else {
            return;
        };
        for enemy in &mut self.enemies {
            enemy.active = false;
            if let Some(offset) = Self::offset(enemy.origin, origin, size) {
                let position = [enemy.body.x() + offset[0], enemy.body.y() + offset[1]];
                if Self::inside(
                    position,
                    [enemy.body.width(), enemy.body.height()],
                    size,
                    96.,
                ) {
                    enemy.body.translate(offset);
                    enemy.origin = origin;
                    enemy.active = true;
                }
            }
        }
        self.parked_shots
            .extend(self.shots.shots().iter().map(|s| (old, *s)));
        self.shots = Projectiles::default();
        self.parked_shots.retain(|&(anchor, mut shot)| {
            if let Some(offset) = Self::offset(anchor, origin, size) {
                let position = [shot.position[0] + offset[0], shot.position[1] + offset[1]];
                // Keep the whole remaining straight trajectory clear of temporary
                // window guards, instead of manufacturing a terrain collision.
                let margin = shot.velocity[0].abs().max(shot.velocity[1].abs())
                    * f32::from(shot.remaining_ticks)
                    + 2.;
                if Self::inside(position, [0., 0.], size, margin) {
                    shot.position = position;
                    self.shots
                        .spawn(shot)
                        .expect("total active and sleeping shots are bounded");
                    return false;
                }
            }
            true
        });
        self.region = Some((origin, size));
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
            rejected: self.rejected_shots,
            enemies: self
                .enemies
                .iter()
                .map(|e| EnemyState {
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
            parked: self
                .parked_shots
                .iter()
                .map(|(o, s)| (*o, ShotState::capture(*s)))
                .collect(),
        })
    }
    pub fn restore_state(state: &crate::world_save::CombatState) -> Result<Self, String> {
        if state.health > 3
            || state.invulnerable > 30
            || state.enemies.len() != 2
            || state.shots.len() + state.parked.len() > 1024
            || state.kills != state.enemies.iter().filter(|e| e.health == 0).count() as u32
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
            if e.health > 3 || e.cooldown > 90 || (e.active && e.origin != state.origin) {
                return Err("invalid saved enemy".into());
            }
            let body = e.body.restore()?;
            if e.active
                && !Self::inside(
                    [body.x(), body.y()],
                    [body.width(), body.height()],
                    state.size,
                    95.,
                )
            {
                return Err("active enemy outside saved region".into());
            }
            enemies.push(Enemy {
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
        let mut parked_shots = Vec::new();
        for (origin, shot) in &state.parked {
            parked_shots.push((*origin, shot.restore()?));
        }
        Ok(Self {
            region: Some((state.origin, state.size)),
            parked_shots,
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
        for enemy in &mut self.enemies {
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
            let front = if direction < 0 {
                enemy.body.x() - 1.
            } else {
                enemy.body.x() + enemy.body.width() + 1.
            };
            let obstacle = direction != 0
                && actor_solid(
                    front.floor() as i32,
                    (enemy.body.y() + enemy.body.height() - 1.).floor() as i32,
                );
            enemy
                .body
                .step(direction, obstacle && enemy.body.grounded, &actor_solid);
            if let Some((_, size)) = self.region {
                if !Self::inside(
                    [enemy.body.x(), enemy.body.y()],
                    [enemy.body.width(), enemy.body.height()],
                    size,
                    96.,
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
                // Sleeping projectiles count against the same bounded budget.
                if self.shots.shots().len() + self.parked_shots.len() >= 1024 {
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
                .enumerate()
                .filter(|(_, e)| e.health > 0 && e.active)
                .map(|(index, e)| EnemyView {
                    id: index as u64 + 1,
                    rect: rect(e.body),
                    health: e.health,
                    behavior: e.behavior,
                })
                .collect(),
            shots: self.shots.shots().iter().map(|s| s.position).collect(),
            kills: self.kills,
            sleeping: self
                .enemies
                .iter()
                .filter(|e| e.health > 0 && !e.active)
                .count(),
            sleeping_shots: self.parked_shots.len(),
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
            ]);
        }
        for enemy in &self.enemies {
            if self.region.is_some() {
                values.extend([
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
        for (origin, shot) in &self.parked_shots {
            values.extend([0x7061726b, origin[0] as u64, origin[1] as u64]);
            values.extend(
                shot.position
                    .into_iter()
                    .chain(shot.velocity)
                    .chain(shot.acceleration)
                    .map(|v| u64::from(v.to_bits())),
            );
            values.push(u64::from(shot.remaining_ticks));
        }
        values
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streamed_enemies_and_shots_sleep_without_reset_or_float_world_coordinates() {
        let origin = [-960, -540];
        let mut combat = Combat::streamed(1920, 1080, origin);
        assert_eq!(combat.frame().sleeping, 1);
        assert_eq!(combat.frame().enemies[0].id, 1);
        combat.begin();
        let player = Body::new(960., 540., 7., 12.);
        combat.tick(player, |_, y| y >= 552, |_, y| y >= 552);
        assert!(combat.hit_enemy([781, 541]));
        assert_eq!(combat.enemies[0].health, 2);
        combat
            .shots
            .spawn(Shot {
                position: [800., 546.],
                velocity: [5., 0.],
                acceleration: [0., 0.],
                remaining_ticks: 16,
            })
            .unwrap();
        let cooldown = combat.enemies[0].cooldown;
        let body = combat.enemies[0].body;
        combat.rebase([64, -540]);
        assert_eq!(combat.frame().enemies[0].id, 2);
        assert_eq!(combat.frame().sleeping, 1);
        assert_eq!(combat.frame().sleeping_shots, 1);
        for _ in 0..40 {
            combat.tick(player, |_, y| y >= 552, |_, y| y >= 552);
        }
        assert_eq!(combat.enemies[0].cooldown, cooldown);
        assert_eq!(combat.enemies[0].health, 2);
        assert_eq!(combat.enemies[0].body.x().to_bits(), body.x().to_bits());
        assert_eq!(
            combat.enemies[0].body.vertical_velocity(),
            body.vertical_velocity()
        );
        assert_eq!(combat.parked_shots[0].1.remaining_ticks, 16);
        let far = [1_i64 << 54, -(1_i64 << 54)];
        combat.rebase(far);
        assert!(combat.frame().enemies.is_empty());
        assert_eq!(combat.enemies[0].origin, origin);
        assert_eq!(combat.enemies[0].body.x().to_bits(), body.x().to_bits());
        combat.rebase(origin);
        let first = combat
            .frame()
            .enemies
            .iter()
            .find(|e| e.id == 1)
            .copied()
            .unwrap();
        assert_eq!(first.health, 2);
        assert_eq!(first.rect[0] as i64 + origin[0], -180);
        assert_eq!(combat.shots.shots()[0].remaining_ticks, 16);
        assert_eq!(combat.shots.shots()[0].position, [800., 546.]);
        combat.hit_enemy([781, 541]);
        combat.hit_enemy([781, 541]);
        assert_eq!(combat.frame().kills, 1);
        combat.rebase(far);
        combat.rebase(origin);
        assert!(
            !combat.frame().enemies.iter().any(|e| e.id == 1),
            "killed enemy respawned on return"
        );
        assert_eq!(combat.frame().kills, 1);
    }
    #[test]
    fn sleeping_shots_share_the_bounded_fire_budget_and_report_rejection() {
        let mut combat = Combat::streamed(1920, 1080, [-960, -540]);
        combat.begin();
        let shot = Shot {
            position: [10., 10.],
            velocity: [5., 0.],
            acceleration: [0., 0.],
            remaining_ticks: 16,
        };
        combat.parked_shots.resize(1024, ([5000, 0], shot));
        combat.enemies[0].cooldown = 1;
        combat.tick(
            Body::new(960., 540., 7., 12.),
            |_, y| y >= 552,
            |_, y| y >= 552,
        );
        assert_eq!(combat.frame().rejected_shots, 1);
        assert_eq!(combat.parked_shots.len(), 1024);
        assert!(combat.shots.shots().is_empty());
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
            assert!(highest < 63., "hunter never jumped at obstacle");
            let x = combat.enemies[0].body.x();
            if height == 4 {
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
