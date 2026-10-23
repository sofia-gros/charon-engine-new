//! Fixed-tick point projectiles. The host supplies collision queries and owns
//! damage, destruction and effects. No dependency on sand or a particular game.

#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub position: [f32; 2],
    pub velocity: [f32; 2],
    pub remaining_ticks: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Impact {
    pub cell: [i32; 2],
    pub position: [f32; 2],
}

#[derive(Default)]
pub struct Projectiles {
    shots: Vec<Shot>,
}

impl Projectiles {
    pub fn shots(&self) -> &[Shot] {
        &self.shots
    }
    pub fn translate(&mut self, offset: [f32; 2]) {
        assert!(offset.into_iter().all(f32::is_finite));
        for shot in &mut self.shots {
            for (axis, delta) in offset.into_iter().enumerate() {
                shot.position[axis] += delta;
            }
        }
    }
    /// Prototype limits bound work: 1024 shots, 1024 cells/axis/tick, 3600 ticks.
    /// Reject invalid/excessive requests; never silently discard an active shot.
    pub fn spawn(&mut self, shot: Shot) -> Result<(), &'static str> {
        if self.shots.len() >= 1024 {
            return Err("projectile capacity reached");
        }
        if shot
            .position
            .iter()
            .any(|v| !v.is_finite() || v.abs() > 1_000_000.)
            || shot
                .velocity
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 1024.)
            || !(1..=3600).contains(&shot.remaining_ticks)
        {
            return Err("invalid projectile position, velocity or lifetime");
        }
        self.shots.push(shot);
        Ok(())
    }
    /// Query a stable collision state for the whole batch; apply returned impact
    /// requests afterward, so firing order does not change the queried terrain.
    pub fn step(&mut self, mut blocks: impl FnMut(i32, i32) -> bool) -> Vec<Impact> {
        let mut impacts = Vec::new();
        self.shots.retain_mut(|shot| {
            if let Some(impact) = sweep(shot.position, shot.velocity, &mut blocks) {
                impacts.push(impact);
                return false;
            }
            for axis in 0..2 {
                shot.position[axis] += shot.velocity[axis];
            }
            shot.remaining_ticks -= 1;
            shot.remaining_ticks != 0
        });
        impacts
    }
}

// Grid traversal checks every crossed cell rather than only the end position.
// Corner ties include both side neighbors, giving a conservative closed boundary.
fn sweep(
    start: [f32; 2],
    velocity: [f32; 2],
    blocks: &mut impl FnMut(i32, i32) -> bool,
) -> Option<Impact> {
    let mut cell = start.map(|v| v.floor() as i32);
    let direction = velocity.map(|v| {
        if v > 0. {
            1
        } else if v < 0. {
            -1
        } else {
            0
        }
    });
    let delta = velocity.map(|v| if v == 0. { f32::INFINITY } else { 1. / v.abs() });
    let mut next = [f32::INFINITY; 2];
    for axis in 0..2 {
        if direction[axis] != 0 {
            let edge = cell[axis] + i32::from(direction[axis] > 0);
            next[axis] = (edge as f32 - start[axis]) / velocity[axis];
        }
    }
    let mut t = 0.;
    loop {
        if blocks(cell[0], cell[1]) {
            return Some(Impact {
                cell,
                position: [start[0] + velocity[0] * t, start[1] + velocity[1] * t],
            });
        }
        t = next[0].min(next[1]);
        if t > 1. {
            return None;
        }
        if next[0] == next[1] {
            for side in [
                [cell[0] + direction[0], cell[1]],
                [cell[0], cell[1] + direction[1]],
            ] {
                if blocks(side[0], side[1]) {
                    return Some(Impact {
                        cell: side,
                        position: [start[0] + velocity[0] * t, start[1] + velocity[1] * t],
                    });
                }
            }
            for axis in 0..2 {
                cell[axis] += direction[axis];
                next[axis] += delta[axis];
            }
        } else {
            let axis = usize::from(next[1] < next[0]);
            cell[axis] += direction[axis];
            next[axis] += delta[axis];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn swept_shots_hit_first_thin_barrier_in_both_directions_and_at_corners() {
        for (position, velocity, wall) in [
            ([0.5, 2.5], [50., 0.], [7, 2]),
            ([20.5, 2.5], [-50., 0.], [7, 2]),
            ([0.5, 0.5], [10., 10.], [1, 0]),
            ([2.5, 0.5], [0., 50.], [2, 7]),
        ] {
            let mut world = Projectiles::default();
            world
                .spawn(Shot {
                    position,
                    velocity,
                    remaining_ticks: 10,
                })
                .unwrap();
            let hits = world.step(|x, y| [x, y] == wall);
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].cell, wall);
            assert!(world.shots().is_empty());
        }
    }
    #[test]
    fn lifetime_and_input_bounds_keep_work_finite() {
        let mut world = Projectiles::default();
        let mut shot = Shot {
            position: [0.5, 0.5],
            velocity: [0., 0.],
            remaining_ticks: 2,
        };
        world.spawn(shot).unwrap();
        assert!(world.step(|_, _| false).is_empty());
        assert_eq!(world.shots().len(), 1);
        world.step(|_, _| false);
        assert!(world.shots().is_empty());
        shot.velocity[0] = f32::NAN;
        assert!(world.spawn(shot).is_err());
        shot.velocity[0] = 1025.;
        assert!(world.spawn(shot).is_err());
    }
}
