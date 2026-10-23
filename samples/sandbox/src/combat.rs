//! Rules for the prototype game. Core/projectiles do not know health or teams.
use charon_platformer::Body;
use charon_projectiles::{Projectiles, Shot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Ready,
    Playing,
    Defeated,
}
#[derive(Clone, Copy)]
pub struct EnemyView {
    pub rect: [f32; 4],
    pub health: u8,
}
pub struct Frame {
    pub phase: Phase,
    pub health: u8,
    pub enemies: Vec<EnemyView>,
    pub shots: Vec<[f32; 2]>,
    pub kills: u32,
}
struct Enemy {
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
            enemy.body.step(0, false, &actor_solid);
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
