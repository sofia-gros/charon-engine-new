//! Conservative geometric two-phase VOF transport with bounded face fluxes.
use super::geometry::Polygon;
use super::{Error, Fluid, WET};
impl Fluid {
    pub(super) fn transport(&mut self, dt: f64) -> Result<(), Error> {
        let w = self.config.width;
        let h = self.config.height;
        // Each entry is a directed finite-volume face flux, not a particle.
        let mut fluxes: Vec<(usize, usize, [f64; 2])> = Vec::new();
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let i = y * w + x;
                let f = self.state.fraction[i];
                let total = f[0] + f[1];
                if total <= 1e-16 {
                    continue;
                }
                let fluid = if total >= 1. - 1e-12 {
                    Polygon::square()
                } else {
                    Polygon::square().partition(total, self.normal(i, None)).0
                };
                let (first, second) = fluid.partition(f[0], self.normal(i, Some(0)));
                for (neighbor, axis, high, speed) in [
                    (i - 1, 0, false, -self.state.u[y * (w + 1) + x]),
                    (i + 1, 0, true, self.state.u[y * (w + 1) + x + 1]),
                    (i - w, 1, false, -self.state.v[y * w + x]),
                    (i + w, 1, true, self.state.v[(y + 1) * w + x]),
                ] {
                    if speed <= 0. || self.solid[neighbor] {
                        continue;
                    }
                    let distance = (speed * dt).min(1.);
                    let amounts = [
                        first.strip(axis, high, distance),
                        second.strip(axis, high, distance),
                    ];
                    if amounts[0] + amounts[1] > 0. {
                        fluxes.push((i, neighbor, amounts));
                    }
                }
            }
        }
        // Bound outgoing phase volumes, then incoming mixture capacity. All
        // limiting is on shared face fluxes: no clipping/deleting excess liquid.
        let n = w * h;
        let mut used = vec![false; n];
        for &(source, dest, _) in &fluxes {
            used[source] = true;
            used[dest] = true;
        }
        let nodes: Vec<_> = used
            .iter()
            .enumerate()
            .filter_map(|(i, &used)| used.then_some(i))
            .collect();
        let mut outgoing = vec![[0.; 2]; n];
        for &(source, _, amounts) in &fluxes {
            for (p, a) in amounts.iter().enumerate() {
                outgoing[source][p] += a;
            }
        }
        for (source, _, amounts) in &mut fluxes {
            for (p, a) in amounts.iter_mut().enumerate() {
                if outgoing[*source][p] > 0. {
                    *a *= (self.state.fraction[*source][p] / outgoing[*source][p]).clamp(0., 1.);
                }
            }
        }
        // Correct overfull receivers through the reverse face-flow graph.
        // Find available capacity, then reduce fluxes along that complete path.
        // Interior balances stay unchanged; unlike repeated global scaling this
        // cannot spend hundreds of iterations attenuating a circulation cycle.
        let none = usize::MAX;
        let mut heads = vec![none; n];
        let mut links = vec![none; fluxes.len()];
        let mut balance: Vec<_> = self.state.fraction.iter().map(|f| f[0] + f[1]).collect();
        for (edge, &(source, dest, a)) in fluxes.iter().enumerate() {
            links[edge] = heads[dest];
            heads[dest] = edge;
            let volume = a[0] + a[1];
            balance[source] -= volume;
            balance[dest] += volume;
        }
        let mut visited = vec![0_u32; n];
        let mut parent = vec![none; n];
        let mut queue = Vec::new();
        let mut generation = 0;
        let mut budget = 4_000_000_usize;
        for &origin in &nodes {
            while balance[origin] > 1. + 1e-8 {
                generation += 1;
                queue.clear();
                queue.push(origin);
                visited[origin] = generation;
                let mut head = 0;
                let mut end = None;
                while head < queue.len() {
                    let i = queue[head];
                    head += 1;
                    budget = budget.checked_sub(1).ok_or(Error::VolumeBounds)?;
                    if balance[i] < 1. - 1e-14 {
                        end = Some(i);
                        break;
                    }
                    let mut edge = heads[i];
                    while edge != none {
                        let (source, _, a) = fluxes[edge];
                        if a[0] + a[1] > 1e-14 && visited[source] != generation {
                            visited[source] = generation;
                            parent[source] = edge;
                            queue.push(source);
                        }
                        edge = links[edge];
                    }
                }
                let end = end.ok_or(Error::VolumeBounds)?;
                let mut amount = (balance[origin] - 1.).min(1. - balance[end]);
                let mut i = end;
                while i != origin {
                    let (_, dest, a) = fluxes[parent[i]];
                    amount = amount.min(a[0] + a[1]);
                    i = dest;
                }
                if amount <= 1e-15 {
                    return Err(Error::VolumeBounds);
                }
                let mut i = end;
                while i != origin {
                    let edge = parent[i];
                    let (_, dest, a) = fluxes[edge];
                    let sum = a[0] + a[1];
                    let factor = (1. - amount / sum).max(0.);
                    fluxes[edge].2 = [a[0] * factor, a[1] * factor];
                    i = dest;
                }
                balance[origin] -= amount;
                balance[end] += amount;
            }
        }
        for (source, dest, amounts) in fluxes {
            for (p, a) in amounts.iter().enumerate() {
                self.state.fraction[source][p] -= a;
                self.state.fraction[dest][p] += a;
            }
        }
        // Resolve only roundoff/limiter residuals, conserving both species.
        // Do not clip volume: return the tiny excess to available space through
        // the connected fluid. Large errors are rejected transactionally.
        let mut seen = vec![0_u32; n];
        let mut generation = 0;
        let mut queue = Vec::new();
        for &source in &nodes {
            let total = self.state.fraction[source].iter().sum::<f64>();
            if total <= 1. + 1e-10 {
                continue;
            }
            let mut excess = total - 1.;
            if excess > 2e-7 {
                return Err(Error::VolumeBounds);
            }
            generation += 1;
            queue.clear();
            queue.push(source);
            seen[source] = generation;
            let mut head = 0;
            while excess > 1e-14 && head < queue.len() {
                let i = queue[head];
                head += 1;
                let volume = self.state.fraction[i].iter().sum::<f64>();
                let amount = excess.min((1. - volume).max(0.));
                if amount > 0. {
                    let original = self.state.fraction[source];
                    let sum = original[0] + original[1];
                    for (p, value) in original.into_iter().enumerate() {
                        let v = amount * value / sum;
                        self.state.fraction[source][p] -= v;
                        self.state.fraction[i][p] += v;
                    }
                    excess -= amount;
                }
                if volume > WET {
                    for j in [i - w, i + w, i - 1, i + 1] {
                        if seen[j] != generation && !self.solid[j] {
                            seen[j] = generation;
                            queue.push(j);
                        }
                    }
                }
            }
            if excess > 1e-12 {
                return Err(Error::VolumeBounds);
            }
        }
        if self.state.fraction.iter().any(|v| {
            v[0] < -1e-10
                || v[1] < -1e-10
                || v[0] + v[1] > 1. + 1e-8
                || !v.iter().all(|f| f.is_finite())
        }) {
            return Err(Error::VolumeBounds);
        }
        Ok(())
    }
}
