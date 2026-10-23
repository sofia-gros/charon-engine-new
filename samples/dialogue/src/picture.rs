//! Game-specific panel layout and scenery composed from the reusable UI plugin.
use crate::story::{self, Page};
use charon_assets::Texture;
use charon_dialogue::Frame;
use charon_render::{Result, SpriteTexture, WindowRenderer};
use charon_ui::{Button, Buttons, Canvas, Rect, Text};

pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 360;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Continue,
    Choose(usize),
    Reset,
}
pub struct Ui {
    pub buttons: Buttons<Action>,
    text: Text,
    texture: Option<SpriteTexture>,
    page: Option<Page>,
    dirty: bool,
    status: String,
    pub redraws: u64,
}
impl Ui {
    pub fn new() -> Result<Self> {
        let mut text = Text::new(include_bytes!("../assets/MPLUS1p-Regular.ttf"))?;
        // Validate all game text before opening the window, not only visited pages.
        for node in story::nodes() {
            text.layout(&node.text, 20., 536.)?;
            for choice in node.choices.iter() {
                text.layout(&choice.label, 18., 244.)?;
            }
        }
        Ok(Self {
            buttons: Buttons::default(),
            text,
            texture: None,
            page: None,
            dirty: true,
            status: String::new(),
            redraws: 0,
        })
    }
    pub fn set_status(&mut self, frame: &crate::game::Frame) {
        let device = match frame.audio {
            charon_audio::DeviceState::Starting => "準備中",
            charon_audio::DeviceState::Ready => "出力中",
            charon_audio::DeviceState::Unavailable(_) => "利用不可",
        };
        let status = format!(
            "音声：{}　音量：{}%{}　BGM：{}\n{}",
            device,
            frame.volume,
            if frame.muted { "（消音）" } else { "" },
            if !matches!(frame.audio, charon_audio::DeviceState::Ready) {
                "停止"
            } else if frame.music_paused {
                "一時停止"
            } else {
                "再生"
            },
            frame.notice
        );
        if status != self.status {
            self.status = status;
            self.dirty = true;
        }
    }
    pub fn cached_glyphs(&self) -> usize {
        self.text.cached_glyphs()
    }
    pub fn texture(&self) -> &SpriteTexture {
        self.texture.as_ref().expect("render UI before presenting")
    }
    pub fn set_page(&mut self, frame: &Frame<Page>) -> Result<()> {
        if self.page == Some(frame.page) {
            return Ok(());
        }
        let mut buttons = Vec::new();
        if !frame.choices.is_empty() {
            for (index, choice) in frame.choices.iter().enumerate() {
                buttons.push(Button {
                    id: Action::Choose(index),
                    rect: Rect::new(44. + index as f32 * 280., 284., 272., 44.)?,
                    label: format!("{}  {}", index + 1, choice.label),
                });
            }
        } else {
            buttons.push(Button {
                id: if frame.can_advance {
                    Action::Continue
                } else {
                    Action::Reset
                },
                rect: Rect::new(388., 284., 208., 44.)?,
                label: if frame.can_advance {
                    "次へ"
                } else {
                    "最初から"
                }
                .into(),
            });
        }
        self.buttons.replace(buttons)?;
        self.page = Some(frame.page);
        self.dirty = true;
        Ok(())
    }
    pub fn pointer_move(&mut self, point: Option<[f32; 2]>) {
        let before = self.buttons.hovered();
        self.buttons.pointer_move(point);
        self.dirty |= before != self.buttons.hovered();
    }
    pub fn press(&mut self) {
        self.buttons.press();
        self.dirty = true;
    }
    pub fn release(&mut self) -> Option<Action> {
        self.dirty = true;
        self.buttons.release()
    }
    pub fn cancel(&mut self) {
        self.buttons.cancel();
        self.dirty = true;
    }
    pub fn navigate(&mut self, backwards: bool) {
        self.buttons.navigate(backwards);
        self.dirty = true;
    }
    pub fn render(&mut self, view: &WindowRenderer, frame: &Frame<Page>) -> Result<bool> {
        if !self.dirty {
            return Ok(false);
        }
        let mut canvas = Canvas::new(WIDTH, HEIGHT)?;
        let all = Rect::new(0., 0., WIDTH as f32, HEIGHT as f32)?;
        let accent = match frame.page {
            Page::River => [95, 199, 211, 255],
            Page::Hill => [167, 204, 128, 255],
            _ => [231, 186, 110, 255],
        };
        let title = self.text.layout("分かれ道の物語", 24., 540.)?;
        self.text
            .paint(&mut canvas, &title, [40., 37.], accent, all);
        let help = self.text.layout(
            "クリック / Enter：決定　Tab / 矢印：選択　R：最初から",
            12.,
            560.,
        )?;
        self.text
            .paint(&mut canvas, &help, [40., 76.], [190, 199, 214, 255], all);
        let status = self.text.layout(&self.status, 11., 560.)?;
        self.text
            .paint(&mut canvas, &status, [40., 103.], [190, 211, 214, 255], all);
        canvas.fill(Rect::new(28., 152., 584., 190.)?, [17, 25, 40, 255]);
        canvas.fill(Rect::new(28., 152., 584., 2.)?, accent);
        let body = self.text.layout(&frame.text, 20., 536.)?;
        if body.height > 116. {
            return Err("sample dialogue text exceeds its panel; paginate this game node".into());
        }
        self.text.paint(
            &mut canvas,
            &body,
            [52., 164.],
            [229, 234, 242, 255],
            Rect::new(52., 164., 536., 116.)?,
        );
        for button in self.buttons.items() {
            let [x, y, w, h] = button.rect.xywh();
            let focused = self.buttons.focused() == Some(button.id);
            let hovered = self.buttons.hovered() == Some(button.id);
            canvas.fill(
                button.rect,
                if focused {
                    [65, 81, 104, 255]
                } else if hovered {
                    [47, 63, 84, 255]
                } else {
                    [31, 44, 62, 255]
                },
            );
            if focused {
                canvas.fill(Rect::new(x, y, w, 2.)?, accent);
                canvas.fill(Rect::new(x, y + h - 2., w, 2.)?, accent);
            }
            let label = self.text.layout(&button.label, 18., w - 24.)?;
            self.text.paint(
                &mut canvas,
                &label,
                [x + (w - label.width) / 2., y + (h - label.height) / 2.],
                [245, 240, 224, 255],
                button.rect,
            );
        }
        let image = Texture::from_rgba(WIDTH as u32, HEIGHT as u32, canvas.into_rgba())?;
        self.texture = Some(view.upload_sprite_texture(&image));
        self.dirty = false;
        self.redraws += 1;
        Ok(true)
    }
}
pub fn background(page: Page) -> Vec<u8> {
    let mut rgba = vec![0; WIDTH * HEIGHT * 4];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let hill = 118 + ((x as isize - 400).unsigned_abs() / 7).min(70);
            let color = if y >= hill {
                [35, 53, 72, 255]
            } else {
                [
                    14 + (y / 20) as u8,
                    20 + (y / 16) as u8,
                    40 + (y / 10) as u8,
                    255,
                ]
            };
            rgba[(y * WIDTH + x) * 4..][..4].copy_from_slice(&color);
        }
    }
    if page == Page::River {
        for y in 118..152 {
            for x in 390..440 {
                rgba[(y * WIDTH + x) * 4..][..4].copy_from_slice(&[58, 123, 154, 255]);
            }
        }
    }
    rgba
}
