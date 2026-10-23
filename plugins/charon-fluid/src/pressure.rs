//! Compact variable-density pressure system: MIC(0)-PCG with warm starting.
//! Grid coordinates are mapped once; Krylov iterations touch only liquid nodes.
use super::{Error, Fluid, WET};
const NONE: usize = usize::MAX;
struct Node {
    world: usize,
    adjacent: [usize; 4],
    weights: [f64; 4],
    diagonal: f64,
    density: f64,
    center: [f64; 2],
    normal: [f64; 2],
    alpha: f64,
}
impl Fluid {
    pub(super) fn project(
        &mut self,
        dt: f64,
        acceleration: [f64; 2],
        _stats: &mut super::Stats,
    ) -> Result<(usize, f64), Error> {
        let assembled = std::time::Instant::now();
        let w = self.config.width;
        let mut mapping = vec![NONE; self.state.fraction.len()];
        let mut nodes = Vec::new();
        for (world, f) in self.state.fraction.iter().enumerate() {
            let volume = f[0] + f[1];
            if volume <= self.config.pressure_cutoff.max(WET) {
                continue;
            }
            let mut center = [0.5, 0.5];
            let mut normal = [0., 0.];
            let mut alpha = 0.;
            if volume < 1. - 1e-8 {
                normal = self.normal(world, None);
                center = super::geometry::Polygon::square()
                    .partition(volume, normal)
                    .0
                    .centroid();
                alpha = super::geometry::plane_alpha(volume, normal);
            }
            mapping[world] = nodes.len();
            nodes.push(Node {
                world,
                adjacent: [NONE; 4],
                weights: [0.; 4],
                diagonal: 0.,
                center,
                normal,
                alpha,
                density: (f[0] * self.config.density[0] + f[1] * self.config.density[1])
                    / volume.max(WET),
            });
        }
        let n = nodes.len();
        if n == 0 {
            return Ok((0, 0.));
        }
        for index in 0..n {
            let i = nodes[index].world;
            let mut weights = [0.; 4];
            let mut adjacent = [NONE; 4];
            for (side, world) in [i - 1, i + 1, i - w, i + w].into_iter().enumerate() {
                if self.solid[world] {
                    continue;
                }
                let axis = side / 2;
                let other = mapping[world];
                adjacent[side] = other;
                if other != NONE {
                    let (a, b) = if world > i {
                        (&nodes[index], &nodes[other])
                    } else {
                        (&nodes[other], &nodes[index])
                    };
                    let distance = (1. + b.center[axis] - a.center[axis]).max(0.01);
                    weights[side] = 2. / ((a.density + b.density) * distance);
                } else {
                    let inside = &nodes[index];
                    let direction = if side % 2 == 0 { -1. } else { 1. };
                    let component = inside.normal[axis] * direction;
                    let distance = if component > 1e-8 {
                        (inside.alpha
                            - inside.normal[0] * inside.center[0]
                            - inside.normal[1] * inside.center[1])
                            / component
                    } else {
                        0.5
                    };
                    weights[side] = 1. / (inside.density * distance.clamp(0.01, 1.));
                }
            }
            nodes[index].diagonal = weights.iter().sum::<f64>().max(1e-20);
            nodes[index].weights = weights;
            nodes[index].adjacent = adjacent;
        }
        // Well-balanced hydrostatic force at displaced liquid pressure points.
        let damping = 1. / (1. + self.config.drag * dt);
        for node in &nodes {
            let x = node.world % w;
            let y = node.world / w;
            let right = node.adjacent[1];
            if right != NONE {
                let other = &nodes[right];
                let distance = (1. + other.center[0] - node.center[0]).max(0.01);
                self.state.u[y * (w + 1) + x + 1] +=
                    dt * damping * acceleration[1] * (other.center[1] - node.center[1]) / distance;
            }
            let below = node.adjacent[3];
            if below != NONE {
                let other = &nodes[below];
                let distance = (1. + other.center[1] - node.center[1]).max(0.01);
                self.state.v[node.world + w] +=
                    dt * damping * acceleration[0] * (other.center[0] - node.center[0]) / distance;
            }
        }
        let divergence = |i: usize| {
            let x = i % w;
            let y = i / w;
            self.state.u[y * (w + 1) + x + 1] - self.state.u[y * (w + 1) + x] + self.state.v[i + w]
                - self.state.v[i]
        };
        let rhs: Vec<_> = nodes
            .iter()
            .map(|node| -divergence(node.world) / dt)
            .collect();
        let mut p: Vec<_> = nodes
            .iter()
            .map(|node| self.state.pressure[node.world])
            .collect();
        _stats.pressure_assembly_ms += assembled.elapsed().as_secs_f64() * 1000.;
        #[cfg(feature = "gpu")]
        let gpu_solution = self
            .gpu
            .as_ref()
            .map(|gpu| {
                let rows: Vec<_> = nodes
                    .iter()
                    .map(|node| (node.adjacent, node.weights, node.diagonal))
                    .collect();
                gpu.solve(&rows, &rhs, &mut p)
            })
            .transpose()?;
        #[cfg(not(feature = "gpu"))]
        let gpu_solution: Option<usize> = None;
        let iterations = match gpu_solution {
            Some(result) => {
                #[cfg(feature = "gpu")]
                {
                    _stats
                        .gpu
                        .as_mut()
                        .expect("GPU step metrics")
                        .add_work(true, result.1);
                    result.0
                }
                #[cfg(not(feature = "gpu"))]
                {
                    result
                }
            }
            None => solve_cpu(&nodes, &rhs, &mut p)?,
        };
        for (i, node) in nodes.iter().enumerate() {
            let world = node.world;
            let x = world % w;
            let y = world / w;
            let neighbor = |side| {
                if node.adjacent[side] == NONE {
                    0.
                } else {
                    p[node.adjacent[side]]
                }
            };
            self.state.u[y * (w + 1) + x + 1] -= dt * node.weights[1] * (neighbor(1) - p[i]);
            if node.adjacent[0] == NONE {
                self.state.u[y * (w + 1) + x] -= dt * node.weights[0] * p[i];
            }
            self.state.v[world + w] -= dt * node.weights[3] * (neighbor(3) - p[i]);
            if node.adjacent[2] == NONE {
                self.state.v[world] -= dt * node.weights[2] * p[i];
            }
        }
        self.state.pressure.fill(0.);
        for (node, value) in nodes.iter().zip(p) {
            self.state.pressure[node.world] = value;
        }
        let maximum = nodes
            .iter()
            .map(|node| {
                let i = node.world;
                let x = i % w;
                let y = i / w;
                (self.state.u[y * (w + 1) + x + 1] - self.state.u[y * (w + 1) + x]
                    + self.state.v[i + w]
                    - self.state.v[i])
                    .abs()
            })
            .fold(0., f64::max);
        Ok((iterations, maximum))
    }
}

