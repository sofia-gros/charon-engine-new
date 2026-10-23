//! Staggered semi-Lagrangian momentum transport, forces and free-surface extension.
use super::{Fluid, WET};
use rayon::prelude::*;
impl Fluid {
    pub(super) fn advect_velocity(&mut self, dt: f64) -> u32 {
        let w = self.config.width;
        let h = self.config.height;
        let u = &self.state.u;
        let v = &self.state.v;
        let sample = |field: &[f64], fw: usize, fh: usize, x: f64, y: f64| {
            let x = x.clamp(0., (fw - 1) as f64);
            let y = y.clamp(0., (fh - 1) as f64);
            let ix = x.floor() as usize;
            let iy = y.floor() as usize;
            let jx = (ix + 1).min(fw - 1);
            let jy = (iy + 1).min(fh - 1);
            let fx = x - ix as f64;
            let fy = y - iy as f64;
            (field[iy * fw + ix] * (1. - fx) + field[iy * fw + jx] * fx) * (1. - fy)
                + (field[jy * fw + ix] * (1. - fx) + field[jy * fw + jx] * fx) * fy
        };
        let diffusion = dt * self.config.viscosity;
        let workers = std::sync::atomic::AtomicU64::new(0);
        let mut next_u = vec![0.; u.len()];
        let mut next_v = vec![0.; v.len()];
        let Some(b) = self.bounds() else {
            self.state.u = next_u;
            self.state.v = next_v;
            return 0;
        };
        let x0 = b[0].saturating_sub(3).max(1);
        let x1 = (b[2] + 3).min(w);
        let y0 = b[1].saturating_sub(3).max(1);
        let y1 = (b[3] + 3).min(h);
        self.pool.install(|| {
            next_u
                .par_chunks_mut(w + 1)
                .enumerate()
                .filter(|(y, _)| *y >= y0 && *y < y1)
                .for_each(|(y, row)| {
                    workers.fetch_or(
                        1 << rayon::current_thread_index().unwrap_or(0),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    for (x, cell) in row.iter_mut().enumerate().take(x1).skip(x0) {
                        let i = y * (w + 1) + x;
                        let px = x as f64;
                        let py = y as f64 + 0.5;
                        let vy = sample(v, w, h + 1, px - 0.5, py);
                        let lap = if y + 1 < h {
                            u[i - 1] + u[i + 1] + u[i - (w + 1)] + u[i + w + 1] - 4. * u[i]
                        } else {
                            0.
                        };
                        *cell = sample(u, w + 1, h, px - dt * u[i], py - 0.5 - dt * vy)
                            + diffusion * lap;
                    }
                });
            next_v
                .par_chunks_mut(w)
                .enumerate()
                .filter(|(y, _)| *y >= y0 && *y < y1)
                .for_each(|(y, row)| {
                    workers.fetch_or(
                        1 << rayon::current_thread_index().unwrap_or(0),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    for (x, cell) in row.iter_mut().enumerate().take(x1).skip(x0) {
                        let i = y * w + x;
                        let px = x as f64 + 0.5;
                        let py = y as f64;
                        let ux = sample(u, w + 1, h, px, py - 0.5);
                        let lap = if x + 1 < w {
                            v[i - 1] + v[i + 1] + v[i - w] + v[i + w] - 4. * v[i]
                        } else {
                            0.
                        };
                        *cell = sample(v, w, h + 1, px - 0.5 - dt * ux, py - dt * v[i])
                            + diffusion * lap;
                    }
                });
        });
        self.state.u = next_u;
        self.state.v = next_v;
        workers
            .load(std::sync::atomic::Ordering::Relaxed)
            .count_ones()
    }
    pub(super) fn forces(&mut self, dt: f64, a: [f64; 2]) {
        let w = self.config.width;
        let h = self.config.height;
        let Some(b) = self.bounds() else {
            return;
        };
        let x0 = b[0].saturating_sub(4).max(1);
        let x1 = (b[2] + 4).min(w);
        let y0 = b[1].saturating_sub(4).max(1);
        let y1 = (b[3] + 4).min(h);
        let damping = 1. / (1. + self.config.drag * dt);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * (w + 1) + x;
                if x == 0
                    || x == w
                    || self.solid[y * w + x - 1]
                    || self.solid[y * w + x]
                    || (self.state.fraction[y * w + x - 1].iter().sum::<f64>() <= WET
                        && self.state.fraction[y * w + x].iter().sum::<f64>() <= WET)
                {
                    self.state.u[i] = 0.;
                } else {
                    self.state.u[i] = (self.state.u[i] + a[0] * dt) * damping;
                }
            }
        }
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * w + x;
                if y == 0
                    || y == h
                    || self.solid[(y - 1) * w + x]
                    || self.solid[y * w + x]
                    || (self.state.fraction[(y - 1) * w + x].iter().sum::<f64>() <= WET
                        && self.state.fraction[y * w + x].iter().sum::<f64>() <= WET)
                {
                    self.state.v[i] = 0.;
                } else {
                    self.state.v[i] = (self.state.v[i] + a[1] * dt) * damping;
                }
            }
        }
    }
    /// Extend projected liquid velocities to the free-surface band. Pressure
    /// unknowns lie inside liquid; fractional cells outside those centers must
    /// inherit this velocity rather than accumulate an independent gas gravity.
    pub(super) fn extrapolate_velocity(&mut self) {
        let w = self.config.width;
        let h = self.config.height;
        let Some(b) = self.bounds() else {
            return;
        };
        let x0 = b[0].saturating_sub(4).max(1);
        let x1 = (b[2] + 4).min(w);
        let y0 = b[1].saturating_sub(4).max(1);
        let y1 = (b[3] + 4).min(h);
        let wet: Vec<_> = self
            .state
            .fraction
            .iter()
            .map(|f| f[0] + f[1] > self.config.pressure_cutoff.max(WET))
            .collect();
        let mut valid_u = vec![false; (w + 1) * h];
        let mut valid_v = vec![false; w * (h + 1)];
        let mut blocked_u = vec![false; (w + 1) * h];
        let mut blocked_v = vec![false; w * (h + 1)];
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * (w + 1) + x;
                blocked_u[i] =
                    x == 0 || x == w || self.solid[y * w + x - 1] || self.solid[y * w + x];
                valid_u[i] = !blocked_u[i] && (wet[y * w + x - 1] || wet[y * w + x]);
            }
        }
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * w + x;
                blocked_v[i] =
                    y == 0 || y == h || self.solid[(y - 1) * w + x] || self.solid[y * w + x];
                valid_v[i] = !blocked_v[i] && (wet[(y - 1) * w + x] || wet[y * w + x]);
            }
        }
        fn extend(
            values: &mut [f64],
            valid: &mut [bool],
            blocked: &[bool],
            w: usize,
            h: usize,
            bounds: [usize; 4],
        ) {
            for _ in 0..3 {
                let mut updates = Vec::new();
                for y in bounds[1]..bounds[3].min(h - 1) {
                    for x in bounds[0]..bounds[2].min(w - 1) {
                        let i = y * w + x;
                        if valid[i] || blocked[i] {
                            continue;
                        }
                        let mut count = 0;
                        let mut sum = 0.;
                        for j in [i - 1, i + 1, i - w, i + w] {
                            if valid[j] {
                                sum += values[j];
                                count += 1;
                            }
                        }
                        if count > 0 {
                            updates.push((i, sum / f64::from(count)));
                        }
                    }
                }
                for (i, value) in updates {
                    values[i] = value;
                    valid[i] = true;
                }
            }
        }
        extend(
            &mut self.state.u,
            &mut valid_u,
            &blocked_u,
            w + 1,
            h,
            [x0, y0, x1, y1],
        );
        extend(
            &mut self.state.v,
            &mut valid_v,
            &blocked_v,
            w,
            h + 1,
            [x0, y0, x1, y1],
        );
    }
}
