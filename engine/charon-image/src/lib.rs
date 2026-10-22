//! Immutable RGBA tiles shared by producers and consumers, without GPU dependencies.
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// Cloning copies tile handles, never pixel buffers. Replacing a tile preserves
/// all previously published images. Consumers may safely skip intermediate images.
#[derive(Clone)]
pub struct TiledImage {
    width: usize,
    height: usize,
    tile_size: usize,
    tiles: Vec<Arc<[u8]>>,
}
impl TiledImage {
    pub fn new(width: usize, height: usize, tile_size: usize) -> Self {
        assert!(width > 0 && height > 0 && tile_size > 0);
        width
            .checked_mul(height)
            .and_then(|v| v.checked_mul(4))
            .expect("image too large");
        let mut image = Self {
            width,
            height,
            tile_size,
            tiles: Vec::new(),
        };
        for index in 0..width.div_ceil(tile_size) * height.div_ceil(tile_size) {
            let rect = image.rect(index);
            image
                .tiles
                .push(vec![0; rect.width * rect.height * 4].into());
        }
        image
    }
    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }
    pub fn byte_len(&self) -> usize {
        self.width * self.height * 4
    }
    pub fn rect(&self, index: usize) -> Rect {
        assert!(index < self.width.div_ceil(self.tile_size) * self.height.div_ceil(self.tile_size));
        let columns = self.width.div_ceil(self.tile_size);
        let (x, y) = (
            index % columns * self.tile_size,
            index / columns * self.tile_size,
        );
        Rect {
            x,
            y,
            width: self.tile_size.min(self.width - x),
            height: self.tile_size.min(self.height - y),
        }
    }
    pub fn pixels(&self, index: usize) -> &[u8] {
        &self.tiles[index]
    }
    pub fn replace(&mut self, index: usize, rgba: Vec<u8>) {
        let rect = self.rect(index);
        assert_eq!(rgba.len(), rect.width * rect.height * 4);
        if self.pixels(index) != rgba {
            self.tiles[index] = rgba.into();
        }
    }
    /// Pointer identity is exact for retained immutable tiles, with no hash collisions.
    pub fn shares_tile(&self, other: &Self, index: usize) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.tile_size == other.tile_size
            && Arc::ptr_eq(&self.tiles[index], &other.tiles[index])
    }
    /// For export/readback validation only; normal presentation does not flatten.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut pixels = vec![0; self.byte_len()];
        for index in 0..self.tile_count() {
            let rect = self.rect(index);
            for row in 0..rect.height {
                let target = ((rect.y + row) * self.width + rect.x) * 4;
                let start = row * rect.width * 4;
                pixels[target..target + rect.width * 4]
                    .copy_from_slice(&self.pixels(index)[start..start + rect.width * 4]);
            }
        }
        pixels
    }
}
