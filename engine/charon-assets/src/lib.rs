//! CPU-side image/atlas data and animation timing. Load once before play; no
//! renderer, physics or per-frame filesystem access is required.
use serde_json::Value;
use std::{io::Cursor, path::Path, sync::Arc, time::Duration};

type Result<T> = std::result::Result<T, String>;

pub struct Texture {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}
impl Texture {
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        if width == 0
            || height == 0
            || width > 4096
            || height > 4096
            || rgba.len() != width as usize * height as usize * 4
        {
            return Err("RGBA image must be 1..4096 per axis and match its dimensions".into());
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
    pub fn load(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if metadata.len() > 32 * 1024 * 1024 {
            return Err("image file exceeds 32 MiB".into());
        }
        Self::decode(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > 32 * 1024 * 1024 {
            return Err("image file exceeds 32 MiB".into());
        }
        let mut reader = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        if !matches!(
            reader.format(),
            Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
        ) {
            return Err("expected PNG or JPEG image".into());
        }
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        let decoded = reader.decode().map_err(|e| e.to_string())?.to_rgba8();
        Ok(Self {
            width: decoded.width(),
            height: decoded.height(),
            rgba: decoded.into_raw(),
        })
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub name: String,
    pub rect: [u32; 4],
    pub duration_ms: u32,
}

pub struct Atlas {
    texture: Arc<Texture>,
    frames: Vec<Frame>,
}
impl Atlas {
    pub fn single(texture: Arc<Texture>) -> Self {
        let rect = [0, 0, texture.width(), texture.height()];
        Self {
            texture,
            frames: vec![Frame {
                name: "default".into(),
                rect,
                duration_ms: 100,
            }],
        }
    }
    /// Aseprite / TexturePacker frames object or array. Rotated and trimmed
    /// exports are rejected explicitly in this initial importer.
    pub fn from_json(texture: Arc<Texture>, json: &str) -> Result<Self> {
        if json.len() > 8 * 1024 * 1024 {
            return Err("atlas JSON exceeds 8 MiB".into());
        }
        let value: Value = serde_json::from_str(json).map_err(|e| format!("atlas JSON: {e}"))?;
        let entries: Vec<_> = match &value["frames"] {
            Value::Object(map) => map
                .iter()
                .map(|(name, value)| (name.as_str(), value))
                .collect(),
            Value::Array(array) => array
                .iter()
                .map(|v| (v["filename"].as_str().unwrap_or(""), v))
                .collect(),
            _ => return Err("atlas needs a frames object or array".into()),
        };
        if entries.is_empty() || entries.len() > 16384 {
            return Err("atlas needs 1..16384 frames".into());
        }
        let mut frames: Vec<Frame> = Vec::with_capacity(entries.len());
        for (name, entry) in entries {
            if name.is_empty() || frames.iter().any(|f| f.name == name) {
                return Err("atlas frame name empty or duplicated".into());
            }
            if entry["rotated"].as_bool().unwrap_or(false)
                || entry["trimmed"].as_bool().unwrap_or(false)
            {
                return Err(format!(
                    "{name}: disable rotation and trimming when exporting this atlas"
                ));
            }
            let mut rect = [0; 4];
            for (i, key) in ["x", "y", "w", "h"].into_iter().enumerate() {
                rect[i] = entry["frame"][key]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or_else(|| format!("{name}: invalid frame {key}"))?;
            }
            let [x, y, w, h] = rect;
            if w == 0
                || h == 0
                || x.checked_add(w).is_none_or(|v| v > texture.width())
                || y.checked_add(h).is_none_or(|v| v > texture.height())
            {
                return Err(format!("{name}: frame outside texture"));
            }
            let duration_ms = match entry.get("duration") {
                Some(value) => value
                    .as_u64()
                    .filter(|v| (1..=60000).contains(v))
                    .ok_or_else(|| format!("{name}: invalid duration"))?
                    as u32,
                None => 100,
            };
            frames.push(Frame {
                name: name.into(),
                rect,
                duration_ms,
            });
        }
        Ok(Self { texture, frames })
    }
    pub fn load(image: &Path, json: &Path) -> Result<Self> {
        if std::fs::metadata(json).map_err(|e| e.to_string())?.len() > 8 * 1024 * 1024 {
            return Err("atlas JSON exceeds 8 MiB".into());
        }
        Self::from_json(
            Arc::new(Texture::load(image)?),
            &std::fs::read_to_string(json).map_err(|e| format!("{}: {e}", json.display()))?,
        )
    }
    pub fn texture(&self) -> &Arc<Texture> {
        &self.texture
    }
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }
    pub fn frame(&self, name: &str) -> Option<usize> {
        self.frames.iter().position(|f| f.name == name)
    }
}

/// Game-owned named clips can share one atlas (idle/walk/run/attack/etc.).
/// Time is supplied by the host: material appearance can use presentation time
/// while gameplay animation can follow simulation time or a paused clock.
pub struct Clip {
    atlas: Arc<Atlas>,
    frames: Vec<usize>,
    ends_ms: Vec<u64>,
    total_ms: u64,
    looping: bool,
}
impl Clip {
    pub fn new(atlas: Arc<Atlas>, names: &[&str], looping: bool) -> Result<Self> {
        if names.is_empty() {
            return Err("animation needs at least one frame".into());
        }
        let mut frames = Vec::with_capacity(names.len());
        let mut ends_ms = Vec::with_capacity(names.len());
        let mut total_ms = 0;
        for name in names {
            let index = atlas
                .frame(name)
                .ok_or_else(|| format!("unknown animation frame: {name}"))?;
            total_ms += u64::from(atlas.frames[index].duration_ms);
            frames.push(index);
            ends_ms.push(total_ms);
        }
        Ok(Self {
            atlas,
            frames,
            ends_ms,
            total_ms,
            looping,
        })
    }
    pub fn frame_at(&self, elapsed: Duration) -> &Frame {
        let elapsed_ms = elapsed.as_millis();
        let time = if self.looping {
            (elapsed_ms % u128::from(self.total_ms)) as u64
        } else {
            elapsed_ms.min(u128::from(self.total_ms - 1)) as u64
        };
        &self.atlas.frames[self.frames[self.ends_ms.partition_point(|&end| end <= time)]]
    }
    pub fn atlas(&self) -> &Arc<Atlas> {
        &self.atlas
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn encoded(format: image::ImageFormat) -> Vec<u8> {
        let image = image::RgbImage::from_fn(4, 2, |x, _| {
            if x < 2 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            }
        });
        let mut cursor = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut cursor, format)
            .unwrap();
        cursor.into_inner()
    }
    #[test]
    fn decodes_png_jpeg_and_rejects_invalid_images() {
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let texture = Texture::decode(&encoded(format)).unwrap();
            assert_eq!(
                (texture.width(), texture.height(), texture.rgba().len()),
                (4, 2, 32)
            );
            if format == image::ImageFormat::Png {
                assert_eq!(&texture.rgba()[..4], &[255, 0, 0, 255]);
            }
        }
        assert!(Texture::decode(b"broken").is_err());
    }
    #[test]
    fn validates_atlases_and_selects_clip_frames_at_exact_boundaries() {
        let texture = Arc::new(Texture::decode(&encoded(image::ImageFormat::Png)).unwrap());
        let json = r#"{"frames":{"walk0":{"frame":{"x":0,"y":0,"w":2,"h":2},"duration":80},"walk1":{"frame":{"x":2,"y":0,"w":2,"h":2},"duration":120}}}"#;
        let atlas = Arc::new(Atlas::from_json(texture.clone(), json).unwrap());
        let clip = Clip::new(atlas.clone(), &["walk0", "walk1"], true).unwrap();
        for (ms, name) in [
            (0, "walk0"),
            (79, "walk0"),
            (80, "walk1"),
            (199, "walk1"),
            (200, "walk0"),
            (280, "walk1"),
        ] {
            assert_eq!(clip.frame_at(Duration::from_millis(ms)).name, name);
        }
        let attack = Clip::new(atlas.clone(), &["walk0", "walk1"], false).unwrap();
        assert_eq!(attack.frame_at(Duration::MAX).name, "walk1");
        assert!(Clip::new(atlas, &["missing"], true).is_err());
        assert!(Atlas::from_json(
            texture.clone(),
            &json.replace("\"duration\":80", "\"duration\":0")
        )
        .is_err());
        assert!(Atlas::from_json(texture.clone(), &json.replace("\"w\":2", "\"w\":5")).is_err());
        assert!(Atlas::from_json(
            texture.clone(),
            &json.replace("\"duration\":80", "\"trimmed\":true")
        )
        .is_err());
        let array = r#"{"frames":[{"filename":"idle","frame":{"x":0,"y":0,"w":4,"h":2}}]}"#;
        assert_eq!(
            Atlas::from_json(texture, array).unwrap().frame("idle"),
            Some(0)
        );
    }
}
