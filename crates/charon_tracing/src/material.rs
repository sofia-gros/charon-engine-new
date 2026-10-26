//! Renderer-only materials. Distances, depth and absorption use world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    pub diffuse: [f32; 3],
    pub roughness: f32,
    pub specular: f32,
    pub ior: f32,
    pub absorption: [f32; 3],
    /// Half thickness of the extruded cell, independent of grid resolution.
    pub depth: f32,
    pub normal: [f32; 3],
    pub emission: [f32; 3],
    pub dielectric: bool,
    pub dynamic: bool,
}
impl Default for Material {
    fn default() -> Self {
        Self::STONE
    }
}
impl Material {
    pub const STONE: Self = Self {
        diffuse: [0.55, 0.52, 0.48],
        roughness: 0.9,
        specular: 0.04,
        ior: 1.5,
        absorption: [0.; 3],
        depth: 8.,
        normal: [0., 0., 1.],
        emission: [0.; 3],
        dielectric: false,
        dynamic: false,
    };
    pub const AIR: Self = Self {
        diffuse: [0.; 3],
        roughness: 1.,
        specular: 0.,
        ior: 1.,
        absorption: [0.; 3],
        depth: 8.,
        normal: [0., 0., 1.],
        emission: [0.; 3],
        dielectric: false,
        dynamic: false,
    };
    pub const WATER: Self = Self {
        diffuse: [0.04, 0.09, 0.12],
        roughness: 0.06,
        specular: 0.02,
        ior: 1.333,
        absorption: [0.045, 0.016, 0.007],
        depth: 8.,
        normal: [0., 0., 1.],
        emission: [0.; 3],
        dielectric: true,
        dynamic: true,
    };
    pub const SAND: Self = Self {
        diffuse: [0.72, 0.57, 0.32],
        roughness: 0.82,
        specular: 0.04,
        ior: 1.46,
        absorption: [0.; 3],
        depth: 8.,
        normal: [0., 0., 1.],
        emission: [0.; 3],
        dielectric: false,
        dynamic: true,
    };
    pub fn validate(self) -> Result<(), String> {
        let unit = |x: f32| x.is_finite() && (0. ..=1.).contains(&x);
        let length = self.normal.iter().map(|v| v * v).sum::<f32>();
        if !self.diffuse.iter().copied().all(unit)
            || !unit(self.roughness)
            || !unit(self.specular)
            || !self.ior.is_finite()
            || !(1. ..=3.).contains(&self.ior)
            || !self
                .absorption
                .iter()
                .chain(&self.emission)
                .all(|v| v.is_finite() && *v >= 0. && *v <= 1024.)
            || !self.depth.is_finite()
            || !(0.001..=128.).contains(&self.depth)
            || !self.normal.iter().all(|v| v.is_finite())
            || (length - 1.).abs() > 0.01
        {
            return Err("invalid trace material".into());
        }
        Ok(())
    }
    pub fn fresnel(self, cosine: f32) -> f32 {
        let f0 = if self.dielectric {
            ((self.ior - 1.) / (self.ior + 1.)).powi(2)
        } else {
            self.specular
        };
        f0 + (1. - f0) * (1. - cosine.clamp(0., 1.)).powi(5)
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceQuality {
    pub reflection_bounces: u32,
    pub transmission_bounces: u32,
    pub denoise_radius: u32,
    pub dynamic_history: u32,
    /// Relative ceiling for rare indirect samples; zero disables firefly limiting.
    /// Direct lighting, surface transport and emission remain HDR and unclipped.
    pub indirect_clamp: f32,
}
impl SurfaceQuality {
    pub fn for_mode(mode: crate::Mode) -> Self {
        if mode == crate::Mode::Rtx {
            Self {
                reflection_bounces: 4,
                transmission_bounces: 8,
                denoise_radius: 2,
                dynamic_history: 8,
                indirect_clamp: 4.,
            }
        } else {
            Self {
                reflection_bounces: 1,
                transmission_bounces: 4,
                denoise_radius: 1,
                dynamic_history: 4,
                indirect_clamp: 2.,
            }
        }
    }
    pub fn validate(self) -> Result<(), String> {
        if self.reflection_bounces > 8
            || self.transmission_bounces > 16
            || self.denoise_radius > 3
            || !(1..=32).contains(&self.dynamic_history)
            || !self.indirect_clamp.is_finite()
            || !(0. ..=64.).contains(&self.indirect_clamp)
        {
            return Err("invalid surface quality".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct HdrField {
    pub geometry: crate::charon_lighting::Geometry,
    pub revision: u64,
    pub diffuse: Vec<[f32; 3]>,
    pub specular: Vec<[f32; 3]>,
    pub emission: Vec<[f32; 3]>,
}
impl HdrField {
    /// Compatibility view. HDR consumers should use the separate components.
    pub fn light_field(&self) -> crate::charon_lighting::LightField {
        crate::charon_lighting::LightField {
            geometry: self.geometry,
            revision: self.revision,
            rgba: self
                .diffuse
                .iter()
                .zip(&self.specular)
                .zip(&self.emission)
                .map(|((d, s), e)| {
                    let c: [f32; 3] = std::array::from_fn(|k| (d[k] + s[k] + e[k]).clamp(0., 1.));
                    [c[0], c[1], c[2], 1.]
                })
                .collect(),
        }
    }
}
