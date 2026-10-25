//! Registry-local material definitions. IDs are u8, while per-cell state stays
//! two bytes. The standard materials are presets; custom ones need no enum edit.
use super::{
    material::{Cell, CellData},
    physics::ParticleContext,
};
pub const REPOSE_SPAN: usize = 16;
pub type MaterialUpdate = fn(&mut ParticleContext<'_>, &mut dyn rand::Rng);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Movement {
    Empty,
    Solid,
    Granular,
    Liquid,
    Gas,
}

#[derive(Clone, Copy, Debug)]
pub struct MaterialProperties {
    pub density: f32,
    pub lateral_spread: u8,
    pub movement: Movement,
    angle: f32,
    pub(crate) allowed_drop: usize,
    pub(crate) custom_update: Option<MaterialUpdate>,
}
impl MaterialProperties {
    pub fn repose_degrees(self) -> f32 {
        self.angle
    }
    pub fn static_friction(self) -> f32 {
        self.angle.to_radians().tan()
    }
}
#[derive(Clone, Debug)]
pub struct MaterialDefinition {
    name: String,
    color: [u8; 4],
    properties: MaterialProperties,
}
impl MaterialDefinition {
    pub fn new(name: impl Into<String>, movement: Movement, density: f32, color: [u8; 4]) -> Self {
        Self {
            name: name.into(),
            color,
            properties: MaterialProperties {
                density,
                lateral_spread: 4,
                movement,
                angle: 32.0,
                allowed_drop: 10,
                custom_update: None,
            },
        }
    }
    /// The callback replaces the stock movement/reaction update for this material.
    pub fn with_update(mut self, update: MaterialUpdate) -> Self {
        self.properties.custom_update = Some(update);
        self
    }
    pub fn with_repose(mut self, degrees: f32) -> Result<Self, &'static str> {
        set_angle(&mut self.properties, degrees)?;
        Ok(self)
    }
}
fn set_angle(properties: &mut MaterialProperties, degrees: f32) -> Result<(), &'static str> {
    if properties.movement != Movement::Granular {
        return Err("repose applies to granular materials");
    }
    if !degrees.is_finite() || !(0.0..90.0).contains(&degrees) {
        return Err("repose must be finite and 0 <= angle < 90 degrees");
    }
    properties.angle = degrees;
    properties.allowed_drop = (degrees.to_radians().tan() * REPOSE_SPAN as f32).round() as usize;
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialView {
    pub kind: Cell,
    pub name: String,
    pub color: [u8; 4],
    pub movement: Movement,
    pub repose_degrees: Option<f32>,
}
#[derive(Clone, Debug)]
pub struct MaterialRegistry {
    entries: Vec<MaterialDefinition>,
}
/// Previous settings name, now backed by the extensible registry.
pub type MaterialTable = MaterialRegistry;
impl MaterialRegistry {
    pub fn legacy() -> Self {
        let kinds = [
            Cell::Air,
            Cell::Sand,
            Cell::Block,
            Cell::Water,
            Cell::Stone,
            Cell::Fire,
            Cell::Steam,
            Cell::Oil,
            Cell::Gravel,
            Cell::Acid,
        ];
        Self {
            entries: kinds
                .into_iter()
                .map(|kind| {
                    let movement = match kind {
                        Cell::Air => Movement::Empty,
                        Cell::Sand | Cell::Gravel => Movement::Granular,
                        Cell::Water | Cell::Oil | Cell::Acid => Movement::Liquid,
                        Cell::Fire | Cell::Steam => Movement::Gas,
                        _ => Movement::Solid,
                    };
                    let mut definition = MaterialDefinition::new(
                        kind.name(),
                        movement,
                        kind.density() as f32,
                        CellData::new(kind, 0).to_rgba(),
                    );
                    definition.properties.angle = 45.0;
                    definition.properties.allowed_drop = REPOSE_SPAN;
                    definition.properties.lateral_spread = match kind {
                        Cell::Water => 5,
                        Cell::Oil => 4,
                        Cell::Acid => 3,
                        _ => 0,
                    };
                    definition
                })
                .collect(),
        }
    }
    /// Register a material without changing any engine enum or renderer code.
    ///
    /// ```
    /// use charon_physics::sim::rules::{MaterialRegistry, MaterialDefinition, Movement};
    /// let mut materials = MaterialRegistry::default();
    /// let nitrogen = materials.register(MaterialDefinition::new(
    ///     "Nitrogen", Movement::Gas, 1.165, [136, 180, 220, 255],
    /// )).unwrap();
    /// assert_eq!(materials.name(nitrogen), "Nitrogen");
    /// ```
    pub fn register(&mut self, definition: MaterialDefinition) -> Result<Cell, &'static str> {
        if self.entries.len() >= 256 {
            return Err("two-byte cells support at most 256 material IDs per registry");
        }
        if definition.name.trim().is_empty()
            || self
                .entries
                .iter()
                .any(|entry| entry.name == definition.name)
        {
            return Err("material name must be nonempty and unique");
        }
        if !definition.properties.density.is_finite() || definition.properties.density <= 0.0 {
            return Err("density must be finite and positive");
        }
        let id = Cell(self.entries.len() as u8);
        self.entries.push(definition);
        Ok(id)
    }
    pub fn buffer_bytes(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<MaterialDefinition>()
            + self
                .entries
                .iter()
                .map(|entry| entry.name.capacity())
                .sum::<usize>()
    }
    pub fn contains(&self, kind: Cell) -> bool {
        usize::from(kind.id()) < self.entries.len()
    }
    pub fn get(&self, kind: Cell) -> MaterialProperties {
        self.entries[usize::from(kind.id())].properties
    }
    pub fn name(&self, kind: Cell) -> &str {
        &self.entries[usize::from(kind.id())].name
    }
    pub fn find(&self, name: &str) -> Option<Cell> {
        self.entries
            .iter()
            .position(|entry| entry.name == name)
            .map(|i| Cell(i as u8))
    }
    pub fn palette(&self) -> Vec<MaterialView> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, entry)| MaterialView {
                kind: Cell(i as u8),
                name: entry.name.clone(),
                color: entry.color,
                movement: entry.properties.movement,
                repose_degrees: (entry.properties.movement == Movement::Granular)
                    .then_some(entry.properties.angle),
            })
            .collect()
    }
    pub fn color(&self, cell: CellData) -> [u8; 4] {
        if cell.kind.id() < 10 {
            cell.to_rgba()
        } else {
            self.entries[usize::from(cell.kind.id())].color
        }
    }
    pub fn set_density(&mut self, kind: Cell, density: f32) -> Result<(), &'static str> {
        if !self.contains(kind) || !density.is_finite() || density <= 0.0 {
            return Err("density must be finite and positive for a registered material");
        }
        self.entries[usize::from(kind.id())].properties.density = density;
        Ok(())
    }
    pub fn set_spread(&mut self, kind: Cell, spread: u8) -> Result<(), &'static str> {
        if !self.contains(kind) || spread > 32 {
            return Err("lateral spread must be 0..=32 for a registered material");
        }
        self.entries[usize::from(kind.id())]
            .properties
            .lateral_spread = spread;
        Ok(())
    }
    pub fn set_repose(&mut self, kind: Cell, degrees: f32) -> Result<(), &'static str> {
        if !self.contains(kind) {
            return Err("unknown material ID");
        }
        set_angle(
            &mut self.entries[usize::from(kind.id())].properties,
            degrees,
        )
    }
    pub fn set_friction(&mut self, kind: Cell, coefficient: f32) -> Result<(), &'static str> {
        if !coefficient.is_finite() || coefficient < 0.0 {
            return Err("friction must be finite and nonnegative");
        }
        self.set_repose(kind, coefficient.atan().to_degrees())
    }
}
impl Default for MaterialRegistry {
    fn default() -> Self {
        let mut table = Self::legacy();
        table.set_repose(Cell::Sand, 32.0).unwrap();
        table.set_repose(Cell::Gravel, 40.0).unwrap();
        table.set_density(Cell::Air, 1.225).unwrap();
        table
    }
}
