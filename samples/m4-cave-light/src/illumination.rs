use charon_assets::ShaderSource;
use charon_lighting::*;
use charon_lighting_gpu::GpuLighting;
use charon_m3_sample::runtime::{Player, View};
use charon_render::{PassId, TextureId, gpu::GpuRenderer};
use charon_tilemap::TileId;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
pub struct Illumination {
    scene: Scene,
    gpu: GpuLighting,
    tiles: charon_tilemap_lighting::TilemapInput,
    last_update: Instant,
    last_revision: Option<u64>,
    pub enabled: bool,
    pub compute_enabled: bool,
    pub defer_inputs: bool,
    pub torch: bool,
    pub ore: bool,
    pub ao: bool,
    pub sky: bool,
    pub cell_size: f32,
    image: TextureId,
    pass: PassId,
}
impl Illumination {
    pub fn new(renderer: &mut GpuRenderer, cell_size: f32) -> Result<Self, String> {
        Self::new_with_ids(renderer, cell_size, TextureId(3), PassId(2))
    }
    pub fn new_with_ids(
        renderer: &mut GpuRenderer,
        cell_size: f32,
        image: TextureId,
        pass: PassId,
    ) -> Result<Self, String> {
        let geometry = Geometry {
            width: (320. / cell_size) as u32,
            height: (192. / cell_size) as u32,
            origin: [0., 0.],
            cell_size,
        };
        let scene = Scene::new(geometry, Settings::default())?;
        let gpu = pollster::block_on(GpuLighting::new(
            renderer.device().clone(),
            renderer.queue().clone(),
            geometry,
        ))?;
        renderer.bind_texture(image, gpu.texture().clone(), true)?;
        pollster::block_on(renderer.register_sampled_pass(
            pass,
            &ShaderSource(charon_lighting_render::SHADER.into()),
            [0., 72., 960., 576.],
            image,
        ))?;
        Ok(Self {
            scene,
            gpu,
            tiles: charon_tilemap_lighting::TilemapInput::new(geometry, [0., 0.])?,
            last_update: Instant::now() - Duration::from_secs(1),
            last_revision: None,
            enabled: true,
            compute_enabled: true,
            defer_inputs: false,
            torch: true,
            ore: true,
            ao: true,
            sky: true,
            cell_size,
            image,
            pass,
        })
    }
    pub fn quality(&mut self, renderer: &mut GpuRenderer) -> Result<(), String> {
        let mut next = Self::new_with_ids(
            renderer,
            if self.cell_size == 4. { 2. } else { 4. },
            self.image,
            self.pass,
        )?;
        next.enabled = self.enabled;
        next.compute_enabled = self.compute_enabled;
        next.defer_inputs = self.defer_inputs;
        next.torch = self.torch;
        next.ore = self.ore;
        next.ao = self.ao;
        next.sky = self.sky;
        *self = next;
        Ok(())
    }
    pub fn update(
        &mut self,
        renderer: &mut GpuRenderer,
        view: &View,
        player: Player,
        size: [u32; 2],
        hz: u32,
    ) -> Result<(), String> {
        renderer.set_parameters(
            self.pass,
            [0., 72., size[0] as f32, size[0] as f32 / 320. * 192.],
        )?;
        if self.defer_inputs {
            return Ok(());
        }
        if !self.enabled {
            return Ok(());
        }
        if self.last_revision.is_some()
            && self.last_update.elapsed() < Duration::from_secs_f64(1. / hz as f64)
        {
            return Ok(());
        }
        self.last_update = Instant::now();
        let mut settings = self.scene.settings();
        settings.ao_strength = if self.ao { 0.65 } else { 0. };
        settings.sky = if self.sky { [1., 0.96, 0.88] } else { [0.; 3] };
        self.scene.set_settings(settings)?;
        let mut properties = HashMap::new();
        properties.insert(
            TileId(2),
            Optical {
                opacity: 0.06,
                transmission: [0.75, 0.88, 1.],
                ..Optical::AIR
            },
        );
        properties.insert(
            TileId(3),
            Optical {
                emission_color: [0.25, 0.8, 1.],
                emission_strength: if self.ore { 1.2 } else { 0. },
                ..Optical::OPAQUE
            },
        );
        let grid = self.tiles.sync(&view.map, &[], &properties)?;
        self.scene.set_layer(1, grid.clone())?;
        #[cfg(feature = "bridge")]
        if let Some(snapshot) = &view.sand {
            use charon_physics::sim::{material::Cell, rules::Movement};
            let grid = charon_sand_lighting::rasterize(
                snapshot,
                self.scene.geometry(),
                [0., 0.],
                1.,
                |cell| {
                    if cell.kind == Cell::Air {
                        return Optical::AIR;
                    }
                    if cell.kind == Cell::Fire {
                        return Optical {
                            opacity: 0.02,
                            emission_color: [1., 0.3, 0.05],
                            emission_strength: 1.,
                            ..Optical::AIR
                        };
                    }
                    if cell.kind == Cell::Water {
                        return Optical {
                            opacity: 0.06,
                            transmission: [0.75, 0.88, 1.],
                            ..Optical::AIR
                        };
                    }
                    match snapshot.palette[cell.kind.id() as usize].movement {
                        Movement::Gas | Movement::Empty => Optical::AIR,
                        Movement::Liquid => Optical {
                            opacity: 0.15,
                            ..Optical::AIR
                        },
                        _ => Optical::OPAQUE,
                    }
                },
            )?;
            self.scene.set_layer(2, grid)?;
        } else {
            self.scene.remove_layer(2);
        }
        if self.torch {
            self.scene.set_light(
                1,
                PointLight {
                    position: [player.position[0] + 3., player.position[1] + 3.],
                    color: [1., 0.75, 0.35],
                    intensity: 1.4,
                },
            )?;
        } else {
            self.scene.remove_light(1);
        }
        if self.last_revision != Some(self.scene.revision()) {
            if self.compute_enabled {
                self.gpu.solve(&self.scene.prepare())?;
            }
            self.last_revision = Some(self.scene.revision());
            self.scene.acknowledge(self.scene.revision());
        }
        Ok(())
    }
    pub fn resume_inputs(&mut self) {
        self.defer_inputs = false;
        self.compute_enabled = true;
        self.last_update = Instant::now() - Duration::from_secs(1);
    }
    /// Restore the existing backend from the current optical snapshot on a mode switch.
    pub fn restore_legacy(&mut self, renderer: &mut GpuRenderer) -> Result<(), String> {
        self.gpu.solve(&self.scene.prepare())?;
        self.compute_enabled = true;
        renderer.bind_texture(self.image, self.gpu.texture().clone(), true)
    }
    /// Low-level optical input for API examples and diagnostics.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }
    /// Explicit synchronous diagnostic readback; do not call every render frame.
    pub fn readback(&mut self) -> Result<LightField, String> {
        self.gpu.readback()
    }
    pub fn verify(&mut self) -> Result<(), String> {
        let actual = self.gpu.readback()?;
        let expected = solve_cpu(&self.scene.prepare())?;
        let error = actual
            .rgba
            .iter()
            .flatten()
            .zip(expected.rgba.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .fold(0_f32, f32::max);
        if error > 0.0002 {
            return Err(format!("cave CPU/GPU mismatch: {error}"));
        }
        println!(
            "M4 live CPU/GPU error {error}, metrics {:?}",
            self.gpu.metrics()
        );
        Ok(())
    }
}
