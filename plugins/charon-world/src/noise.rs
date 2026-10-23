//! Seeded coherent value noise in Q16 fixed point. Sampling depends only on the
//! global integer coordinates, so chunk order and negative coordinates do not
//! introduce seams. No floating-point world coordinates or external dependency.
#[derive(Clone, Copy)]
pub struct Noise {
    seed: u64,
}
impl Noise {
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }
    fn lattice(self, x: i64, y: i64) -> i64 {
        let mut v = self.seed
            ^ (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
            ^ (y as u64).wrapping_mul(0xbf58476d1ce4e5b9);
        v = (v ^ (v >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        v = (v ^ (v >> 27)).wrapping_mul(0x94d049bb133111eb);
        v ^= v >> 31;
        ((v >> 48) as i64) * 2 - 65535
    }
    /// Smooth, bounded [-65535,65535] noise with a lattice period in cells.
    /// This is the lattice spacing, not a repetition period of the output.
    pub fn sample(self, x: i64, y: i64, spacing: u32) -> i32 {
        assert!(spacing > 0, "noise lattice spacing must be nonzero");
        let spacing = i64::from(spacing);
        let ix = x.div_euclid(spacing);
        let iy = y.div_euclid(spacing);
        fn smooth(t: i64) -> i64 {
            (((t * t) >> 16) * (3 * 65536 - 2 * t)) >> 16
        }
        fn lerp(a: i64, b: i64, t: i64) -> i64 {
            a + (((b - a) * t) >> 16)
        }
        let tx = smooth(x.rem_euclid(spacing) * 65536 / spacing);
        let ty = smooth(y.rem_euclid(spacing) * 65536 / spacing);
        let a = lerp(
            self.lattice(ix, iy),
            self.lattice(ix.wrapping_add(1), iy),
            tx,
        );
        let b = lerp(
            self.lattice(ix, iy.wrapping_add(1)),
            self.lattice(ix.wrapping_add(1), iy.wrapping_add(1)),
            tx,
        );
        lerp(a, b, ty) as i32
    }
    /// Three smooth scales: small features layered over broad terrain variation.
    pub fn fractal(self, x: i64, y: i64) -> i32 {
        (self.sample(x, y, 48) * 4 + self.sample(x, y, 128) * 2 + self.sample(x, y, 384)) / 7
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn noise_is_seeded_continuous_at_lattice_and_chunk_edges_and_world_bounded() {
        let noise = Noise::new(42);
        let mut seed_diff = false;
        for y in [-1025, -513, -512, -1, 0, 1, 511, 512, 1024] {
            for x in [-35840, -513, -512, -1, 0, 1, 511, 512, 35840] {
                let a = noise.fractal(x, y);
                assert!((-65535..=65535).contains(&a));
                assert_eq!(a, Noise::new(42).fractal(x, y));
                assert!(
                    (a - noise.fractal(x - 1, y)).abs() < 6000,
                    "discontinuity at {x},{y}"
                );
                seed_diff |= a != Noise::new(43).fractal(x, y);
            }
        }
        assert!(seed_diff);
        // Fractional world coordinates are never formed, even far beyond f32's
        // ability to distinguish adjacent cells.
        let far = (1_i64 << 54) + 511;
        assert_eq!(noise.sample(far, -far, 128), noise.sample(far, -far, 128));
        assert_ne!(noise.fractal(far, -far), noise.fractal(far + 9, -far));
    }
}
