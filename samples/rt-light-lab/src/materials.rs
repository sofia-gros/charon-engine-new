//! Demo policy only: game material IDs do not leak into the tracing plugin.
use charon_m5_sample::scenes::Session;
use charon_tilemap::TileId;
use charon_tracing::{Material, Mode, SurfaceQuality, TraceScene, charon_lighting::*};
use std::collections::HashMap;
pub fn scene(session: &Session, mode: Mode) -> Result<TraceScene, String> {
    let light = session.light.as_ref().ok_or("lighting missing")?;
    let runtime = session.runtime.as_ref().ok_or("runtime missing")?;
    let view = runtime.shared.view.lock().unwrap().clone();
    let player = *runtime.shared.player.lock().unwrap();
    from_view(
        &view,
        player.position,
        light.scene().settings(),
        light.torch,
        light.ore,
        mode,
    )
}
/// Shared by the interactive demo and the full-size headless regression check.
pub fn from_view(
    view: &charon_m3_sample::runtime::View,
    player: [f32; 2],
    mut settings: Settings,
    torch: bool,
    ore: bool,
    mode: Mode,
) -> Result<TraceScene, String> {
    // Build from original tile and particle storage. Resampling the legacy
    // 2/4-unit grid here would permanently preserve oversized particle shadows.
    let geometry = Geometry {
        width: 320,
        height: 192,
        origin: [0.; 2],
        cell_size: 1.,
    };
    settings.ao_strength = 0.;
    let mut input = Scene::new(geometry, settings)?;
    let properties = HashMap::from([
        (
            TileId(2),
            Optical {
                opacity: 0.,
                transmission: [1.; 3],
                ..Optical::AIR
            },
        ),
        (
            TileId(3),
            Optical {
                emission_color: [0.25, 0.8, 1.],
                emission_strength: if ore { 6. } else { 0. },
                ..Optical::OPAQUE
            },
        ),
    ]);
    let tiles = charon_tilemap_lighting::rasterize(&view.map, geometry, [0.; 2], &properties)?;
    // Water has a renderer material even when its extinction is represented by
    // absorption rather than legacy per-cell opacity.
    let mut mats = vec![Material::AIR; geometry.len()];
    let [tw, th] = view.map.tile_size();
    for (x, y, id) in view.map.occupied() {
        let mut material = if id == TileId(2) {
            Material::WATER
        } else {
            Material::STONE
        };
        if let Some(def) = view.map.definition(id) {
            material.diffuse = std::array::from_fn(|k| def.tint[k].clamp(0., 1.) * 0.65);
        }
        if id == TileId(2) {
            material = Material::WATER;
        }
        if id == TileId(3) && ore {
            material.emission = [1.5, 4.8, 6.];
        }
        for py in y * th..((y + 1) * th).min(geometry.height) {
            for px in x * tw..((x + 1) * tw).min(geometry.width) {
                mats[(py * geometry.width + px) as usize] = material;
            }
        }
    }
    input.set_layer(1, tiles)?;
    #[cfg(feature = "bridge")]
    if let Some(snapshot) = &view.sand {
        use charon_physics::sim::{material::Cell, rules::Movement};
        let mut particles = OpticalGrid::new(geometry)?;
        for (i, cell) in snapshot.cells.iter().enumerate() {
            if cell.kind == Cell::Air {
                continue;
            }
            let x = i % snapshot.width;
            let y = i / snapshot.width;
            if x >= 320 || y >= 192 {
                continue;
            }
            let j = y * 320 + x;
            let mut m;
            let optical;
            if cell.kind == Cell::Fire {
                let mut neighbors = 0_u32;
                for dy in -1_i32..=1 {
                    for dx in -1_i32..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = x as i32 + dx;
                        let ny = y as i32 + dy;
                        if nx >= 0
                            && ny >= 0
                            && (nx as usize) < snapshot.width
                            && (ny as usize) < snapshot.height
                            && snapshot.cells[ny as usize * snapshot.width + nx as usize].kind
                                == Cell::Fire
                        {
                            neighbors += 1;
                        }
                    }
                }
                let heat = (0.3 + cell.extra as f32 / 255. * 0.3 + neighbors as f32 / 8. * 0.4)
                    .clamp(0., 1.);
                let core = (heat * heat).clamp(0., 1.);
                let color = [1., 0.08 + 0.72 * core, 0.015 + 0.32 * core];
                let strength = 4. + 10. * heat;
                m = Material {
                    emission: color.map(|v| v * strength),
                    dynamic: true,
                    ..Material::AIR
                };
                optical = Optical {
                    emission_color: color,
                    emission_strength: strength,
                    ..Optical::AIR
                };
            } else if cell.kind == Cell::Water {
                m = Material::WATER;
                optical = Optical::AIR;
            } else if cell.kind == Cell::Sand || cell.kind == Cell::Gravel {
                let grain = ((cell.extra as u32 * 197) % 256) as f32 / 255. - 0.5;
                let n = charon_tracing::hdr::normalize([grain * 0.3, grain * 0.17, 1.]);
                m = Material {
                    normal: n,
                    ..Material::SAND
                };
                optical = Optical::OPAQUE;
            } else {
                let def = &snapshot.palette[cell.kind.id() as usize];
                match def.movement {
                    Movement::Gas | Movement::Empty => {
                        m = Material {
                            dynamic: true,
                            ..Material::AIR
                        };
                        optical = Optical {
                            opacity: 0.005,
                            ..Optical::AIR
                        };
                    }
                    Movement::Liquid => {
                        m = Material {
                            absorption: [0.08, 0.04, 0.02],
                            ..Material::WATER
                        };
                        optical = Optical::AIR;
                    }
                    _ => {
                        m = Material {
                            diffuse: std::array::from_fn(|k| {
                                (def.color[k] as f32 / 255.).powf(2.2)
                            }),
                            dynamic: true,
                            ..Material::STONE
                        };
                        optical = Optical::OPAQUE;
                    }
                }
            }
            // Adjacent falling particles remain separate full-resolution cells.
            m.dynamic = true;
            mats[j] = m;
            particles.set(x as u32, y as u32, optical)?;
        }
        input.set_layer(2, particles)?;
    }
    if torch {
        input.set_light(
            1,
            PointLight {
                position: [player[0] + 3., player[1] + 3.],
                color: [1., 0.6, 0.22],
                intensity: 14.,
            },
        )?;
    }
    let mut scene = TraceScene::new(input.prepare())?;
    for y in 0..192 {
        for x in 0..320 {
            let i = y * 320 + x;
            if mats[i].dielectric {
                let surface = y == 0 || !mats[i - 320].dielectric;
                mats[i].normal = if surface {
                    [0., -0.98, 0.19899748]
                } else {
                    [0., 0., 1.]
                };
                if !surface {
                    mats[i].specular = 0.;
                }
            }
        }
    }
    scene.materials = mats;
    scene.surface_quality = SurfaceQuality::for_mode(mode);
    scene.validate()?;
    Ok(scene)
}
