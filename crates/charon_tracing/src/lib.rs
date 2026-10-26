//! 2D diffuse path tracing for optical grids. No renderer, physical world, or GPU.
//! Ray distance, reflectance and stochastic budgets are explicit. This is a 2D
//! lighting estimator, not a 3D PBR material/geometry renderer.
pub use charon_lighting;
use charon_lighting::{LightField, Prepared, ambient_occlusion};
mod reference;
pub use reference::{solve_sample, visibility};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Pc,
    Mobile,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Rtx,
    Path,
    LitePath,
    Legacy,
}
impl Mode {
    pub fn validate(self, platform: Platform) -> Result<(), String> {
        let mobile =
            platform == Platform::Mobile || cfg!(any(target_os = "android", target_os = "ios"));
        if mobile && !matches!(self, Self::LitePath | Self::Legacy) {
            return Err("Mobile supports LitePath and Legacy only".into());
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quality {
    pub samples_per_update: u32,
    pub bounces: u32,
    pub max_steps: u32,
    pub max_samples: u32,
}
impl Quality {
    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::LitePath => Self {
                samples_per_update: 1,
                bounces: 1,
                max_steps: 64,
                max_samples: 32,
            },
            _ => Self {
                samples_per_update: 4,
                bounces: 2,
                max_steps: 192,
                max_samples: 256,
            },
        }
    }
    pub fn validate(self, mode: Mode, platform: Platform) -> Result<Self, String> {
        mode.validate(platform)?;
        if mode == Mode::Legacy {
            return Err("Legacy is provided by the existing lighting plugin".into());
        }
        if !(1..=16).contains(&self.samples_per_update)
            || self.bounces > 4
            || !(1..=512).contains(&self.max_steps)
            || !(1..=4096).contains(&self.max_samples)
        {
            return Err("invalid tracing budget".into());
        }
        if mode == Mode::LitePath
            && (self.samples_per_update > 2
                || self.bounces > 1
                || self.max_steps > 96
                || self.max_samples > 64)
        {
            return Err("LitePath exceeds its mobile budget".into());
        }
        Ok(self)
    }
}
#[derive(Clone)]
pub struct TraceScene {
    pub input: Prepared,
    pub reflectance: Vec<[f32; 3]>,
    pub point_lights: Vec<charon_lighting::PointLight>,
}
impl TraceScene {
    pub fn new(input: Prepared) -> Result<Self, String> {
        input.validate()?;
        let n = input.geometry.len();
        Ok(Self {
            input,
            reflectance: vec![[0.65; 3]; n],
            point_lights: Vec::new(),
        })
    }
    /// Conservatively resample both tile and particle optics in world units.
    /// Emission sources retain their world position at the new cell resolution.
    pub fn at_resolution(input: &Prepared, cell_size: f32) -> Result<Self, String> {
        input.validate()?;
        if !cell_size.is_finite() || cell_size <= 0. {
            return Err("invalid trace cell size".into());
        }
        if cell_size == input.geometry.cell_size {
            return Self::new(input.clone());
        }
        let old = input.geometry;
        let geometry = charon_lighting::Geometry {
            width: (old.width as f32 * old.cell_size / cell_size).ceil() as u32,
            height: (old.height as f32 * old.cell_size / cell_size).ceil() as u32,
            origin: old.origin,
            cell_size,
        }
        .validate()?;
        let mut grid = charon_lighting::OpticalGrid::new(geometry)?;
        let mut sources = vec![[0_f32; 3]; geometry.len()];
        for (i, optic) in input.optics.iter().enumerate() {
            let x = old.origin[0] + (i as u32 % old.width) as f32 * old.cell_size;
            let y = old.origin[1] + (i as u32 / old.width) as f32 * old.cell_size;
            if *optic != charon_lighting::Optical::AIR {
                grid.raster_rect([x, y, old.cell_size, old.cell_size], *optic)?;
            }
            if let Some(j) = geometry.index([x + old.cell_size * 0.5, y + old.cell_size * 0.5]) {
                for (value, source) in sources[j].iter_mut().zip(input.sources[i]) {
                    *value = (*value).max(source);
                }
            }
        }
        Self::new(Prepared {
            geometry,
            settings: input.settings,
            optics: grid.cells().to_vec(),
            sources,
            revision: input.revision,
        })
    }
    pub fn validate(&self) -> Result<(), String> {
        self.input.validate()?;
        if self.point_lights.len() > 32 {
            return Err("maximum 32 explicit point lights".into());
        }
        for light in &self.point_lights {
            light.validate()?;
            if self.input.geometry.index(light.position).is_none() {
                return Err("point light outside trace domain".into());
            }
        }
        if self.reflectance.len() != self.input.geometry.len()
            || self
                .reflectance
                .iter()
                .flatten()
                .any(|v| !v.is_finite() || !(0.0..=0.95).contains(v))
        {
            return Err("invalid per-cell diffuse reflectance".into());
        }
        Ok(())
    }
    /// Compare optical payload; revision-only notifications do not reset accumulation.
    pub fn same_input(&self, other: &Self) -> bool {
        self.input.geometry == other.input.geometry
            && self.input.settings == other.input.settings
            && self.input.optics == other.input.optics
            && self.input.sources == other.input.sources
            && self.reflectance == other.reflectance
            && self.point_lights == other.point_lights
    }
    /// Bounded deterministic aggregation; point lights retain sub-cell positions.
    pub fn emitters(&self) -> Vec<Emitter> {
        use std::collections::BTreeMap;
        let g = self.input.geometry;
        let mut width = 8_f32;
        let bins = loop {
            let mut bins: BTreeMap<(i32, i32), (f32, Emitter)> = BTreeMap::new();
            for (index, color) in self.input.sources.iter().enumerate() {
                if color.iter().all(|v| *v == 0.) {
                    continue;
                }
                let position = [
                    g.origin[0] + (index as u32 % g.width) as f32 * g.cell_size + g.cell_size * 0.5,
                    g.origin[1] + (index as u32 / g.width) as f32 * g.cell_size + g.cell_size * 0.5,
                ];
                let key = (
                    (position[0] / width).floor() as i32,
                    (position[1] / width).floor() as i32,
                );
                let brightness = color.iter().sum::<f32>();
                let value = bins.entry(key).or_insert((
                    brightness,
                    Emitter {
                        position,
                        radiance: [0.; 3],
                    },
                ));
                if brightness > value.0 {
                    value.0 = brightness;
                    value.1.position = position;
                }
                for (out, c) in value.1.radiance.iter_mut().zip(color) {
                    *out += c * g.cell_size * g.cell_size / 64.;
                }
            }
            if bins.len() <= 64 - self.point_lights.len() {
                break bins;
            }
            width *= 2.;
        };
        let mut emitters: Vec<_> = bins.into_values().map(|v| v.1).collect();
        emitters.extend(self.point_lights.iter().map(|l| Emitter {
            position: l.position,
            radiance: l.color.map(|c| c * l.intensity),
        }));
        emitters
    }
}
/// Exact running mean with a finite sample cap. Any input/configuration change
/// clears history rather than blending old occlusion/light into the new world.
#[derive(Clone, Copy, Debug)]
pub struct Emitter {
    pub position: [f32; 2],
    pub radiance: [f32; 3],
}
#[derive(Default)]
pub struct Accumulator {
    scene: Option<TraceScene>,
    quality: Option<Quality>,
    sum: Vec<[f32; 3]>,
    samples: u32,
}
impl Accumulator {
    pub fn samples(&self) -> u32 {
        self.samples
    }
    pub fn update(&mut self, scene: &TraceScene, quality: Quality) -> Result<LightField, String> {
        scene.validate()?;
        quality.validate(
            if cfg!(any(target_os = "android", target_os = "ios")) {
                Mode::LitePath
            } else {
                Mode::Path
            },
            if cfg!(any(target_os = "android", target_os = "ios")) {
                Platform::Mobile
            } else {
                Platform::Pc
            },
        )?;
        if self.scene.as_ref().is_none_or(|old| !old.same_input(scene))
            || self.quality != Some(quality)
        {
            self.sum = vec![[0.; 3]; scene.input.geometry.len()];
            self.samples = 0;
            self.scene = Some(scene.clone());
            self.quality = Some(quality);
        }
        let count = quality
            .samples_per_update
            .min(quality.max_samples - self.samples);
        let emitters = scene.emitters();
        for sample in self.samples..self.samples + count {
            for (i, sum) in self.sum.iter_mut().enumerate() {
                let value = reference::sample(scene, quality, &emitters, i as u32, sample);
                for c in 0..3 {
                    sum[c] += value[c];
                }
            }
        }
        self.samples += count;
        let ao = ambient_occlusion(
            scene.input.geometry,
            &scene.input.optics,
            scene.input.settings.ao_strength,
        )?;
        let mut indirect: Vec<[f32; 3]> = self
            .sum
            .iter()
            .map(|v| v.map(|c| c / self.samples as f32))
            .collect();
        if quality.bounces > 0 {
            indirect = reference::filter_indirect(scene, &indirect, quality.max_steps);
        }
        let rgba = indirect
            .iter()
            .enumerate()
            .map(|(i, value)| {
                let direct = reference::direct_primary(scene, quality, &emitters, i as u32);
                let rgb: [f32; 3] = std::array::from_fn(|c| {
                    ((direct[c] + value[c] + scene.input.settings.ambient[c]) * ao[i])
                        .max(scene.input.sources[i][c])
                        .clamp(0., 1.)
                });
                [rgb[0], rgb[1], rgb[2], ao[i]]
            })
            .collect();
        Ok(LightField {
            geometry: scene.input.geometry,
            revision: scene.input.revision,
            rgba,
        })
    }
}
