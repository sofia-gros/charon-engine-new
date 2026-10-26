//! Demo-owned world adapters. This closure executes on the lighting worker;
//! neither tracing nor Core knows Tilemap, Falling Sand, input bindings or files.
use charon_m3_sample::runtime::{Player, Shared, View};
use charon_m5_sample::controller::Flags;
use charon_tasks::TaskPool;
use charon_tilemap::TileId;
use charon_tracing::{TraceScene, charon_lighting::*};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
pub fn build(
    view: Arc<View>,
    player: Player,
    flags: Flags,
    geometry: Geometry,
    pool: &TaskPool,
) -> Result<TraceScene, String> {
    let data = view.clone();
    let state = flags.clone();
    let kinds = vec![0];
    #[cfg(feature = "bridge")]
    let kinds = if view.sand.is_some() {
        vec![0, 1]
    } else {
        kinds
    };
    let grids = pool.map(kinds, move |kind| -> Result<OpticalGrid, String> {
        if kind == 0 {
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
                    emission_strength: if state.ore { 1.2 } else { 0. },
                    ..Optical::OPAQUE
                },
            );
            return charon_tilemap_lighting::rasterize(&data.map, geometry, [0., 0.], &properties);
        }
        #[cfg(feature = "bridge")]
        if let Some(snapshot) = &data.sand {
            use charon_physics::sim::{material::Cell, rules::Movement};
            return charon_sand_lighting::rasterize(snapshot, geometry, [0., 0.], 1., |cell| {
                match cell.kind {
                    Cell::Air => Optical::AIR,
                    Cell::Fire => Optical {
                        opacity: 0.02,
                        emission_color: [1., 0.3, 0.05],
                        emission_strength: 1.,
                        ..Optical::AIR
                    },
                    Cell::Water => Optical {
                        opacity: 0.06,
                        transmission: [0.75, 0.88, 1.],
                        ..Optical::AIR
                    },
                    _ => match snapshot.palette[cell.kind.id() as usize].movement {
                        Movement::Gas | Movement::Empty => Optical::AIR,
                        Movement::Liquid => Optical {
                            opacity: 0.15,
                            ..Optical::AIR
                        },
                        _ => Optical::OPAQUE,
                    },
                }
            });
        }
        OpticalGrid::new(geometry)
    })?;
    let settings = Settings {
        sky: if flags.sky { [1., 0.96, 0.88] } else { [0.; 3] },
        ao_strength: if flags.ao { 0.65 } else { 0. },
        ..Settings::default()
    };
    let mut scene = Scene::new(geometry, settings)?;
    for (index, grid) in grids.into_iter().enumerate() {
        scene.set_layer(index as u64, grid?)?;
    }
    let mut input = TraceScene::new(scene.prepare())?;
    if flags.torch {
        input.point_lights.push(PointLight {
            position: [player.position[0] + 3., player.position[1] + 3.],
            color: [1., 0.75, 0.35],
            intensity: 1.4,
        });
    }
    input.validate()?;
    Ok(input)
}
pub fn source(
    shared: Shared,
    flags: Arc<Mutex<Flags>>,
    geometry: Geometry,
    pool: Arc<TaskPool>,
) -> impl FnMut() -> Result<Option<TraceScene>, String> + Send {
    let mut previous: Option<(Arc<View>, Player, Flags)> = None;
    move || {
        let view = shared.view.lock().unwrap().clone();
        let player = *shared.player.lock().unwrap();
        let flags = flags.lock().unwrap().clone();
        let unchanged = previous.as_ref().is_some_and(|(old, position, state)| {
            let same = old.map.revision() == view.map.revision()
                && (!flags.torch || position.position == player.position)
                && *state == flags;
            #[cfg(feature = "bridge")]
            let same = same
                && match (&old.sand, &view.sand) {
                    (Some(a), Some(b)) => a.cells == b.cells,
                    (None, None) => true,
                    _ => false,
                };
            same
        });
        if unchanged {
            return Ok(None);
        }
        let input = build(view.clone(), player, flags.clone(), geometry, &pool)?;
        previous = Some((view, player, flags));
        Ok(Some(input))
    }
}
