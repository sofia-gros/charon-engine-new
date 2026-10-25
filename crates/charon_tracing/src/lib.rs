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
}
impl TraceScene {
    pub fn new(input: Prepared) -> Result<Self, String> {
        input.validate()?;
        let n = input.geometry.len();
        Ok(Self {
            input,
            reflectance: vec![[0.65; 3]; n],
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
                for c in 0..3 {
                    sources[j][c] = sources[j][c].max(input.sources[i][c]);
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
    /// Compare payload as well as revision: scene-local revisions are not global IDs.
    pub fn same_input(&self, other: &Self) -> bool {
        self.input.geometry == other.input.geometry
            && self.input.settings == other.input.settings
            && self.input.optics == other.input.optics
            && self.input.sources == other.input.sources
            && self.input.revision == other.input.revision
            && self.reflectance == other.reflectance
    }
    pub fn emitters(&self) -> Vec<u32> {
        self.input
            .sources
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.iter().any(|v| *v > 0.).then_some(i as u32))
            .collect()
    }
}
/// Exact running mean with a finite sample cap. Any input/configuration change
/// clears history rather than blending old occlusion/light into the new world.
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
        let rgba = self
            .sum
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let rgb: [f32; 3] = std::array::from_fn(|c| {
                    ((v[c] / self.samples as f32 + scene.input.settings.ambient[c]) * ao[i])
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
