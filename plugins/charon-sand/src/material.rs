//! Immutable, validated material table. Dense cell IDs are local to this table;
//! stable keys and the table fingerprint identify definitions in saved inputs.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Empty,
    Static,
    Powder,
}

#[derive(Clone, Debug)]
pub struct Material {
    pub key: String,
    pub color: [u8; 4],
    pub motion: Motion,
    pub blocks_actors: bool,
    pub blocks_projectiles: bool,
}

#[derive(Clone, Debug)]
pub struct Materials {
    entries: Vec<Material>,
    air: u8,
    boundary: u8,
}

impl Materials {
    /// Cell IDs are entry positions. This bounded prototype supports 256 materials;
    /// callers get an error rather than silently truncating a registration.
    pub fn new(entries: Vec<Material>, air: u8, boundary: u8) -> Result<Self, String> {
        if entries.is_empty() || entries.len() > 256 {
            return Err("material table needs 1..256 entries".into());
        }
        for (index, entry) in entries.iter().enumerate() {
            if entry.key.is_empty()
                || !entry
                    .key
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c))
                || entries[..index].iter().any(|other| other.key == entry.key)
            {
                return Err(format!("invalid or duplicate material key: {}", entry.key));
            }
        }
        if entries
            .get(air as usize)
            .is_none_or(|m| m.motion != Motion::Empty || m.blocks_actors || m.blocks_projectiles)
            || entries.iter().filter(|m| m.motion == Motion::Empty).count() != 1
        {
            return Err("air must be the single empty material and have no collision".into());
        }
        if entries
            .get(boundary as usize)
            .is_none_or(|m| m.motion != Motion::Static || !m.blocks_actors || !m.blocks_projectiles)
        {
            return Err(
                "protected boundary must be static and block actors and projectiles".into(),
            );
        }
        Ok(Self {
            entries,
            air,
            boundary,
        })
    }
    pub fn get(&self, id: u8) -> Option<&Material> {
        self.entries.get(id as usize)
    }
    pub fn id(&self, key: &str) -> Option<u8> {
        self.entries
            .iter()
            .position(|m| m.key == key)
            .map(|i| i as u8)
    }
    pub fn air(&self) -> u8 {
        self.air
    }
    pub fn boundary(&self) -> u8 {
        self.boundary
    }
    /// Includes ID ordering and all behavior/appearance properties. It is not a
    /// cryptographic hash or a promise of long-term save-format compatibility.
    pub fn fingerprint(&self) -> u64 {
        let mut hash = 0xcbf29ce484222325_u64;
        for byte in [self.air, self.boundary]
            .into_iter()
            .chain(self.entries.iter().flat_map(|m| {
                m.key.bytes().chain([0]).chain(m.color).chain([
                    match m.motion {
                        Motion::Empty => 0,
                        Motion::Static => 1,
                        Motion::Powder => 2,
                    },
                    u8::from(m.blocks_actors),
                    u8::from(m.blocks_projectiles),
                ])
            }))
        {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
        hash
    }
}

/// Convenience palette for standalone sand demos and legacy input traces.
/// Games can supply their own complete table via `SandWorld::with_materials`.
impl Default for Materials {
    fn default() -> Self {
        Self::new(
            vec![
                Material {
                    key: "air".into(),
                    color: [16, 23, 36, 255],
                    motion: Motion::Empty,
                    blocks_actors: false,
                    blocks_projectiles: false,
                },
                Material {
                    key: "sand".into(),
                    color: [232, 181, 72, 255],
                    motion: Motion::Powder,
                    blocks_actors: true,
                    blocks_projectiles: true,
                },
                Material {
                    key: "wall".into(),
                    color: [82, 100, 121, 255],
                    motion: Motion::Static,
                    blocks_actors: true,
                    blocks_projectiles: true,
                },
            ],
            0,
            2,
        )
        .expect("valid convenience palette")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registration_rejects_ambiguous_or_unsafe_tables_and_tracks_definition_changes() {
        let base = Materials::default();
        assert_eq!(base.id("sand"), Some(1));
        assert_eq!(base.id("missing"), None);
        assert!(base.get(255).is_none());
        let mut duplicate = base.entries.clone();
        duplicate[1].key = "air".into();
        assert!(Materials::new(duplicate, 0, 2)
            .unwrap_err()
            .contains("duplicate"));
        let mut second_empty = base.entries.clone();
        second_empty[1].motion = Motion::Empty;
        assert!(Materials::new(second_empty, 0, 2).is_err());
        assert!(Materials::new(base.entries.clone(), 9, 2).is_err());
        assert!(Materials::new(base.entries.clone(), 0, 1).is_err());
        let mut changed = base.entries.clone();
        changed[1].blocks_projectiles = false;
        assert_ne!(
            Materials::new(changed, 0, 2).unwrap().fingerprint(),
            base.fingerprint()
        );
        let mut too_many = base.entries.clone();
        for i in 3..257 {
            let mut material = base.entries[1].clone();
            material.key = format!("extra.{i}");
            too_many.push(material);
        }
        assert!(Materials::new(too_many, 0, 2).is_err());
    }
}
