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
        })
    }
    pub fn upload(&mut self, renderer: &WindowRenderer) {
        self.texture = Some(renderer.upload_sprite_texture(self.atlas.texture()));
    }
    pub fn sprites(&mut self, frame: &Frame, position: Option<[f32; 2]>) -> Vec<Sprite<'_>> {
        let texture = self.texture.as_ref().expect("upload before drawing");
        let mut sprites: Vec<_> = frame
            .images
            .iter()
            .map(|&[x, y]| Sprite {
                texture,
                source: self.idle.frame_at(Duration::ZERO).rect,
                destination: [x, y, 8., 14.],
                flip_x: false,
            })
            .collect();
        let Some(body) = frame.player.filter(|body| !body.embedded) else {
            return sprites;
        };
        let clip = if frame.direction == 0 {
            &self.idle
        } else {
            &self.walk
        };
        let elapsed = Duration::from_secs_f64(frame.simulation_tick as f64 / 60.);
        let selected = clip.frame_at(elapsed);
        let [x, y] = position.unwrap_or([body.x(), body.y()]);
        sprites.push(Sprite {
            texture: self.texture.as_ref().expect("upload before drawing"),
            source: selected.rect,
            destination: [x - 0.5, y - 2., 8., 14.],
            // This sample sheet is authored facing right.
            flip_x: frame.facing < 0,
        });
        sprites
    }
}
