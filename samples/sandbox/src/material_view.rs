//! Game-owned mapping from material keys to visual clips. No physics edits or
//! wake requests are needed when the texture frame changes.
use charon_assets::{Atlas, Clip, Texture};
use charon_render::{Result, TileAppearance};
use std::{sync::Arc, time::Duration};

pub struct MaterialVisual {
    atlas: Arc<Atlas>,
    ash: Clip,
    ash_id: usize,
    looks: Vec<TileAppearance>,
}
impl MaterialVisual {
    pub fn new() -> Result<Self> {
        let texture = Arc::new(Texture::decode(include_bytes!("../assets/materials.png"))?);
        let atlas = Arc::new(Atlas::from_json(
            texture,
            include_str!("../assets/materials.json"),
        )?);
        let ash = Clip::new(Arc::clone(&atlas), &["ash0", "ash1", "ash2"], true)?;
        let materials = crate::materials::palette();
        let mut looks = vec![
            TileAppearance {
                color: [255, 0, 255, 255],
                source: None
            };
            256
        ];
        for (id, look) in looks.iter_mut().enumerate() {
            if let Some(material) = materials.get(id as u8) {
                look.color = material.color;
            }
        }
        looks[materials
            .id("wall")
            .ok_or("wall appearance has no registered material")? as usize]
            .source = Some(atlas.frames()[atlas.frame("wall").ok_or("missing wall frame")?].rect);
        let ash_id = materials
            .id("demo.ash")
            .ok_or("ash appearance has no registered material")? as usize;
        Ok(Self {
            atlas,
            ash,
            ash_id,
            looks,
        })
    }
    pub fn update(&mut self, elapsed: Duration) {
        self.looks[self.ash_id].source = Some(self.ash.frame_at(elapsed).rect);
    }
    pub fn looks(&self) -> &[TileAppearance] {
        &self.looks
    }
    pub fn texture(&self) -> &Arc<Texture> {
        self.atlas.texture()
    }
}
