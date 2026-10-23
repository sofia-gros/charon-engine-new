//! Minimal fixed-60-Hz kinematic controller. The game supplies its collision policy.
const DT: f32 = 1.0 / 60.0;
#[derive(Clone, Copy, Debug)]
pub struct Body {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    vy: f32,
    pub grounded: bool,
    /// New terrain could not be escaped within 16 cells. Editing terrain can free it.
    pub embedded: bool,
    /// Presentation must snap when recovery moves the body out of terrain.
    pub discontinuous: bool,
}
impl Body {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        assert!(
            [x, y, width, height].into_iter().all(f32::is_finite) && width > 0.0 && height > 0.0
        );
        Self {
            x,
            y,
            width,
            height,
            vy: 0.0,
            grounded: false,
            embedded: false,
            discontinuous: true,
        }
    }
    pub fn x(&self) -> f32 {
        self.x
    }
    pub fn y(&self) -> f32 {
        self.y
    }
    pub fn width(&self) -> f32 {
        self.width
    }
    pub fn vertical_velocity(&self) -> f32 {
        self.vy
    }
    pub fn height(&self) -> f32 {
        self.height
    }
    fn collision(&self, x: f32, y: f32, solid: &impl Fn(i32, i32) -> bool) -> Option<(i32, i32)> {
        for cy in y.floor() as i32..=(y + self.height - 0.0001).floor() as i32 {
            for cx in x.floor() as i32..=(x + self.width - 0.0001).floor() as i32 {
                if solid(cx, cy) {
                    return Some((cx, cy));
                }
            }
        }
        None
    }
    fn move_axis(
        &mut self,
        amount: f32,
        horizontal: bool,
        solid: &impl Fn(i32, i32) -> bool,
    ) -> bool {
        let steps = (amount.abs() / 0.5).ceil().max(1.0) as usize;
        let delta = amount / steps as f32;
        for _ in 0..steps {
            let (x, y) = if horizontal {
                (self.x + delta, self.y)
            } else {
                (self.x, self.y + delta)
            };
            if let Some((cx, cy)) = self.collision(x, y, solid) {
                if horizontal {
                    self.x = if delta > 0.0 {
                        cx as f32 - self.width
                    } else {
                        cx as f32 + 1.0
                    };
                } else {
                    self.y = if delta > 0.0 {
                        cy as f32 - self.height
                    } else {
                        cy as f32 + 1.0
                    };
                }
                return true;
            }
            self.x = x;
            self.y = y;
        }
        false
    }
    pub fn step(&mut self, horizontal: i8, jump: bool, solid: impl Fn(i32, i32) -> bool) {
        self.embedded = false;
        self.discontinuous = false;
        if self.collision(self.x, self.y, &solid).is_some() {
            // Explicit recovery policy for newly painted/fallen terrain. Never delete material.
            let mut free = None;
            'search: for distance in 1..=16 {
                let d = distance as f32;
                for (x, y) in [
                    (self.x, self.y - d),
                    (self.x - d, self.y),
                    (self.x + d, self.y),
                    (self.x, self.y + d),
                ] {
                    if self.collision(x, y, &solid).is_none() {
                        free = Some((x, y));
                        break 'search;
                    }
                }
            }
            if let Some((x, y)) = free {
                self.x = x;
                self.y = y;
                self.vy = 0.0;
                self.discontinuous = true;
            } else {
                self.embedded = true;
                self.grounded = false;
                self.vy = 0.0;
                return;
            }
        }
        self.grounded = self.collision(self.x, self.y + 0.05, &solid).is_some();
        if jump && self.grounded {
            self.vy = -140.0;
        }
        self.move_axis(horizontal.clamp(-1, 1) as f32 * 90.0 * DT, true, &solid);
        self.vy = (self.vy + 400.0 * DT).min(240.0);
        let falling = self.vy > 0.0;
        let hit = self.move_axis(self.vy * DT, false, &solid);
        self.grounded = falling && hit;
        if hit {
            self.vy = 0.0;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn landing_walls_jump_and_removed_support() {
        let mut body = Body::new(5.0, 0.0, 3.0, 6.0);
        let solid = |x, y| y >= 20 || x >= 30;
        for _ in 0..120 {
            body.step(0, false, solid);
        }
        assert_eq!(body.y(), 14.0);
        assert!(body.grounded);
        body.step(0, true, solid);
        assert!(body.y() < 14.0);
        assert!(!body.grounded);
        for _ in 0..120 {
            body.step(1, false, solid);
        }
        assert_eq!(body.x(), 27.0);
        assert_eq!(body.y(), 14.0);
        body.step(0, true, |x, y| y >= 40 || x >= 30);
        assert!(body.y() > 14.0, "jumped from removed support");
        for _ in 0..120 {
            body.step(0, false, |_, y| y >= 40);
        }
        assert_eq!(body.y(), 34.0);
    }
    #[test]
    fn fast_falls_do_not_cross_thin_floor_and_embedded_state_recovers() {
        let mut body = Body::new(2.0, 0.0, 2.0, 4.0);
        for _ in 0..300 {
            body.step(0, false, |_, y| y == 100);
        }
        assert_eq!(body.y(), 96.0);
        assert!(body.grounded);
        body.step(0, false, |_, _| true);
        assert!(body.embedded);
        body.step(0, false, |_, y| y >= 100);
        assert!(!body.embedded);
        assert!(body.grounded);
        let mut escaped = Body::new(2., 0., 2., 4.);
        escaped.step(0, false, |_, y| y == 0);
        assert!(escaped.discontinuous);
        escaped.step(0, false, |_, _| false);
        assert!(!escaped.discontinuous);
        // A low ceiling stops upward motion without moving through it.
        let mut body = Body::new(2.0, 6.0, 2.0, 4.0);
        for _ in 0..10 {
            body.step(0, true, |_, y| y >= 10 || y == 3);
            assert!(body.y() >= 4.0);
        }
    }
}
