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
}
struct Enemy {
    behavior: Behavior,
    body: Body,
    health: u8,
    cooldown: u16,
}
pub struct Combat {
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
            phase: if ready { Phase::Ready } else { Phase::Playing },
            health: 3,
            enemies: vec![Enemy {
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
            .filter(|e| e.health > 0)
            .map(|e| rect(e.body))
            .collect()
    }
    /// Keep dead bodies addressable until the batch ends: simultaneous impacts
    /// on a killed enemy must not turn into terrain destruction requests.
    pub fn hit_enemy(&mut self, cell: [i32; 2]) -> bool {
        if let Some(enemy) = self
            .enemies
            .iter_mut()
            .find(|e| contains(rect(e.body), cell[0], cell[1]))
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
            if enemy.health == 0 {
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
                // One enemy, one shot per 90 ticks, lifetime 180: at most 2 live.
                self.shots
                    .spawn(Shot {
                        position,
                        velocity,
                        acceleration: [0., 0.],
                        remaining_ticks: 180,
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
                .filter(|e| e.health > 0)
                .map(|e| EnemyView {
                    rect: rect(e.body),
                    health: e.health,
                    behavior: e.behavior,
                })
                .collect(),
            shots: self.shots.shots().iter().map(|s| s.position).collect(),
            kills: self.kills,
        }
    }
    pub fn signature(&self) -> Vec<u64> {
        let mut values = vec![
            self.phase as u64,
            self.health as u64,
            self.invulnerable as u64,
            self.kills as u64,
        ];
        for enemy in &self.enemies {
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
