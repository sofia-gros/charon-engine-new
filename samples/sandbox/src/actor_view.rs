//! Example game presentation: share one image between independently named clips.
use crate::game::Frame;
use charon_assets::{Atlas, Clip, Texture};
use charon_render::{Result, Sprite, SpriteTexture, WindowRenderer};
use std::{sync::Arc, time::Duration};

pub struct ActorVisual {
    atlas: Arc<Atlas>,
    idle: Clip,
    walk: Clip,
    texture: Option<SpriteTexture>,
    facing_left: bool,
}
impl ActorVisual {
    pub fn new() -> Result<Self> {
        let image = Arc::new(Texture::decode(include_bytes!("../assets/actor.png"))?);
        let atlas = Arc::new(Atlas::from_json(
            image,
            include_str!("../assets/actor.json"),
        )?);
        let idle = Clip::new(Arc::clone(&atlas), &["idle"], true)?;
        let walk = Clip::new(Arc::clone(&atlas), &["walk0", "walk1"], true)?;
        Ok(Self {
            atlas,
            idle,
            walk,
            texture: None,
            facing_left: false,
        })
    }
    pub fn upload(&mut self, renderer: &WindowRenderer) {
        self.texture = Some(renderer.upload_sprite_texture(self.atlas.texture()));
    }
    pub fn sprites(&mut self, frame: &Frame, position: Option<[f32; 2]>) -> Vec<Sprite<'_>> {
        let Some(body) = frame.player.filter(|body| !body.embedded) else {
            return vec![];
        };
        if frame.direction != 0 {
            self.facing_left = frame.direction < 0;
        }
        let clip = if frame.direction == 0 {
            &self.idle
        } else {
            &self.walk
        };
        let elapsed = Duration::from_secs_f64(frame.simulation_tick as f64 / 60.);
        let selected = clip.frame_at(elapsed);
        let [x, y] = position.unwrap_or([body.x(), body.y()]);
        vec![Sprite {
            texture: self.texture.as_ref().expect("upload before drawing"),
            source: selected.rect,
            destination: [x - 0.5, y - 2., 8., 14.],
            flip_x: self.facing_left,
        }]
    }
}
