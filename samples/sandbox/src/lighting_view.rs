//! Game-owned optical properties and emissive-block collection. The renderer
//! accepts lights and optics without knowing any game material identities.
use charon_render::{Camera2D, IndexedImage, LightMaterial, Lighting, PointLight};
use std::collections::BTreeMap;
#[derive(Default)]
pub struct LightingView {
    image: Option<IndexedImage>,
    sources: BTreeMap<usize, Vec<PointLight>>,
    pub omitted: usize,
}
impl LightingView {
    pub fn prepare(&mut self, image: &IndexedImage, camera: Camera2D) -> Lighting {
        self.sources.retain(|&tile, _| tile < image.tile_count());
        for tile in 0..image.tile_count() {
            if self
                .image
                .as_ref()
                .is_some_and(|old| image.shares_tile(old, tile))
            {
                continue;
            }
            let rect = image.rect(tile);
            let data = image.pixels(tile);
            let mut sources = vec![];
            for y in (0..rect.height).step_by(4) {
                for x in (0..rect.width).step_by(4) {
                    let mut best: Option<(usize, usize, usize)> = None;
                    for dy in 0..4.min(rect.height - y) {
                        for dx in 0..4.min(rect.width - x) {
                            if data[(y + dy) * rect.width + x + dx] == crate::materials::ORE {
                                let d = dx.abs_diff(2).pow(2) + dy.abs_diff(2).pow(2);
                                if best.is_none_or(|b| d < b.0) {
                                    best = Some((d, x + dx, y + dy));
                                }
                            }
                        }
                    }
                    if let Some((_, x, y)) = best {
                        sources.push(PointLight {
                            position: [(rect.x + x) as f32 + 0.5, (rect.y + y) as f32 + 0.5],
                            radius: 112.,
                            color: [3.0, 1.3, 0.4],
                        });
                    }
                }
            }
            self.sources.insert(tile, sources);
        }
        self.image = Some(image.clone());
        let center = [
            camera.origin()[0] + camera.size()[0] * 0.5,
            camera.origin()[1] + camera.size()[1] * 0.5,
        ];
        let mut lights: Vec<_> = self
            .sources
            .values()
            .flatten()
            .filter(|l| {
                (0..2).all(|i| {
                    l.position[i] + l.radius >= camera.origin()[i]
                        && l.position[i] - l.radius <= camera.origin()[i] + camera.size()[i]
                })
            })
            .copied()
            .collect();
        lights.sort_by(|a, b| {
            let distance = |l: &PointLight| {
                (l.position[0] - center[0]).powi(2) + (l.position[1] - center[1]).powi(2)
            };
            distance(a)
                .total_cmp(&distance(b))
                .then(a.position[0].total_cmp(&b.position[0]))
                .then(a.position[1].total_cmp(&b.position[1]))
        });
        self.omitted = lights.len().saturating_sub(32);
        lights.truncate(32);
        lights.sort_by(|a, b| {
            a.position[0]
                .total_cmp(&b.position[0])
                .then(a.position[1].total_cmp(&b.position[1]))
        });
        let palette = crate::materials::palette();
        let mut materials = vec![LightMaterial::default(); 256];
        for (id, optics) in materials.iter_mut().enumerate() {
            if let Some(m) = palette.get(id as u8) {
                optics.opacity = match m.motion {
                    charon_sand::Motion::Empty => 0.,
                    charon_sand::Motion::Liquid => 0.015,
                    _ => 1.,
                };
            }
        }
        materials[crate::materials::ORE as usize] = LightMaterial {
            emission: [1.4, 0.6, 0.15],
            opacity: 0.,
        };
        Lighting {
            materials,
            lights,
            ambient: 0.8,
            interior_darkness: 0.88,
            interior_radius: 16,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn block_sources_follow_pixel_edits_and_expose_the_visible_light_budget() {
        let mut view = LightingView::default();
        let mut image = IndexedImage::new(32, 32, 32);
        let mut pixels = vec![0; 1024];
        for y in 0..24 {
            for x in 0..24 {
                pixels[y * 32 + x] = crate::materials::ORE;
            }
        }
        image.replace(0, pixels);
        let camera = Camera2D::new([0., 0.], [32., 32.]).unwrap();
        let lights = view.prepare(&image, camera);
        assert_eq!(lights.lights.len(), 32);
        assert_eq!(view.omitted, 4);
        assert_eq!(view.prepare(&image, camera), lights);
        image.replace(0, vec![0; 1024]);
        assert!(view.prepare(&image, camera).lights.is_empty());
        assert_eq!(view.omitted, 0);
        let small = IndexedImage::new(16, 16, 16);
        assert!(view.prepare(&small, camera).lights.is_empty());
    }
}
