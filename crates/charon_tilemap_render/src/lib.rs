//! Data-only adapter from tile/sprite snapshots to generic render packets.
use charon_render::{DrawFrame, PassId, Quad, TextureId};
use charon_tilemap::{Atlas, Sprite, Tilemap};
pub fn append_tiles(
    frame: &mut DrawFrame,
    map: &Tilemap,
    atlas: &Atlas,
    origin: [f32; 2],
    scale: f32,
    layer: i32,
    pass: PassId,
) -> Result<(), String> {
    if !scale.is_finite() || scale <= 0. || origin.iter().any(|v| !v.is_finite()) {
        return Err("invalid tile transform".into());
    }
    let [w, h] = map.tile_size();
    for (x, y, id) in map.occupied() {
        let def = map.definition(id).ok_or("unknown tile")?;
        let uv = atlas.uv(def.atlas_index).ok_or("tile outside atlas")?;
        frame.quads.push(Quad {
            rect: [
                origin[0] + x as f32 * w as f32 * scale,
                origin[1] + y as f32 * h as f32 * scale,
                w as f32 * scale,
                h as f32 * scale,
            ],
            uv,
            tint: def.tint,
            texture: TextureId(atlas.texture),
            layer,
            pass,
        });
    }
    Ok(())
}
pub fn append_sprite(
    frame: &mut DrawFrame,
    sprite: &Sprite,
    atlas: &Atlas,
    pass: PassId,
) -> Result<(), String> {
    let uv = atlas.uv(sprite.atlas_index).ok_or("sprite outside atlas")?;
    frame.quads.push(Quad {
        rect: [
            sprite.position[0],
            sprite.position[1],
            sprite.size[0],
            sprite.size[1],
        ],
        uv,
        tint: sprite.tint,
        texture: TextureId(atlas.texture),
        layer: sprite.layer,
        pass,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_tilemap::{TileDefinition, Tilemap};
    #[test]
    fn adapters_preserve_atlas_layer_and_world_scale() {
        let mut map = Tilemap::new(3, 2, [8, 16]).unwrap();
        let id = map
            .register(TileDefinition {
                name: "tile".into(),
                solid: true,
                atlas_index: 1,
                tint: [0.5; 4],
            })
            .unwrap();
        map.set(2, 1, Some(id)).unwrap();
        let mut frame = DrawFrame::new([100, 100]);
        let atlas = Atlas {
            texture: 4,
            size: [16, 8],
            tile_size: [8, 8],
        };
        append_tiles(&mut frame, &map, &atlas, [3., 5.], 2., 7, PassId(2)).unwrap();
        let quad = &frame.quads[0];
        assert_eq!(quad.rect, [35., 37., 16., 32.]);
        assert_eq!(quad.uv, [0.5, 0., 0.5, 1.]);
        assert_eq!(quad.layer, 7);
        assert_eq!(quad.texture, TextureId(4));
        assert!(append_tiles(&mut frame, &map, &atlas, [0., 0.], f32::NAN, 0, PassId(0)).is_err());
    }
}
