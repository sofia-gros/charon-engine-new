//! Tile optics are game data, not fields baked into TileDefinition or Rendering.
use charon_lighting::{Geometry, Optical, OpticalGrid};
use charon_tilemap::{Change, TileId, Tilemap};
use std::collections::HashMap;
pub fn rasterize(
    map: &Tilemap,
    geometry: Geometry,
    origin: [f32; 2],
    properties: &HashMap<TileId, Optical>,
) -> Result<OpticalGrid, String> {
    if origin.iter().any(|v| !v.is_finite()) {
        return Err("invalid tile optical origin".into());
    }
    let mut grid = OpticalGrid::new(geometry)?;
    let [w, h] = map.tile_size();
    for (x, y, id) in map.occupied() {
        let def = map.definition(id).ok_or("unknown optical tile")?;
        let optical = properties.get(&id).copied().unwrap_or(if def.solid {
            Optical::OPAQUE
        } else {
            Optical::AIR
        });
        grid.raster_rect(
            [
                origin[0] + x as f32 * w as f32,
                origin[1] + y as f32 * h as f32,
                w as f32,
                h as f32,
            ],
            optical,
        )?;
    }
    Ok(grid)
}
/// Revision and explicit edit notifications trigger a rebuild. Full rasterizing
/// is intentional in M4; Scene::set_layer then detects the actual optical diff.
pub struct TilemapInput {
    geometry: Geometry,
    origin: [f32; 2],
    last_revision: Option<u64>,
    cached: Option<OpticalGrid>,
    last_properties: Option<HashMap<TileId, Optical>>,
}
impl TilemapInput {
    pub fn new(geometry: Geometry, origin: [f32; 2]) -> Result<Self, String> {
        geometry.validate()?;
        if origin.iter().any(|v| !v.is_finite()) {
            return Err("invalid origin".into());
        }
        Ok(Self {
            geometry,
            origin,
            last_revision: None,
            cached: None,
            last_properties: None,
        })
    }
    pub fn invalidate(&mut self) {
        self.last_revision = None;
    }
    pub fn sync(
        &mut self,
        map: &Tilemap,
        changes: &[Change],
        properties: &HashMap<TileId, Optical>,
    ) -> Result<&OpticalGrid, String> {
        if self.last_revision != Some(map.revision())
            || !changes.is_empty()
            || self.last_properties.as_ref() != Some(properties)
        {
            self.cached = Some(rasterize(map, self.geometry, self.origin, properties)?);
            self.last_revision = Some(map.revision());
            self.last_properties = Some(properties.clone());
        }
        self.cached
            .as_ref()
            .ok_or("optical cache uninitialized".into())
    }
}
