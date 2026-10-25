//! Independent 2D optical fields and a finite-range CPU reference solver.
//! Units and origin belong to the host. No tile, particle, rendering or GPU types.
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    pub origin: [f32; 2],
    pub cell_size: f32,
}
impl Geometry {
    pub fn validate(self) -> Result<Self, String> {
        if self.width == 0
            || self.height == 0
            || self.width > 8192
            || self.height > 8192
            || u64::from(self.width) * u64::from(self.height) > 1_048_576
            || !self.cell_size.is_finite()
            || self.cell_size <= 0.
            || self.origin.iter().any(|v| !v.is_finite())
            || [
                self.origin[0] + self.width as f32 * self.cell_size,
                self.origin[1] + self.height as f32 * self.cell_size,
            ]
            .iter()
            .enumerate()
            .any(|(i, v)| !v.is_finite() || *v <= self.origin[i])
        {
            return Err("invalid lighting geometry (maximum 1M cells)".into());
        }
        Ok(self)
    }
    pub fn len(self) -> usize {
        self.width as usize * self.height as usize
    }
    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
    pub fn index(self, position: [f32; 2]) -> Option<usize> {
        if position.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let x = ((position[0] - self.origin[0]) / self.cell_size).floor();
        let y = ((position[1] - self.origin[1]) / self.cell_size).floor();
        (x >= 0. && y >= 0. && x < self.width as f32 && y < self.height as f32)
            .then(|| y as usize * self.width as usize + x as usize)
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Optical {
    pub opacity: f32,
    pub transmission: [f32; 3],
    pub emission_color: [f32; 3],
    pub emission_strength: f32,
}
impl Default for Optical {
    fn default() -> Self {
        Self::AIR
    }
}
impl Optical {
    pub const AIR: Self = Self {
        opacity: 0.,
        transmission: [1.; 3],
        emission_color: [0.; 3],
        emission_strength: 0.,
    };
    pub const OPAQUE: Self = Self {
        opacity: 1.,
        transmission: [0.; 3],
        emission_color: [0.; 3],
        emission_strength: 0.,
    };
    pub fn validate(self) -> Result<Self, String> {
        if !self.opacity.is_finite()
            || !(0.0..=1.0).contains(&self.opacity)
            || self
                .transmission
                .iter()
                .chain(self.emission_color.iter())
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || !self.emission_strength.is_finite()
            || !(0.0..=16.0).contains(&self.emission_strength)
        {
            return Err("invalid optical properties".into());
        }
        Ok(self)
    }
    pub fn emission(self) -> [f32; 3] {
        self.emission_color.map(|c| c * self.emission_strength)
    }
    /// Layers combine conservatively and independently of order. RGB emission is
    /// a maximum, not an additive energy model; overlapping sources are not doubled.
    pub fn merge(self, other: Self) -> Self {
        let a = self.emission();
        let b = other.emission();
        let e = std::array::from_fn(|i| a[i].max(b[i]));
        let strength = e.into_iter().fold(0_f32, f32::max);
        Self {
            opacity: 1. - (1. - self.opacity) * (1. - other.opacity),
            transmission: std::array::from_fn(|i| self.transmission[i] * other.transmission[i]),
            emission_color: if strength > 0. {
                e.map(|v| v / strength)
            } else {
                [0.; 3]
            },
            emission_strength: strength,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct OpticalGrid {
    geometry: Geometry,
    cells: Vec<Optical>,
}
impl OpticalGrid {
    pub fn new(geometry: Geometry) -> Result<Self, String> {
        let geometry = geometry.validate()?;
        Ok(Self {
            geometry,
            cells: vec![Optical::AIR; geometry.len()],
        })
    }
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }
    pub fn cells(&self) -> &[Optical] {
        &self.cells
    }
    pub fn set(&mut self, x: u32, y: u32, value: Optical) -> Result<(), String> {
        if x >= self.geometry.width || y >= self.geometry.height {
            return Err("optical cell outside grid".into());
        }
        self.cells[y as usize * self.geometry.width as usize + x as usize] = value.validate()?;
        Ok(())
    }
    /// Conservative coverage: any intersected cell receives the input. Use a
    /// finer optical resolution for sub-cell gaps; do not average opaque walls away.
    pub fn raster_rect(&mut self, rect: [f32; 4], value: Optical) -> Result<(), String> {
        let value = value.validate()?;
        if rect.iter().any(|v| !v.is_finite())
            || rect[2] <= 0.
            || rect[3] <= 0.
            || !(rect[0] + rect[2]).is_finite()
            || !(rect[1] + rect[3]).is_finite()
        {
            return Err("invalid optical rectangle".into());
        }
        let g = self.geometry;
        let x0 = ((rect[0] - g.origin[0]) / g.cell_size)
            .floor()
            .clamp(0., g.width as f32) as usize;
        let y0 = ((rect[1] - g.origin[1]) / g.cell_size)
            .floor()
            .clamp(0., g.height as f32) as usize;
        let x1 = ((rect[0] + rect[2] - g.origin[0]) / g.cell_size)
            .ceil()
            .clamp(0., g.width as f32) as usize;
        let y1 = ((rect[1] + rect[3] - g.origin[1]) / g.cell_size)
            .ceil()
            .clamp(0., g.height as f32) as usize;
        // Samples within one layer cover a coarse cell using max opacity / min
        // transmission, rather than repeatedly multiplying partial coverage.
        for y in y0..y1 {
            for x in x0..x1 {
                let c = &mut self.cells[y * g.width as usize + x];
                let emission = c.merge(value);
                c.opacity = c.opacity.max(value.opacity);
                c.transmission =
                    std::array::from_fn(|k| c.transmission[k].min(value.transmission[k]));
                c.emission_color = emission.emission_color;
                c.emission_strength = emission.emission_strength;
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub sky: [f32; 3],
    pub ambient: [f32; 3],
    pub falloff_per_unit: f32,
    pub max_distance: f32,
    pub ao_strength: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            sky: [1., 0.96, 0.88],
            ambient: [0.008; 3],
            falloff_per_unit: 0.978,
            max_distance: 128.,
            ao_strength: 0.55,
        }
    }
}
impl Settings {
    pub fn steps(self, g: Geometry) -> Result<u32, String> {
        g.validate()?;
        if self
            .sky
            .iter()
            .chain(self.ambient.iter())
            .any(|v| !v.is_finite() || !(0.0..=16.0).contains(v))
            || !self.falloff_per_unit.is_finite()
            || !(0.0..1.0).contains(&self.falloff_per_unit)
            || !self.max_distance.is_finite()
            || self.max_distance < 0.
            || !self.ao_strength.is_finite()
            || !(0.0..=1.0).contains(&self.ao_strength)
        {
            return Err("invalid lighting settings".into());
        }
        let steps = (self.max_distance / g.cell_size).ceil();
        if steps > 256. {
            return Err("lighting radius exceeds 256 propagation steps".into());
        }
        Ok(steps as u32)
    }
    pub fn decay(self, g: Geometry) -> f32 {
        self.falloff_per_unit.powf(g.cell_size)
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointLight {
    pub position: [f32; 2],
    pub color: [f32; 3],
    pub intensity: f32,
}
impl PointLight {
    pub fn validate(self) -> Result<Self, String> {
        if self.position.iter().any(|v| !v.is_finite())
            || self
                .color
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || !self.intensity.is_finite()
            || !(0.0..=16.0).contains(&self.intensity)
        {
            return Err("invalid point light".into());
        }
        Ok(self)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub min: [u32; 2],
    pub max: [u32; 2],
}
impl Region {
    fn full(g: Geometry) -> Self {
        Self {
            min: [0, 0],
            max: [g.width, g.height],
        }
    }
    fn union(self, b: Self) -> Self {
        Self {
            min: [self.min[0].min(b.min[0]), self.min[1].min(b.min[1])],
            max: [self.max[0].max(b.max[0]), self.max[1].max(b.max[1])],
        }
    }
}
#[derive(Clone)]
pub struct Scene {
    geometry: Geometry,
    layers: BTreeMap<u64, OpticalGrid>,
    lights: BTreeMap<u64, PointLight>,
    settings: Settings,
    revision: u64,
    dirty: Option<Region>,
}
impl Scene {
    pub fn new(geometry: Geometry, settings: Settings) -> Result<Self, String> {
        geometry.validate()?;
        settings.steps(geometry)?;
        Ok(Self {
            geometry,
            layers: BTreeMap::new(),
            lights: BTreeMap::new(),
            settings,
            revision: 0,
            dirty: Some(Region::full(geometry)),
        })
    }
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }
    pub fn settings(&self) -> Settings {
        self.settings
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    fn changed(&mut self, region: Region) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("lighting revision exhausted");
        self.dirty = Some(self.dirty.map_or(region, |r| r.union(region)));
    }
    pub fn set_layer(&mut self, id: u64, grid: OpticalGrid) -> Result<bool, String> {
        if grid.geometry != self.geometry {
            return Err("optical layers must share origin, resolution and extent".into());
        }
        let mut region = None;
        for (i, cell) in grid.cells.iter().enumerate() {
            let before = self.layers.get(&id).map_or(Optical::AIR, |g| g.cells[i]);
            if *cell != before {
                let x = i as u32 % self.geometry.width;
                let y = i as u32 / self.geometry.width;
                let r = Region {
                    min: [x, y],
                    max: [x + 1, y + 1],
                };
                region = Some(region.map_or(r, |old: Region| old.union(r)));
            }
        }
        self.layers.insert(id, grid);
        if let Some(r) = region {
            self.changed(r);
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub fn remove_layer(&mut self, id: u64) -> bool {
        if self.layers.remove(&id).is_some() {
            self.changed(Region::full(self.geometry));
            true
        } else {
            false
        }
    }
    pub fn set_light(&mut self, id: u64, light: PointLight) -> Result<bool, String> {
        let light = light.validate()?;
        if self.lights.get(&id) == Some(&light) {
            return Ok(false);
        }
        self.lights.insert(id, light);
        self.changed(Region::full(self.geometry));
        Ok(true)
    }
    pub fn remove_light(&mut self, id: u64) -> bool {
        if self.lights.remove(&id).is_some() {
            self.changed(Region::full(self.geometry));
            true
        } else {
            false
        }
    }
    pub fn set_settings(&mut self, settings: Settings) -> Result<bool, String> {
        settings.steps(self.geometry)?;
        if settings == self.settings {
            return Ok(false);
        }
        self.settings = settings;
        self.changed(Region::full(self.geometry));
        Ok(true)
    }
    /// Dependency region includes propagation radius and the AO neighborhood.
    /// The reference backend deliberately recomputes the full domain on change.
    pub fn dirty_region(&self) -> Option<Region> {
        let r = self.dirty?;
        let halo = self.settings.steps(self.geometry).ok()?.saturating_add(1);
        Some(Region {
            min: [r.min[0].saturating_sub(halo), r.min[1].saturating_sub(halo)],
            max: [
                r.max[0].saturating_add(halo).min(self.geometry.width),
                r.max[1].saturating_add(halo).min(self.geometry.height),
            ],
        })
    }
    pub fn acknowledge(&mut self, revision: u64) {
        if revision == self.revision {
            self.dirty = None;
        }
    }
    pub fn prepare(&self) -> Prepared {
        let mut optics = vec![Optical::AIR; self.geometry.len()];
        for layer in self.layers.values() {
            for (a, b) in optics.iter_mut().zip(&layer.cells) {
                *a = a.merge(*b)
            }
        }
        let mut sources: Vec<_> = optics.iter().map(|o| o.emission()).collect();
        for light in self.lights.values() {
            if let Some(i) = self.geometry.index(light.position) {
                for (k, c) in sources[i].iter_mut().enumerate() {
                    *c = c.max(light.color[k] * light.intensity);
                }
            }
        }
        Prepared {
            geometry: self.geometry,
            settings: self.settings,
            optics,
            sources,
            revision: self.revision,
        }
    }
}
#[derive(Clone)]
pub struct Prepared {
    pub geometry: Geometry,
    pub settings: Settings,
    pub optics: Vec<Optical>,
    pub sources: Vec<[f32; 3]>,
    pub revision: u64,
}
impl Prepared {
    pub fn validate(&self) -> Result<(), String> {
        self.settings.steps(self.geometry)?;
        if self.optics.len() != self.geometry.len() || self.sources.len() != self.geometry.len() {
            return Err("lighting input length mismatch".into());
        }
        for c in &self.optics {
            c.validate()?;
        }
        if self
            .sources
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || !(0.0..=16.0).contains(v))
        {
            return Err("invalid source field".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct LightField {
    pub geometry: Geometry,
    pub rgba: Vec<[f32; 4]>,
    pub revision: u64,
}
impl LightField {
    pub fn sample(&self, position: [f32; 2]) -> Option<[f32; 4]> {
        self.geometry
            .index(position)
            .and_then(|i| self.rgba.get(i).copied())
    }
}
/// AO is independent of illumination. Outside-domain samples are open air;
/// include a terrain halo around a cropped viewport to preserve boundary AO.
pub fn ambient_occlusion(
    g: Geometry,
    optics: &[Optical],
    strength: f32,
) -> Result<Vec<f32>, String> {
    g.validate()?;
    if optics.len() != g.len() || !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
        return Err("invalid AO input".into());
    }
    for o in optics {
        o.validate()?;
    }
    let mut ao = vec![1.; g.len()];
    for y in 0..g.height as i32 {
        for x in 0..g.width as i32 {
            let mut opacity = 0.;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && nx < g.width as i32 && ny < g.height as i32 {
                        opacity += optics[ny as usize * g.width as usize + nx as usize].opacity;
                    }
                }
            }
            ao[y as usize * g.width as usize + x as usize] = 1. - strength * opacity / 8.;
        }
    }
    Ok(ao)
}
pub fn solve_cpu(input: &Prepared) -> Result<LightField, String> {
    input.validate()?;
    let g = input.geometry;
    let mut base = input.sources.clone();
    for x in 0..g.width as usize {
        let mut sky = input.settings.sky;
        for y in 0..g.height as usize {
            let i = y * g.width as usize + x;
            for (k, sky_channel) in sky.iter_mut().enumerate() {
                base[i][k] = base[i][k].max(*sky_channel);
                *sky_channel *= (1. - input.optics[i].opacity) * input.optics[i].transmission[k];
            }
        }
    }
    let mut previous = base.clone();
    let mut next = base.clone();
    let decay = input.settings.decay(g);
    for _ in 0..input.settings.steps(g)? {
        for y in 0..g.height as i32 {
            for x in 0..g.width as i32 {
                let i = y as usize * g.width as usize + x as usize;
                let mut light = base[i];
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= g.width as i32 || ny >= g.height as i32 {
                        continue;
                    }
                    let n = ny as usize * g.width as usize + nx as usize;
                    for (k, channel) in light.iter_mut().enumerate() {
                        let transmitted = previous[n][k]
                            * (1. - input.optics[n].opacity)
                            * input.optics[n].transmission[k];
                        *channel = channel.max(transmitted.max(input.sources[n][k]) * decay);
                    }
                }
                next[i] = light;
            }
        }
        std::mem::swap(&mut previous, &mut next);
    }
    let ao = ambient_occlusion(g, &input.optics, input.settings.ao_strength)?;
    let rgba = previous
        .iter()
        .enumerate()
        .map(|(i, light)| {
            let rgb: [f32; 3] = std::array::from_fn(|k| {
                ((light[k] + input.settings.ambient[k]) * ao[i])
                    .max(input.sources[i][k])
                    .clamp(0., 1.)
            });
            [rgb[0], rgb[1], rgb[2], ao[i]]
        })
        .collect();
    Ok(LightField {
        geometry: g,
        rgba,
        revision: input.revision,
    })
}
#[cfg(feature = "plugin")]
pub struct LightingState {
    pub scene: Scene,
    pub field: Option<LightField>,
}
#[cfg(feature = "plugin")]
pub struct LightingPlugin {
    initial: Option<Scene>,
    owns: bool,
}
#[cfg(feature = "plugin")]
impl LightingPlugin {
    pub fn new(scene: Scene) -> Self {
        Self {
            initial: Some(scene),
            owns: false,
        }
    }
}
#[cfg(feature = "plugin")]
impl charon_core::Plugin for LightingPlugin {
    fn name(&self) -> &'static str {
        "lighting"
    }
    fn init(&mut self, w: &mut charon_core::World) -> charon_core::PluginResult {
        if w.get::<LightingState>().is_some() {
            return Err("LightingState already installed".into());
        }
        w.insert(LightingState {
            scene: self.initial.take().ok_or("lighting already initialized")?,
            field: None,
        });
        self.owns = true;
        Ok(())
    }
    fn fixed_update(
        &mut self,
        w: &mut charon_core::World,
        _: charon_core::Step,
    ) -> charon_core::PluginResult {
        let state = w
            .get_mut::<LightingState>()
            .ok_or("LightingState missing")?;
        if state
            .field
            .as_ref()
            .is_none_or(|f| f.revision != state.scene.revision())
        {
            state.field = Some(solve_cpu(&state.scene.prepare())?);
            state.scene.acknowledge(state.scene.revision());
        }
        Ok(())
    }
    fn shutdown(&mut self, w: &mut charon_core::World) -> charon_core::PluginResult {
        if self.owns {
            w.remove::<LightingState>();
            self.owns = false;
        }
        Ok(())
    }
}