fn solve_cpu(nodes: &[Node], rhs: &[f64], p: &mut [f64]) -> Result<usize, Error> {
    let n = nodes.len();
    let mut factor = vec![0_f64; n];
    for (i, node) in nodes.iter().enumerate() {
        let mut d = node.diagonal;
        for side in [0, 2] {
            let j = node.adjacent[side];
            if j != NONE {
                d -= (node.weights[side] / factor[j]).powi(2);
                let cross = if side == 0 { 3 } else { 1 };
                if nodes[j].adjacent[cross] != NONE {
                    d -= 0.97 * node.weights[side] * nodes[j].weights[cross] / factor[j].powi(2);
                }
            }
        }
        if d < 0.25 * node.diagonal {
            d = node.diagonal;
        }
        factor[i] = d.sqrt();
    }
    let apply = |x: &[f64], result: &mut [f64]| {
        for (i, node) in nodes.iter().enumerate() {
            let sum = node
                .adjacent
                .iter()
                .zip(&node.weights)
                .map(|(&j, &a)| if j == NONE { 0. } else { a * x[j] })
                .sum::<f64>();
            result[i] = node.diagonal * x[i] - sum;
        }
    };
    let precondition = |r: &[f64], z: &mut [f64]| {
        for (i, node) in nodes.iter().enumerate() {
            let mut sum = 0.;
            for side in [0, 2] {
                let j = node.adjacent[side];
                if j != NONE {
                    sum += node.weights[side] / factor[j] * z[j];
                }
            }
            z[i] = (r[i] + sum) / factor[i];
        }
        for (i, node) in nodes.iter().enumerate().rev() {
            let mut sum = 0.;
            for side in [1, 3] {
                let j = node.adjacent[side];
                if j != NONE {
                    sum += node.weights[side] / factor[i] * z[j];
                }
            }
            z[i] = (z[i] + sum) / factor[i];
        }
    };
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
    let mut product = vec![0.; n];
    apply(p, &mut product);
    let mut r: Vec<_> = rhs.iter().zip(&product).map(|(b, ap)| b - ap).collect();
    let mut z = vec![0.; n];
    precondition(&r, &mut z);
    let mut direction = z.clone();
    let mut rz = dot(&r, &z);
    let tolerance = (dot(rhs, rhs).sqrt() * 1e-8).max(1e-9);
    let mut iterations = 0;
    while dot(&r, &r).sqrt() > tolerance {
        if iterations >= 600 {
            return Err(Error::Pressure);
        }
        apply(&direction, &mut product);
        let denominator = dot(&direction, &product);
        if !denominator.is_finite() || denominator <= 0. {
            return Err(Error::Pressure);
        }
        let alpha = rz / denominator;
        for i in 0..n {
            p[i] += alpha * direction[i];
            r[i] -= alpha * product[i];
        }
        precondition(&r, &mut z);
        let next = dot(&r, &z);
        let beta = next / rz;
        for i in 0..n {
            direction[i] = z[i] + beta * direction[i];
        }
        rz = next;
        iterations += 1;
    }
    Ok(iterations)
}
