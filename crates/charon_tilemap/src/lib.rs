//! Editable tile and sprite data. No physics, rendering, window, or Core dependency.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileId(pub u16);
#[derive(Clone, Debug, PartialEq)]
pub struct TileDefinition {
    pub name: String,
    pub solid: bool,
    pub atlas_index: u32,
    pub tint: [f32; 4],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub revision: u64,
    pub x: u32,
    pub y: u32,
    pub before: Option<TileId>,
    pub after: Option<TileId>,
}
#[derive(Clone, Debug)]
pub struct Tilemap {
    width: u32,
    height: u32,
    tile_size: [u32; 2],
    cells: Vec<Option<TileId>>,
    definitions: Vec<TileDefinition>,
    revision: u64,
    changes: Vec<Change>,
}
impl Tilemap {
    pub fn new(width: u32, height: u32, tile_size: [u32; 2]) -> Result<Self, String> {
        let len = (width as usize)
            .checked_mul(height as usize)
            .ok_or("map dimensions overflow")?;
        if width == 0 || height == 0 || tile_size.contains(&0) || len > 16_777_216 {
            return Err("invalid map dimensions (maximum 16M tiles)".into());
        }
        Ok(Self {
            width,
            height,
            tile_size,
            cells: vec![None; len],
            definitions: Vec::new(),
            revision: 0,
            changes: Vec::new(),
        })
    }
    pub fn register(&mut self, definition: TileDefinition) -> Result<TileId, String> {
        if definition.name.trim().is_empty()
            || self.definitions.iter().any(|d| d.name == definition.name)
            || self.definitions.len() > u16::MAX as usize
            || definition
                .tint
                .iter()
                .any(|c| !c.is_finite() || !(0.0..=1.0).contains(c))
        {
            return Err("invalid or duplicate tile definition".into());
        }
        let id = TileId(self.definitions.len() as u16);
        self.definitions.push(definition);
        Ok(id)
    }
    pub fn definition(&self, id: TileId) -> Option<&TileDefinition> {
        self.definitions.get(id.0 as usize)
    }
    pub fn dimensions(&self) -> [u32; 2] {
        [self.width, self.height]
    }
    pub fn tile_size(&self) -> [u32; 2] {
        self.tile_size
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    fn index(&self, x: u32, y: u32) -> Option<usize> {
        (x < self.width && y < self.height).then_some(y as usize * self.width as usize + x as usize)
    }
    pub fn get(&self, x: u32, y: u32) -> Option<TileId> {
        self.index(x, y).and_then(|i| self.cells[i])
    }
    pub fn set(&mut self, x: u32, y: u32, tile: Option<TileId>) -> Result<bool, String> {
        let i = self.index(x, y).ok_or("tile out of bounds")?;
        if tile.is_some_and(|id| self.definition(id).is_none()) {
            return Err("unknown tile ID".into());
        }
        let before = self.cells[i];
        if before == tile {
            return Ok(false);
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or("tile revision exhausted")?;
        self.cells[i] = tile;
        self.changes.push(Change {
            revision: self.revision,
            x,
            y,
            before,
            after: tile,
        });
        Ok(true)
    }
    /// Consumers must drain changes at their chosen update boundary.
    pub fn drain_changes(&mut self) -> impl Iterator<Item = Change> + '_ {
        self.changes.drain(..)
    }
    pub fn occupied(&self) -> impl Iterator<Item = (u32, u32, TileId)> + '_ {
        self.cells
            .iter()
            .enumerate()
            .filter_map(|(i, id)| id.map(|id| (i as u32 % self.width, i as u32 / self.width, id)))
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Atlas {
    pub texture: u64,
    pub size: [u32; 2],
    pub tile_size: [u32; 2],
}
impl Atlas {
    pub fn uv(&self, index: u32) -> Option<[f32; 4]> {
        if self.tile_size.contains(&0) || self.size.contains(&0) {
            return None;
        }
        let columns = self.size[0] / self.tile_size[0];
        let rows = self.size[1] / self.tile_size[1];
        if columns == 0 || rows == 0 || u64::from(index) >= u64::from(columns) * u64::from(rows) {
            return None;
        }
        Some([
            (index % columns * self.tile_size[0]) as f32 / self.size[0] as f32,
            (index / columns * self.tile_size[1]) as f32 / self.size[1] as f32,
            self.tile_size[0] as f32 / self.size[0] as f32,
            self.tile_size[1] as f32 / self.size[1] as f32,
        ])
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    pub position: [f32; 2],
    pub size: [f32; 2],
    pub atlas_index: u32,
    pub layer: i32,
    pub tint: [f32; 4],
}
#[cfg(feature = "plugin")]
pub struct TilemapPlugin {
    initial: Option<Tilemap>,
    owns: bool,
}
#[cfg(feature = "plugin")]
impl TilemapPlugin {
    pub fn new(map: Tilemap) -> Self {
        Self {
            initial: Some(map),
            owns: false,
        }
    }
}
#[cfg(feature = "plugin")]
impl charon_core::Plugin for TilemapPlugin {
    fn name(&self) -> &'static str {
        "tilemap"
    }
    fn init(&mut self, world: &mut charon_core::World) -> charon_core::PluginResult {
        if world.get::<Tilemap>().is_some() {
            return Err("Tilemap already installed".into());
        }
        world.insert(self.initial.take().ok_or("tilemap already initialized")?);
        self.owns = true;
        Ok(())
    }
    fn fixed_update(
        &mut self,
        world: &mut charon_core::World,
        _: charon_core::Step,
    ) -> charon_core::PluginResult {
        world.clear_events::<Change>();
        let changes: Vec<_> = world
            .get_mut::<Tilemap>()
            .ok_or("Tilemap missing")?
            .drain_changes()
            .collect();
        for change in changes {
            world.send(change)
        }
        Ok(())
    }
    fn shutdown(&mut self, world: &mut charon_core::World) -> charon_core::PluginResult {
        if self.owns {
            world.remove::<Tilemap>();
            self.owns = false;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editing_is_validated_and_notifies_only_changes() {
        let mut map = Tilemap::new(2, 2, [8, 8]).unwrap();
        let id = map
            .register(TileDefinition {
                name: "soil".into(),
                solid: true,
                atlas_index: 0,
                tint: [1.; 4],
            })
            .unwrap();
        assert_eq!(map.set(0, 0, Some(id)), Ok(true));
        assert_eq!(map.set(0, 0, Some(id)), Ok(false));
        assert!(map.set(2, 0, Some(id)).is_err());
        assert!(map.set(1, 1, Some(TileId(9))).is_err());
        assert_eq!(map.drain_changes().count(), 1);
        assert_eq!(map.revision(), 1);
    }
    #[test]
    fn atlas_dimensions_are_configurable() {
        let atlas = Atlas {
            texture: 1,
            size: [32, 16],
            tile_size: [8, 8],
        };
        assert_eq!(atlas.uv(5), Some([0.25, 0.5, 0.25, 0.5]));
        assert!(atlas.uv(8).is_none());
    }
}
