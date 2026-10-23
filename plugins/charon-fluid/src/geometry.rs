//! Convex half-plane cuts for conservative, geometric volume-of-fluid fluxes.
#[derive(Clone, Copy, Default)]
pub(crate) struct Polygon {
    points: [[f64; 2]; 10],
    len: usize,
}
impl Polygon {
    pub fn square() -> Self {
        let mut p = Self::default();
        for point in [[0., 0.], [1., 0.], [1., 1.], [0., 1.]] {
            p.points[p.len] = point;
            p.len += 1;
        }
        p
    }
    pub fn area(&self) -> f64 {
        if self.len < 3 {
            return 0.;
        }
        (0..self.len)
            .map(|i| {
                let a = self.points[i];
                let b = self.points[(i + 1) % self.len];
                a[0] * b[1] - a[1] * b[0]
            })
            .sum::<f64>()
            .abs()
            * 0.5
    }
    pub fn centroid(&self) -> [f64; 2] {
        let mut area = 0.;
        let mut result = [0., 0.];
        for i in 0..self.len {
            let a = self.points[i];
            let b = self.points[(i + 1) % self.len];
            let cross = a[0] * b[1] - a[1] * b[0];
            area += cross;
            result[0] += (a[0] + b[0]) * cross;
            result[1] += (a[1] + b[1]) * cross;
        }
        if area.abs() < 1e-15 {
            [0.5, 0.5]
        } else {
            [result[0] / (3. * area), result[1] / (3. * area)]
        }
    }
    /// Integrate the squared, linearly reconstructed MAC velocity over liquid,
    /// not over the empty part of a cut cell. Triangle quadrature is exact for
    /// this quadratic and avoids cancellation in very thin wall films.
    pub fn motion(&self, u: [f64; 2], v: [f64; 2]) -> (f64, f64) {
        let velocity = |p: [f64; 2]| {
            [
                (1. - p[0]) * u[0] + p[0] * u[1],
                (1. - p[1]) * v[0] + p[1] * v[1],
            ]
        };
        let speed = self.points[..self.len]
            .iter()
            .flat_map(|&p| velocity(p))
            .map(f64::abs)
            .fold(0., f64::max);
        let mut energy = 0.;
        if self.len < 3 {
            return (0., speed);
        }
        let a = self.points[0];
        for i in 1..self.len - 1 {
            let b = self.points[i];
            let c = self.points[i + 1];
            let area = ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() * 0.5;
            for weights in [
                [2. / 3., 1. / 6., 1. / 6.],
                [1. / 6., 2. / 3., 1. / 6.],
                [1. / 6., 1. / 6., 2. / 3.],
            ] {
                let p = [
                    weights[0] * a[0] + weights[1] * b[0] + weights[2] * c[0],
                    weights[0] * a[1] + weights[1] * b[1] + weights[2] * c[1],
                ];
                let value = velocity(p);
                energy += area / 3. * (value[0].powi(2) + value[1].powi(2));
            }
        }
        (energy, speed)
    }
    pub fn cut(&self, normal: [f64; 2], alpha: f64) -> Self {
        let mut result = Self::default();
        if self.len == 0 {
            return result;
        }
        for i in 0..self.len {
            let a = self.points[i];
            let b = self.points[(i + 1) % self.len];
            let da = a[0] * normal[0] + a[1] * normal[1] - alpha;
            let db = b[0] * normal[0] + b[1] * normal[1] - alpha;
            if da <= 0. {
                result.points[result.len] = a;
                result.len += 1;
            }
            if (da < 0. && db > 0.) || (da > 0. && db < 0.) {
                let t = da / (da - db);
                result.points[result.len] = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
                result.len += 1;
            }
        }
        result
    }
    pub fn partition(&self, area: f64, normal: [f64; 2]) -> (Self, Self) {
        if area <= 1e-14 {
            return (Self::default(), *self);
        }
        if area >= self.area() - 1e-14 {
            return (*self, Self::default());
        }
        if self.len == 4 && (self.area() - 1.).abs() < 1e-14 {
            // Analytic inverse of the area cut from a unit square. Most VOF
            // interface cells need no iterative polygon search.
            let alpha = plane_alpha(area, normal);
            return (
                self.cut(normal, alpha),
                self.cut([-normal[0], -normal[1]], -alpha),
            );
        }
        let span = normal[0].abs() + normal[1].abs();
        let (mut lo, mut hi) = (-span, span);
        for _ in 0..42 {
            let mid = (lo + hi) * 0.5;
            if self.cut(normal, mid).area() < area {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let alpha = (lo + hi) * 0.5;
        (
            self.cut(normal, alpha),
            self.cut([-normal[0], -normal[1]], -alpha),
        )
    }
    pub fn strip(&self, axis: usize, high: bool, distance: f64) -> f64 {
        let mut normal = [0., 0.];
        normal[axis] = if high { -1. } else { 1. };
        self.cut(normal, if high { distance - 1. } else { distance })
            .area()
    }
}

pub(crate) fn plane_alpha(area: f64, normal: [f64; 2]) -> f64 {
    let a = normal[0].abs();
    let b = normal[1].abs();
    let small = a.min(b);
    let large = a.max(b);
    let f = area.min(1. - area);
    let t = if small < 1e-14 {
        large * f
    } else if f < small / (2. * large) {
        (2. * a * b * f).sqrt()
    } else {
        large * f + small * 0.5
    };
    let t = if area > 0.5 { a + b - t } else { t };
    t + normal[0].min(0.) + normal[1].min(0.)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thin_film_energy_excludes_fast_velocity_in_the_empty_cell_region() {
        let f = 0.02;
        let film = Polygon::square().partition(f, [0., -1.]).0;
        let (energy, speed) = film.motion([0., 0.], [12., 0.]);
        assert!((energy - 144. * f * f * f / 3.).abs() < 1e-12);
        assert!((speed - 12. * f).abs() < 1e-12);
    }
    #[test]
    fn oblique_interfaces_partition_volume_and_face_flux_without_overlap() {
        for normal in [[1., 0.], [0., -1.], [0.3, -0.8], [-0.7, -0.2]] {
            for volume in [0.001, 0.2, 0.5, 0.9, 0.999] {
                let (water, oil) = Polygon::square().partition(volume, normal);
                assert!((water.area() - volume).abs() < 1e-10);
                assert!((oil.area() + water.area() - 1.).abs() < 1e-10);
                for axis in 0..2 {
                    for high in [false, true] {
                        assert!(
                            (water.strip(axis, high, 0.2) + oil.strip(axis, high, 0.2) - 0.2).abs()
                                < 1e-10
                        );
                    }
                }
            }
        }
    }
}
