//! Screen-space text and buttons. No window, renderer, sand or story dependency.
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use unicode_normalization::UnicodeNormalization;
pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}
impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Result<Self> {
        if ![x, y, width, height, x + width, y + height]
            .into_iter()
            .all(f32::is_finite)
            || width <= 0.
            || height <= 0.
        {
            return Err("UI rectangle must be finite with positive dimensions".into());
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }
    pub fn xywh(self) -> [f32; 4] {
        [self.x, self.y, self.width, self.height]
    }
    pub fn contains(self, p: [f32; 2]) -> bool {
        p[0] >= self.x
            && p[0] < self.x + self.width
            && p[1] >= self.y
            && p[1] < self.y + self.height
    }
}
/// Match the renderer's stretched logical canvas. Points outside the client area
/// or a zero-size/minimized window do not target any widget.
pub fn logical_pointer(point: [f32; 2], client: [f32; 2], logical: [f32; 2]) -> Option<[f32; 2]> {
    if !point
        .into_iter()
        .chain(client)
        .chain(logical)
        .all(f32::is_finite)
        || (0..2)
            .any(|i| client[i] <= 0. || logical[i] <= 0. || point[i] < 0. || point[i] >= client[i])
    {
        return None;
    }
    Some(std::array::from_fn(|i| point[i] / client[i] * logical[i]))
}
#[derive(Clone)]
pub struct Button<K> {
    pub id: K,
    pub rect: Rect,
    pub label: String,
}
pub struct Buttons<K> {
    items: Vec<Button<K>>,
    focused: Option<K>,
    hovered: Option<K>,
    pressed: Option<K>,
}
impl<K: Copy + Eq> Default for Buttons<K> {
    fn default() -> Self {
        Self {
            items: vec![],
            focused: None,
            hovered: None,
            pressed: None,
        }
    }
}
impl<K: Copy + Eq> Buttons<K> {
    pub fn replace(&mut self, items: Vec<Button<K>>) -> Result<()> {
        for (index, item) in items.iter().enumerate() {
            if item.label.trim().is_empty()
                || items[..index].iter().any(|other| other.id == item.id)
            {
                return Err("UI buttons need unique IDs and nonempty labels".into());
            }
        }
        self.focused = self
            .focused
            .filter(|id| items.iter().any(|b| b.id == *id))
            .or_else(|| items.first().map(|b| b.id));
        self.items = items;
        self.hovered = None;
        self.pressed = None;
        Ok(())
    }
    pub fn items(&self) -> &[Button<K>] {
        &self.items
    }
    pub fn focused(&self) -> Option<K> {
        self.focused
    }
    pub fn hovered(&self) -> Option<K> {
        self.hovered
    }
    pub fn pointer_move(&mut self, point: Option<[f32; 2]>) {
        self.hovered = point.and_then(|p| {
            self.items
                .iter()
                .rev()
                .find(|b| b.rect.contains(p))
                .map(|b| b.id)
        });
    }
    pub fn press(&mut self) {
        self.pressed = self.hovered;
        if self.hovered.is_some() {
            self.focused = self.hovered;
        }
    }
    pub fn release(&mut self) -> Option<K> {
        self.pressed.take().filter(|id| Some(*id) == self.hovered)
    }
    pub fn cancel(&mut self) {
        self.pressed = None;
        self.hovered = None;
    }
    pub fn navigate(&mut self, backwards: bool) {
        if self.items.is_empty() {
            return;
        }
        let current = self
            .items
            .iter()
            .position(|b| Some(b.id) == self.focused)
            .unwrap_or(0);
        let next = if backwards {
            (current + self.items.len() - 1) % self.items.len()
        } else {
            (current + 1) % self.items.len()
        };
        self.focused = Some(self.items[next].id);
    }
}

