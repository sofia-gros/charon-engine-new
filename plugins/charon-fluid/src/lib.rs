//! Eulerian two-phase liquid: MAC velocity/pressure projection and geometric VOF.
//! No particles or prescribed surface animation. Units: cell, second, relative density.
mod geometry;
#[cfg(feature = "gpu")]
pub mod gpu;
mod pressure;
#[cfg(test)]
mod tests;
mod transport;
mod velocity;

// Fractions below one millionth of a cell retain mass but do not define a
// pressure surface; otherwise solver roundoff nucleates artificial fluid layers.
const WET: f64 = 1e-6;
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub width: usize,
    pub height: usize,
    pub workers: usize,
    pub density: [f64; 2],
    /// Linear momentum damping (s^-1), independent of gravity and pressure.
    pub drag: f64,
    /// Effective kinematic viscosity in cell^2/s.
    pub viscosity: f64,
    /// Subcell pressure-DOF cutoff; excluded volume remains in conservative VOF.
    pub pressure_cutoff: f64,
    /// Maximum kinetic head (cells) and phase-fraction change per tick for sleep.
    /// Zero disables sleep; callers choose this resolution/accuracy policy.
    pub sleep_tolerance: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Error {
    InvalidInput,
    Occupied,
    Pressure,
    VolumeBounds,
    TimeStep,
    Backend,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct GpuStats {
    /// Actual compute-pass duration. None means queries unavailable/disabled,
    /// never zero as a substitute for an unsupported measurement.
    pub velocity_ms: Option<f64>,
    pub pressure_ms: Option<f64>,
    /// Explicit queue uploads and readbacks, including timestamp data.
    pub upload_bytes: u64,
    pub readback_bytes: u64,
    pub dispatches: u32,
    /// CPU pressure preparation and f64 residual checks outside GPU solve calls.
    /// Includes small-path upload enqueueing; excludes GPU execution/waiting.
    /// This is a subset of Stats::pressure_ms, not its entire CPU portion.
    pub pressure_setup_ms: f64,
    /// Completed compute/readback submissions (CPU/GPU synchronization count).
    pub readbacks: u32,
    /// Largest dispatch in this tick, not simultaneous hardware occupancy.
    pub velocity_workgroups: u32,
    pub pressure_workgroups: u32,
}
#[cfg(feature = "gpu")]
impl GpuStats {
    fn new(gpu: &gpu::GpuContext) -> Self {
        Self {
            velocity_ms: gpu.empty_work().kernel_ms,
            pressure_ms: gpu.empty_work().kernel_ms,
            ..Self::default()
        }
    }
    fn add_work(&mut self, pressure: bool, work: gpu::Work) {
        let target = if pressure {
            &mut self.pressure_ms
        } else {
            &mut self.velocity_ms
        };
        *target = target.zip(work.kernel_ms).map(|(a, b)| a + b);
        if pressure {
            self.pressure_setup_ms += work.setup_ms;
        }
        self.readbacks += work.readbacks;
        self.upload_bytes += work.upload_bytes;
        self.readback_bytes += work.readback_bytes;
        self.dispatches += work.dispatches;
        let groups = if pressure {
            &mut self.pressure_workgroups
        } else {
            &mut self.velocity_workgroups
        };
        *groups = (*groups).max(work.max_workgroups);
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub volume: [f64; 2],
    pub wet_cells: usize,
    pub active_tiles: usize,
    pub substeps: usize,
    pub pressure_iterations: usize,
    pub flux_retries: usize,
    pub max_divergence: f64,
    pub max_speed: f64,
    pub rms_speed: f64,
    /// Maximum sqrt(volume * squared speed) per cell. Gas extension velocities
    /// must not keep resting liquid awake; a moving full cell must always do so.
    pub max_mass_speed: f64,
    pub max_fraction_change: f64,
    pub volume_change: f64,
    pub workers_used: u32,
    pub sleeping: bool,
    pub pressure_ms: f64,
    /// CPU construction of liquid pressure nodes, coefficients and RHS.
    pub pressure_assembly_ms: f64,
    pub transport_ms: f64,
    pub velocity_ms: f64,
    pub gpu: Option<GpuStats>,
}
#[derive(Clone)]
struct State {
    fraction: Vec<[f64; 2]>,
    u: Vec<f64>,
    v: Vec<f64>,
    pressure: Vec<f64>,
}
pub struct Fluid {
    #[cfg(feature = "gpu")]
    gpu: Option<std::sync::Arc<gpu::GpuContext>>,
    config: Config,
    solid: Vec<bool>,
    state: State,
    backup: State,
    pool: rayon::ThreadPool,
    idle_ticks: usize,
    sleeping: bool,
    stats: Stats,
    acceleration: Option<[f64; 2]>,
}
impl Fluid {
    pub fn new(config: Config, solid: Vec<bool>) -> Result<Self, Error> {
        let Config {
            width: w,
            height: h,
            workers,
            density,
            drag,
            viscosity,
            pressure_cutoff,
            sleep_tolerance,
        } = config;
        if !(8..=1920).contains(&w)
            || !(8..=1080).contains(&h)
            || !(1..=32).contains(&workers)
            || solid.len() != w * h
            || !density.iter().all(|v| v.is_finite() && *v > 0.)
            || !drag.is_finite()
            || drag < 0.
            || !viscosity.is_finite()
            || viscosity < 0.
            || !pressure_cutoff.is_finite()
            || !(0. ..=0.1).contains(&pressure_cutoff)
            || !sleep_tolerance.is_finite()
            || !(0. ..=0.01).contains(&sleep_tolerance)
            || (0..w).any(|x| !solid[x] || !solid[(h - 1) * w + x])
            || (0..h).any(|y| !solid[y * w] || !solid[y * w + w - 1])
        {
            return Err(Error::InvalidInput);
        }
        let state = State {
            fraction: vec![[0.; 2]; w * h],
            u: vec![0.; (w + 1) * h],
            v: vec![0.; w * (h + 1)],
            pressure: vec![0.; w * h],
        };
        Ok(Self {
            #[cfg(feature = "gpu")]
            gpu: None,
            config,
            solid,
            backup: state.clone(),
            state,
            pool: rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .map_err(|_| Error::InvalidInput)?,
            idle_ticks: 0,
            sleeping: false,
            stats: Stats::default(),
            acceleration: None,
        })
    }
    /// Use compute for momentum and pressure. Conservative VOF stays on CPU.
    /// Device resources are shared across worlds, not recreated each tick.
    #[cfg(feature = "gpu")]
    pub fn use_gpu(&mut self, context: std::sync::Arc<gpu::GpuContext>) {
        self.gpu = Some(context);
        self.sleeping = false;
        self.idle_ticks = 0;
    }
    pub fn backend_label(&self) -> &'static str {
        #[cfg(feature = "gpu")]
        if let Some(gpu) = &self.gpu {
            return if gpu.adapter_info().device_type == wgpu::DeviceType::Cpu {
                "SWGPU"
            } else {
                "GPU"
            };
        }
        "CPU"
    }
    fn advance_velocity(&mut self, dt: f64, _stats: &mut Stats) -> Result<u32, Error> {
        #[cfg(feature = "gpu")]
        if let Some(gpu) = &self.gpu {
            let result =
                gpu.advect(self.config, &self.state.u, &self.state.v, self.bounds(), dt)?;
            _stats
                .gpu
                .as_mut()
                .expect("GPU step metrics")
                .add_work(false, result.work);
            self.state.u = result.u;
            self.state.v = result.v;
            return Ok(0);
        }
        Ok(self.advect_velocity(dt))
    }
    pub fn fraction(&self, x: usize, y: usize) -> Option<[f64; 2]> {
        (x < self.config.width && y < self.config.height)
            .then(|| self.state.fraction[y * self.config.width + x])
    }
    pub fn volumes(&self) -> [f64; 2] {
        self.state.fraction.iter().fold([0., 0.], |mut sum, v| {
            sum[0] += v[0];
            sum[1] += v[1];
            sum
        })
    }
    pub fn stats(&self) -> Stats {
        self.stats
    }
    pub fn column_volumes(&self, rect: [usize; 4]) -> Option<Vec<[f64; 2]>> {
        let [x, y, w, h] = rect;
        if x.checked_add(w)? > self.config.width || y.checked_add(h)? > self.config.height {
            return None;
        }
        let mut result = vec![[0.; 2]; w];
        for py in y..y + h {
            for (px, v) in result.iter_mut().enumerate() {
                let f = self.state.fraction[py * self.config.width + x + px];
                v[0] += f[0];
                v[1] += f[1];
            }
        }
        Some(result)
    }
    /// Add physical volume, never overwrite another phase or a solid.
    pub fn add(&mut self, x: usize, y: usize, phase: usize, volume: f64) -> Result<(), Error> {
        if x >= self.config.width
            || y >= self.config.height
            || phase >= 2
            || !volume.is_finite()
            || volume < 0.
        {
            return Err(Error::InvalidInput);
        }
        let i = y * self.config.width + x;
        if self.solid[i] || self.state.fraction[i].iter().sum::<f64>() + volume > 1. + 1e-12 {
            return Err(Error::Occupied);
        }
        self.state.fraction[i][phase] += volume;
        self.stats.volume[phase] += volume;
        self.wake();
        Ok(())
    }
    /// Arbitrary fixed geometry, including concave walls, openings and islands.
    /// Inserting a wall into liquid is explicitly rejected, not destructive.
    pub fn set_solid(&mut self, x: usize, y: usize, solid: bool) -> Result<(), Error> {
        let w = self.config.width;
        let h = self.config.height;
        if x == 0 || y == 0 || x >= w - 1 || y >= h - 1 {
            return Err(Error::InvalidInput);
        }
        let i = y * w + x;
        if solid && self.state.fraction[i].iter().sum::<f64>() > 1e-12 {
            return Err(Error::Occupied);
        }
        self.solid[i] = solid;
        self.wake();
        Ok(())
    }
    fn wake(&mut self) {
        self.sleeping = false;
        self.idle_ticks = 0;
    }
    pub fn signature(&self) -> u64 {
        let hash = self
            .state
            .fraction
            .iter()
            .flatten()
            .chain(&self.state.u)
            .chain(&self.state.v)
            .chain(&self.state.pressure)
            .fold(0xcbf29ce484222325, |h, v| {
                (h ^ v.to_bits()).wrapping_mul(0x100000001b3)
            });
        let hash = self
            .config
            .density
            .into_iter()
            .chain([
                self.config.drag,
                self.config.viscosity,
                self.config.pressure_cutoff,
                self.config.sleep_tolerance,
            ])
            .fold(hash, |h, v| (h ^ v.to_bits()).wrapping_mul(0x100000001b3));
        let hash = self
            .solid
            .iter()
            .fold(hash, |h, &s| (h ^ u64::from(s)).wrapping_mul(0x100000001b3));
        [self.idle_ticks as u64, u64::from(self.sleeping)]
            .into_iter()
            .chain(self.acceleration.unwrap_or([0., 0.]).map(f64::to_bits))
            .fold(hash, |h, v| (h ^ v).wrapping_mul(0x100000001b3))
    }
    /// Advance with gravity plus any acceleration of the reference frame.
    /// On error the complete previous physical state is restored.
    pub fn step(&mut self, dt: f64, acceleration: [f64; 2]) -> Result<Stats, Error> {
        if !dt.is_finite() || dt <= 0. || dt > 0.1 || !acceleration.iter().all(|v| v.is_finite()) {
            return Err(Error::InvalidInput);
        }
        let old_idle = self.idle_ticks;
        let old_sleep = self.sleeping;
        if self.acceleration != Some(acceleration) {
            self.wake();
        }
        if self.sleeping {
            self.stats.substeps = 0;
            self.stats.active_tiles = 0;
            self.stats.pressure_iterations = 0;
            self.stats.workers_used = 0;
            self.stats.max_speed = 0.;
            self.stats.rms_speed = 0.;
            self.stats.max_mass_speed = 0.;
            self.stats.max_fraction_change = 0.;
            self.stats.volume_change = 0.;
            self.stats.pressure_ms = 0.;
            self.stats.pressure_assembly_ms = 0.;
            self.stats.transport_ms = 0.;
            self.stats.velocity_ms = 0.;
            if let Some(gpu) = &mut self.stats.gpu {
                gpu.velocity_ms = gpu.velocity_ms.map(|_| 0.);
                gpu.pressure_ms = gpu.pressure_ms.map(|_| 0.);
                gpu.upload_bytes = 0;
                gpu.readback_bytes = 0;
                gpu.dispatches = 0;
                gpu.readbacks = 0;
                gpu.pressure_setup_ms = 0.;
                gpu.velocity_workgroups = 0;
                gpu.pressure_workgroups = 0;
            }
            return Ok(self.stats);
        }
        self.backup.clone_from(&self.state);
        match self.advance(dt, acceleration) {
            Ok(stats) => {
                self.stats = stats;
                self.acceleration = Some(acceleration);
                Ok(stats)
            }
            Err(e) => {
                std::mem::swap(&mut self.state, &mut self.backup);
                self.idle_ticks = old_idle;
                self.sleeping = old_sleep;
                Err(e)
            }
        }
    }
    fn advance(&mut self, dt: f64, acceleration: [f64; 2]) -> Result<Stats, Error> {
        let mut remaining = dt;
        let mut stats = Stats::default();
        #[cfg(feature = "gpu")]
        {
            stats.gpu = self.gpu.as_ref().map(|gpu| GpuStats::new(gpu));
        }
        while remaining > 1e-10 {
            if stats.substeps >= 64 {
                return Err(Error::TimeStep);
            }
            let speed = self.physical_motion().0;
            let force = acceleration[0].abs().max(acceleration[1].abs());
            // Bound total outgoing swept volume, including acceleration.
            let mut sub = remaining
                .min(0.45 / (speed + force * remaining + 1e-6))
                .min(0.24 / self.config.viscosity.max(1e-20));
            let before = self.state.clone();
            let mut attempts = 0;
            let projection = loop {
                if attempts > 0 {
                    self.state.clone_from(&before);
                }
                let clock = std::time::Instant::now();
                let workers = self.advance_velocity(sub, &mut stats)?;
                stats.workers_used = stats.workers_used.max(workers);
                self.forces(sub, acceleration);
                stats.velocity_ms += clock.elapsed().as_secs_f64() * 1000.;
                let clock = std::time::Instant::now();
                let projection = self.project(sub, acceleration, &mut stats)?;
                stats.pressure_ms += clock.elapsed().as_secs_f64() * 1000.;
                self.extrapolate_velocity();
                let clock = std::time::Instant::now();
                let transport = self.transport(sub);
                stats.transport_ms += clock.elapsed().as_secs_f64() * 1000.;
                match transport {
                    Ok(()) => break projection,
                    Err(Error::VolumeBounds) if attempts < 12 => {
                        attempts += 1;
                        stats.flux_retries += 1;
                        sub *= 0.5;
                    }
                    Err(e) => return Err(e),
                }
            };
            stats.pressure_iterations += projection.0;
            stats.max_divergence = stats.max_divergence.max(projection.1);
            stats.substeps += 1;
            remaining -= sub;
        }
        stats.volume = self.volumes();
        stats.wet_cells = self
            .state
            .fraction
            .iter()
            .filter(|v| v[0] + v[1] > WET)
            .count();
        let (speed, energy, maximum, tiles) = self.physical_motion();
        stats.max_speed = speed;
        stats.active_tiles = tiles;
        stats.rms_speed = (energy / (stats.volume[0] + stats.volume[1]).max(WET)).sqrt();
        stats.max_mass_speed = maximum.sqrt();
        stats.volume_change = self
            .state
            .fraction
            .iter()
            .zip(&self.backup.fraction)
            .map(|(a, b)| (a[0] - b[0]).abs() + (a[1] - b[1]).abs())
            .sum();
        stats.max_fraction_change = self
            .state
            .fraction
            .iter()
            .zip(&self.backup.fraction)
            .map(|(a, b)| (a[0] - b[0]).abs().max((a[1] - b[1]).abs()))
            .fold(0., f64::max);
        // Sleep depends on local liquid energy and local volume change, never
        // gas extension velocity or a world average that hides a moving drop.
        let gravity = (acceleration[0].powi(2) + acceleration[1].powi(2)).sqrt();
        let head = stats.max_mass_speed.powi(2) / (2. * gravity).max(1.);
        if self.config.sleep_tolerance > 0.
            && head < self.config.sleep_tolerance
            && stats.max_fraction_change < self.config.sleep_tolerance
        {
            self.idle_ticks += 1;
        } else {
            self.idle_ticks = 0;
        }
        if self.idle_ticks >= 60 {
            self.sleeping = true;
            self.state.u.fill(0.);
            self.state.v.fill(0.);
        }
        stats.sleeping = self.sleeping;
        Ok(stats)
    }
    fn physical_motion(&self) -> (f64, f64, f64, usize) {
        let w = self.config.width;
        let mut tiles = vec![false; w.div_ceil(32) * self.config.height.div_ceil(32)];
        let mut speed: f64 = 0.;
        let mut energy = 0.;
        let mut maximum: f64 = 0.;
        for (i, f) in self.state.fraction.iter().enumerate() {
            let volume = f[0] + f[1];
            if volume <= WET {
                continue;
            }
            let x = i % w;
            let y = i / w;
            tiles[y / 32 * w.div_ceil(32) + x / 32] = true;
            let u = [
                self.state.u[y * (w + 1) + x],
                self.state.u[y * (w + 1) + x + 1],
            ];
            let v = [self.state.v[i], self.state.v[i + w]];
            let (e, s) = if volume >= 1. - 1e-12 {
                (
                    (u[0] * u[0]
                        + u[0] * u[1]
                        + u[1] * u[1]
                        + v[0] * v[0]
                        + v[0] * v[1]
                        + v[1] * v[1])
                        / 3.,
                    u.into_iter().chain(v).map(f64::abs).fold(0., f64::max),
                )
            } else {
                geometry::Polygon::square()
                    .partition(volume, self.normal(i, None))
                    .0
                    .motion(u, v)
            };
            energy += e;
            maximum = maximum.max(e);
            speed = speed.max(s);
        }
        (speed, energy, maximum, tiles.iter().filter(|&&v| v).count())
    }
    fn bounds(&self) -> Option<[usize; 4]> {
        let w = self.config.width;
        let h = self.config.height;
        let mut b = [w, h, 0, 0];
        for (i, f) in self.state.fraction.iter().enumerate() {
            if f[0] + f[1] > WET {
                let x = i % w;
                let y = i / w;
                b[0] = b[0].min(x);
                b[1] = b[1].min(y);
                b[2] = b[2].max(x + 1);
                b[3] = b[3].max(y + 1);
            }
        }
        (b[0] < b[2]).then_some(b)
    }
    /// Raster values are a view of volume fractions, never the physical state.
    /// Only the caller chooses material IDs and the subcell visibility cutoff.
    pub fn rasterize(&self, threshold: f64, offset_x: i32, values: [u8; 3]) -> Vec<u8> {
        let w = self.config.width;
        let h = self.config.height;
        let mut view = vec![values[0]; w * h];
        for (i, f) in self.state.fraction.iter().enumerate() {
            if f[0] + f[1] < threshold {
                continue;
            }
            let x = (i % w) as i32 + offset_x;
            if (0..w as i32).contains(&x) {
                view[i / w * w + x as usize] = values[1 + usize::from(f[1] > f[0])];
            }
        }
        view
    }
    fn normal(&self, index: usize, phase: Option<usize>) -> [f64; 2] {
        let w = self.config.width;
        let value = |i: usize| {
            let v = self.state.fraction[i];
            if let Some(p) = phase {
                v[p]
            } else {
                v[0] + v[1]
            }
        };
        // Solid neighbors extend the local fraction: a wall is not a free surface.
        let at = |i: usize| {
            if self.solid[i] {
                value(index)
            } else {
                value(i)
            }
        };
        let n = [at(index - 1) - at(index + 1), at(index - w) - at(index + w)];
        let length = n[0].abs() + n[1].abs();
        if length < 1e-10 {
            [0., -1.]
        } else {
            [n[0] / length, n[1] / length]
        }
    }
}
