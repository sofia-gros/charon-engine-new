//! Pixel edits and block yield rules. The game supplies material identities;
//! terrain storage, falling debris and items remain owned by their plugins.
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelEdit {
    pub x: usize,
    pub y: usize,
    pub material: u8,
}
pub struct Rules {
    side: usize,
    threshold: usize,
    air: u8,
}
pub struct Resolution {
    pub edits: Vec<PixelEdit>,
    pub drops: usize,
}
impl Rules {
    pub fn new(side: usize, threshold: usize, air: u8) -> Result<Self, &'static str> {
        if !(1..=16).contains(&side) || threshold >= side * side {
            return Err("invalid block yield rules");
        }
        Ok(Self {
            side,
            threshold,
            air,
        })
    }
    /// Each threshold crossing creates one item cell and converts every remaining
    /// block pixel to debris. Debris has a different material identity, so further
    /// excavation cannot produce another block item, including after save/reload.
    pub fn resolve(
        &self,
        edits: Vec<PixelEdit>,
        read: impl Fn(usize, usize) -> u8,
        products: impl Fn(u8) -> Option<(u8, u8)>,
    ) -> Resolution {
        let mut blocks: BTreeMap<(usize, usize), (Vec<u8>, Vec<u8>)> = BTreeMap::new();
        for edit in &edits {
            let key = (edit.x / self.side, edit.y / self.side);
            let (before, after) = blocks.entry(key).or_insert_with(|| {
                let pixels: Vec<_> = (0..self.side * self.side)
                    .map(|i| {
                        read(
                            key.0 * self.side + i % self.side,
                            key.1 * self.side + i / self.side,
                        )
                    })
                    .collect();
                (pixels.clone(), pixels)
            });
            let _ = before;
            after[edit.y % self.side * self.side + edit.x % self.side] = edit.material;
        }
        let mut result = Resolution { edits, drops: 0 };
        for ((bx, by), (before, after)) in blocks {
            let original = before.iter().filter(|m| products(**m).is_some()).count();
            let remaining = after.iter().filter(|m| products(**m).is_some()).count();
            if original <= remaining || remaining > self.threshold {
                continue;
            }
            let Some(slot) = after.iter().position(|m| *m == self.air) else {
                continue;
            };
            let mut types = BTreeMap::new();
            for m in &before {
                if products(*m).is_some() {
                    *types.entry(*m).or_insert(0) += 1;
                }
            }
            let material = types
                .into_iter()
                .max_by_key(|(id, count)| (*count, std::cmp::Reverse(*id)))
                .unwrap()
                .0;
            for (i, m) in after.iter().enumerate() {
                if let Some((debris, _)) = products(*m) {
                    result.edits.push(PixelEdit {
                        x: bx * self.side + i % self.side,
                        y: by * self.side + i / self.side,
                        material: debris,
                    });
                }
            }
            let (_, item) = products(material).unwrap();
            result.edits.push(PixelEdit {
                x: bx * self.side + slot % self.side,
                y: by * self.side + slot / self.side,
                material: item,
            });
            result.drops += 1;
        }
        result
    }
}
/// Pixel-center rasterization of a disk inside a diameter×diameter footprint.
/// Gameplay may expose a smaller brush range; explosions can use larger disks.
pub fn disk(diameter: usize) -> Result<Vec<(i32, i32)>, &'static str> {
    if !(1..=64).contains(&diameter) {
        return Err("disk diameter must be 1..64");
    }
    let d = diameter as i32;
    Ok((0..d)
        .flat_map(|y| {
            (0..d).filter_map(move |x| {
                let dx = 2 * x + 1 - d;
                let dy = 2 * y + 1 - d;
                (dx * dx + dy * dy <= d * d).then_some((x, y))
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn circular_brush_covers_its_diameter_symmetrically_without_square_corners() {
        assert_eq!(disk(1).unwrap(), vec![(0, 0)]);
        assert_eq!(disk(2).unwrap().len(), 4);
        for diameter in 1..=16 {
            let pixels = disk(diameter).unwrap();
            let last = diameter as i32 - 1;
            for &(x, y) in &pixels {
                assert!((0..=last).contains(&x) && (0..=last).contains(&y));
                assert!(pixels.contains(&(last - x, y)));
                assert!(pixels.contains(&(x, last - y)));
                assert!(pixels.contains(&(y, x)));
            }
            assert!(pixels.iter().any(|&(x, _)| x == 0));
            assert!(pixels.iter().any(|&(x, _)| x == last));
            if diameter >= 4 {
                assert!(!pixels.contains(&(0, 0)));
                assert!(pixels.len() < diameter * diameter);
            }
        }
        assert!(disk(0).is_err());
        assert!(disk(65).is_err());
    }
    #[test]
    fn pixel_damage_yields_once_at_half_and_preserves_all_remaining_pixels_as_debris() {
        let rules = Rules::new(4, 8, 0).unwrap();
        let mut cells = vec![2; 16];
        let mut apply = |positions: Vec<usize>| {
            let edits = positions
                .into_iter()
                .map(|i| PixelEdit {
                    x: i % 4,
                    y: i / 4,
                    material: 0,
                })
                .collect();
            let result = rules.resolve(
                edits,
                |x, y| cells[y * 4 + x],
                |m| (m == 2).then_some((6, 7)),
            );
            for e in result.edits {
                cells[e.y * 4 + e.x] = e.material;
            }
            (result.drops, cells.clone())
        };
        let (n, c) = apply(vec![0]);
        assert_eq!(n, 0);
        assert_eq!(c.iter().filter(|m| **m == 2).count(), 15);
        let (n, c) = apply((1..7).collect());
        assert_eq!(n, 0);
        assert_eq!(c.iter().filter(|m| **m == 2).count(), 9);
        let (n, c) = apply(vec![7]);
        assert_eq!(n, 1);
        assert_eq!(c.iter().filter(|m| **m == 6).count(), 8);
        assert_eq!(c.iter().filter(|m| **m == 7).count(), 1);
        let (n, _) = apply((8..16).collect());
        assert_eq!(n, 0);
    }
    #[test]
    fn blast_spanning_blocks_yields_one_item_per_block_and_keeps_different_debris_types() {
        let rules = Rules::new(4, 8, 0).unwrap();
        let mut cells = [0; 32];
        for y in 0..4 {
            for x in 0..8 {
                cells[y * 8 + x] = if x < 4 { 2 } else { 3 };
            }
        }
        let edits = (0..2)
            .flat_map(|y| (0..8).map(move |x| PixelEdit { x, y, material: 0 }))
            .collect();
        let out = rules.resolve(
            edits,
            |x, y| cells[y * 8 + x],
            |m| match m {
                2 => Some((6, 7)),
                3 => Some((8, 9)),
                _ => None,
            },
        );
        assert_eq!(out.drops, 2);
        for e in out.edits {
            cells[e.y * 8 + e.x] = e.material;
        }
        assert_eq!(cells.iter().filter(|m| **m == 6).count(), 8);
        assert_eq!(cells.iter().filter(|m| **m == 8).count(), 8);
        assert_eq!(cells.iter().filter(|m| **m == 7 || **m == 9).count(), 2);
    }
}