pub struct Canvas {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}
impl Canvas {
    pub fn new(width: usize, height: usize) -> Result<Self> {
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return Err("UI canvas needs dimensions 1..4096".into());
        }
        Ok(Self {
            width,
            height,
            rgba: vec![0; width * height * 4],
        })
    }
    pub fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
    pub fn fill(&mut self, rect: Rect, color: [u8; 4]) {
        let [x, y, w, h] = rect.xywh();
        for py in (y.floor().max(0.) as usize)..((y + h).ceil().max(0.) as usize).min(self.height) {
            for px in
                (x.floor().max(0.) as usize)..((x + w).ceil().max(0.) as usize).min(self.width)
            {
                self.blend(px, py, color, 255);
            }
        }
    }
    fn blend(&mut self, x: usize, y: usize, color: [u8; 4], coverage: u8) {
        let index = (y * self.width + x) * 4;
        let sa = u32::from(color[3]) * u32::from(coverage) / 255;
        if sa == 0 {
            return;
        }
        if sa == 255 {
            self.rgba[index..index + 4].copy_from_slice(&color);
            return;
        }
        let da = u32::from(self.rgba[index + 3]);
        let alpha = (sa * 255 + da * (255 - sa) + 127) / 255;
        for (channel, value) in color[..3].iter().enumerate() {
            self.rgba[index + channel] = ((u32::from(*value) * sa * 255
                + u32::from(self.rgba[index + channel]) * da * (255 - sa)
                + alpha * 127)
                / (alpha * 255))
                .min(255) as u8;
        }
        self.rgba[index + 3] = alpha as u8;
    }
}
struct Raster {
    metrics: fontdue::Metrics,
    pixels: Vec<u8>,
}
struct Glyph {
    raster: Arc<Raster>,
    x: f32,
    y: f32,
}
pub struct TextLayout {
    pub width: f32,
    pub height: f32,
    pub lines: Vec<String>,
    glyphs: Vec<Glyph>,
}
pub struct Text {
    font: fontdue::Font,
    cache: HashMap<(char, u32), Arc<Raster>>,
}
impl Text {
    pub fn new(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            font: fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
                .map_err(str::to_owned)?,
            cache: HashMap::new(),
        })
    }
    pub fn cached_glyphs(&self) -> usize {
        self.cache.len()
    }
    fn raster(&mut self, ch: char, size: f32) -> Result<Arc<Raster>> {
        if self.font.lookup_glyph_index(ch) == 0 {
            return Err(format!(
                "font has no glyph for {ch:?} (U+{:04X})",
                ch as u32
            ));
        }
        let key = (ch, size.to_bits());
        if let Some(raster) = self.cache.get(&key) {
            return Ok(Arc::clone(raster));
        }
        let (metrics, pixels) = self.font.rasterize(ch, size);
        let raster = Arc::new(Raster { metrics, pixels });
        if self.cache.len() >= 4096 {
            self.cache.clear();
        }
        self.cache.insert(key, Arc::clone(&raster));
        Ok(raster)
    }
    /// Horizontal Japanese/Latin text, NFC normalization and UAX #14 wrapping.
    /// Unsupported characters produce an error instead of invisible text.
    pub fn layout(&mut self, value: &str, size: f32, max_width: f32) -> Result<TextLayout> {
        if !size.is_finite()
            || !(1.0..=256.0).contains(&size)
            || !max_width.is_finite()
            || max_width <= 0.
            || value.len() > 65536
        {
            return Err("invalid text size/width or text exceeds 64 KiB".into());
        }
        let normalized: String = value.replace("\r\n", "\n").nfc().collect();
        let metrics = self
            .font
            .horizontal_line_metrics(size)
            .ok_or("font has no horizontal metrics")?;
        let line_height = metrics.new_line_size.ceil().max(size);
        let mut output = TextLayout {
            width: 0.,
            height: 0.,
            lines: vec![],
            glyphs: vec![],
        };
        for paragraph in normalized.split('\n') {
            let breaks: HashSet<_> = unicode_linebreak::linebreaks(paragraph)
                .map(|(index, _)| index)
                .collect();
            let mut characters = Vec::new();
            for (index, ch) in paragraph.char_indices() {
                characters.push((index, ch, self.raster(ch, size)?));
            }
            if characters.is_empty() {
                output.lines.push(String::new());
                output.height += line_height;
                continue;
            }
            let mut start = 0;
            while start < characters.len() {
                let (mut end, mut width, mut last_break) = (start, 0., None);
                while end < characters.len() {
                    let advance = characters[end].2.metrics.advance_width;
                    if width + advance > max_width {
                        break;
                    }
                    width += advance;
                    end += 1;
                    let byte_end = characters.get(end).map_or(paragraph.len(), |c| c.0);
                    if breaks.contains(&byte_end) {
                        last_break = Some(end);
                    }
                }
                if end == start {
                    return Err("text column is narrower than one glyph".into());
                }
                if end < characters.len() {
                    end = last_break.unwrap_or(end);
                }
                let mut x = 0.;
                let mut line = String::new();
                for (_, ch, raster) in &characters[start..end] {
                    output.glyphs.push(Glyph {
                        raster: Arc::clone(raster),
                        x: x + raster.metrics.xmin as f32,
                        y: output.height + metrics.ascent
                            - raster.metrics.height as f32
                            - raster.metrics.ymin as f32,
                    });
                    x += raster.metrics.advance_width;
                    line.push(*ch);
                }
                output.width = output.width.max(x);
                output.lines.push(line);
                output.height += line_height;
                start = end;
            }
        }
        Ok(output)
    }
    pub fn paint(
        &self,
        canvas: &mut Canvas,
        layout: &TextLayout,
        position: [f32; 2],
        color: [u8; 4],
        clip: Rect,
    ) {
        for glyph in &layout.glyphs {
            let left = (position[0] + glyph.x).round() as i64;
            let top = (position[1] + glyph.y).round() as i64;
            for row in 0..glyph.raster.metrics.height {
                for column in 0..glyph.raster.metrics.width {
                    let (x, y) = (left + column as i64, top + row as i64);
                    if x < 0
                        || y < 0
                        || x >= canvas.width as i64
                        || y >= canvas.height as i64
                        || !clip.contains([x as f32 + 0.5, y as f32 + 0.5])
                    {
                        continue;
                    }
                    canvas.blend(
                        x as usize,
                        y as usize,
                        color,
                        glyph.raster.pixels[row * glyph.raster.metrics.width + column],
                    );
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transparent_ui_pixels_keep_straight_alpha_for_gpu_compositing() {
        let mut canvas = Canvas::new(2, 2).unwrap();
        let rect = Rect::new(0., 0., 1., 1.).unwrap();
        canvas.fill(rect, [200, 100, 50, 128]);
        assert_eq!(&canvas.rgba()[..4], &[200, 100, 50, 128]);
        canvas.fill(rect, [0, 0, 0, 0]);
        assert_eq!(&canvas.rgba()[..4], &[200, 100, 50, 128]);
        canvas.fill(rect, [10, 20, 30, 255]);
        assert_eq!(&canvas.rgba()[..4], &[10, 20, 30, 255]);
        assert_eq!(&canvas.rgba()[4..8], &[0, 0, 0, 0]);
    }
    #[test]
    fn pointer_capture_focus_and_resize_do_not_activate_the_wrong_button() {
        let mut buttons = Buttons::default();
        let items = vec![
            Button {
                id: 0,
                rect: Rect::new(10., 10., 80., 30.).unwrap(),
                label: "選ぶ".into(),
            },
            Button {
                id: 1,
                rect: Rect::new(100., 10., 80., 30.).unwrap(),
                label: "戻る".into(),
            },
        ];
        buttons.replace(items.clone()).unwrap();
        buttons.pointer_move(logical_pointer([40., 40.], [400., 200.], [200., 100.]));
        buttons.press();
        buttons.pointer_move(Some([110., 20.]));
        assert_eq!(buttons.release(), None);
        buttons.pointer_move(Some([20., 20.]));
        buttons.press();
        buttons.replace(items).unwrap();
        assert_eq!(buttons.release(), None);
        buttons.navigate(false);
        assert_eq!(buttons.focused(), Some(1));
        buttons.pointer_move(Some([110., 20.]));
        buttons.press();
        assert_eq!(buttons.release(), Some(1));
        buttons.press();
        buttons.cancel();
        assert_eq!(buttons.release(), None);
        assert!(logical_pointer([0., 0.], [0., 0.], [640., 360.]).is_none());
    }
    #[test]
    fn japanese_text_wraps_renders_clips_and_reuses_glyphs() {
        let bytes = include_bytes!("../../../samples/dialogue/assets/MPLUS1p-Regular.ttf");
        let mut text = Text::new(bytes).unwrap();
        let layout = text.layout("あいう。えお\nか\u{3099}き", 20., 60.).unwrap();
        assert!(layout.lines.len() >= 3);
        assert!(layout.lines.iter().all(|line| !line.starts_with('。')));
        assert!(layout.lines.iter().any(|line| line.contains('が')));
        let cached = text.cached_glyphs();
        text.layout("あいう。えお\nがき", 20., 60.).unwrap();
        assert_eq!(text.cached_glyphs(), cached);
        let mut canvas = Canvas::new(100, 100).unwrap();
        let clip = Rect::new(10., 10., 30., 60.).unwrap();
        text.paint(&mut canvas, &layout, [0., 0.], [255; 4], clip);
        let mut painted = 0;
        for y in 0..100 {
            for x in 0..100 {
                if canvas.rgba()[(y * 100 + x) * 4 + 3] > 0 {
                    assert!(clip.contains([x as f32 + 0.5, y as f32 + 0.5]));
                    painted += 1;
                }
            }
        }
        assert!(painted > 0);
        assert!(text.layout("\u{10FFFF}", 20., 60.).is_err());
    }
}
