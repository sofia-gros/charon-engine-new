//! Game-owned composition, checkpoint schema and sound content.
use crate::story::{self, Page};
use charon_audio::{Audio, Clip, Command as Sound, DeviceState, Levels};
use charon_dialogue::{Dialogue, Frame as DialogueFrame};
use charon_runtime::Simulation;
use charon_save::{Outcome, Store};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

pub enum Command {
    Dialogue(charon_dialogue::Command),
    Save,
    Load,
    Volume(i8),
    Mute,
    PauseMusic,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    story: String,
    page: Page,
    volume: u8,
    muted: bool,
    music_paused: bool,
}
impl Checkpoint {
    fn decode(bytes: &[u8], story: &str) -> Result<Self, String> {
        let value: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if value.version != 1 || value.story != story || value.volume > 100 {
            return Err("incompatible checkpoint version, story or volume".into());
        }
        Ok(value)
    }
}
pub struct Frame {
    pub dialogue: DialogueFrame<Page>,
    pub volume: u8,
    pub muted: bool,
    pub music_paused: bool,
    pub audio: DeviceState,
    pub notice: Arc<str>,
    pub saves: u64,
    pub loads: u64,
}
pub struct Game {
    dialogue: Dialogue<Page>,
    store: Store,
    loading: bool,
    audio: Option<Audio>,
    click: Clip,
    volume: u8,
    muted: bool,
    music_paused: bool,
    notice: Arc<str>,
    saves: u64,
    loads: u64,
    story: String,
}
impl Game {
    pub fn new(path: PathBuf, audio: bool) -> charon_render::Result<Self> {
        let audio = audio.then(Audio::start).transpose()?;
        let click = tone(&[660., 880.], 0.12)?;
        let game = Self {
            dialogue: story::build()?,
            store: Store::new(path)?,
            loading: false,
            audio,
            click,
            volume: 50,
            muted: false,
            music_paused: false,
            notice: "F5：保存　F9：読込　M：消音　＋ / －：音量　P：BGM停止・再開".into(),
            saves: 0,
            loads: 0,
            story: story::fingerprint(),
        };
        game.sound(Sound::Music(tone(&[261.63, 329.63, 392., 329.63], 2.0)?));
        game.apply_levels();
        Ok(game)
    }
    fn sound(&self, command: Sound) {
        if let Some(audio) = &self.audio {
            // Unavailable devices are exposed in every published frame. Queue
            // pressure is exceptional and reported instead of blocking a tick.
            if let Err(error) = audio.submit(command) {
                eprintln!("Audio command: {error}");
            }
        }
    }
    fn apply_levels(&self) {
        self.sound(Sound::Levels(
            Levels::new(
                if self.muted {
                    0.
                } else {
                    self.volume as f32 / 100.
                },
                0.25,
                0.7,
            )
            .unwrap(),
        ));
        self.sound(Sound::PauseMusic(self.music_paused));
    }
    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            version: 1,
            story: self.story.clone(),
            page: self.dialogue.current(),
            volume: self.volume,
            muted: self.muted,
            music_paused: self.music_paused,
        }
    }
    fn fail(&mut self, error: impl std::fmt::Display) {
        eprintln!("Save/load: {error}");
        self.notice = "保存・読込に失敗しました。現在の進行は維持されています。".into();
    }
    fn poll(&mut self) {
        if let Some(result) = self.store.poll() {
            self.loading = false;
            match result {
                Ok(Outcome::Saved) => {
                    self.saves += 1;
                    self.notice = "保存しました。F9 でこの場所と音量を復元できます。".into();
                }
                Ok(Outcome::Loaded(bytes)) => {
                    let result = Checkpoint::decode(&bytes, &self.story).and_then(|state| {
                        // Validate all fields before mutating any game state.
                        self.dialogue.restore(state.page).map_err(str::to_owned)?;
                        self.volume = state.volume;
                        self.muted = state.muted;
                        self.music_paused = state.music_paused;
                        Ok(())
                    });
                    match result {
                        Ok(()) => {
                            self.loads += 1;
                            self.notice = "読込が完了しました。進行と音量を復元しました。".into();
                            self.apply_levels();
                        }
                        Err(error) => self.fail(error),
                    }
                }
                Err(error) => self.fail(error),
            }
        }
    }
}
impl Simulation for Game {
    type Command = Command;
    type Frame = Frame;
    fn step(&mut self, commands: Vec<Command>) -> Frame {
        self.poll();
        for command in commands {
            if self.loading {
                self.notice = "読込中です。完了するまでお待ちください。".into();
                continue;
            }
            match command {
                Command::Dialogue(command) => {
                    let before = self.dialogue.current();
                    self.dialogue.step(vec![command]);
                    if before != self.dialogue.current() {
                        self.sound(Sound::Effect(self.click.clone()));
                    }
                }
                Command::Volume(delta) => {
                    self.volume = (self.volume as i16 + delta as i16).clamp(0, 100) as u8;
                    self.apply_levels();
                }
                Command::Mute => {
                    self.muted = !self.muted;
                    self.apply_levels();
                }
                Command::PauseMusic => {
                    self.music_paused = !self.music_paused;
                    self.apply_levels();
                }
                Command::Save => {
                    let bytes =
                        serde_json::to_vec(&self.checkpoint()).expect("checkpoint serialization");
                    match self.store.save(bytes) {
                        Ok(()) => self.notice = "保存中です。".into(),
                        Err(e) => self.fail(e),
                    }
                }
                Command::Load => match self.store.load() {
                    Ok(()) => {
                        self.loading = true;
                        self.notice = "読込中です。".into();
                    }
                    Err(e) => self.fail(e),
                },
            }
        }
        Frame {
            dialogue: self.dialogue.frame(),
            volume: self.volume,
            muted: self.muted,
            music_paused: self.music_paused,
            audio: self
                .audio
                .as_ref()
                .map_or(DeviceState::Unavailable("disabled".into()), |a| {
                    a.status().device
                }),
            notice: self.notice.clone(),
            saves: self.saves,
            loads: self.loads,
        }
    }
}
/// Original sample content, not a dependency of the audio plugin. A gentle
/// enveloped sequence with quiet boundaries; no external media license needed.
fn tone(notes: &[f32], seconds: f32) -> charon_audio::Result<Clip> {
    let rate = 48_000;
    let count = (seconds * rate as f32) as usize;
    let segment = count / notes.len();
    let samples = (0..count)
        .map(|i| {
            let local = (i % segment) as f32 / segment as f32;
            let fade = (local * 30.).min(1.) * ((1. - local) * 10.).min(1.);
            (std::f32::consts::TAU
                * notes[(i / segment).min(notes.len() - 1)]
                * (i % segment) as f32
                / rate as f32)
                .sin()
                * 0.18
                * fade
        })
        .collect();
    Clip::pcm(1, rate, samples)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant, SystemTime};
    fn wait(game: &mut Game, done: impl Fn(&Frame) -> bool) -> Frame {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let frame = game.step(vec![]);
            if done(&frame) {
                return frame;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn ordered_checkpoint_roundtrip_and_corrupt_load_preserve_live_state() {
        let path = std::env::temp_dir().join(format!(
            "charon-game-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut game = Game::new(path.clone(), false).unwrap();
        game.step(vec![
            Command::Dialogue(charon_dialogue::Command::Advance),
            Command::Volume(20),
            Command::Save,
            Command::Dialogue(charon_dialogue::Command::Choose(1)),
            Command::Mute,
        ]);
        let frame = wait(&mut game, |f| f.saves == 1);
        assert_eq!(frame.dialogue.page, Page::Hill);
        game.step(vec![Command::Load]);
        let frame = wait(&mut game, |f| f.loads == 1);
        assert_eq!(
            (frame.dialogue.page, frame.volume, frame.muted),
            (Page::Choice, 70, false)
        );
        let original = std::fs::read(&path).unwrap();
        for bad in [b"truncated".to_vec(), original.clone()] {
            let bad = if bad == original {
                let mut v: serde_json::Value = serde_json::from_slice(&bad).unwrap();
                v["volume"] = 255.into();
                serde_json::to_vec(&v).unwrap()
            } else {
                bad
            };
            std::fs::write(&path, bad).unwrap();
            game.step(vec![
                Command::Dialogue(charon_dialogue::Command::Choose(0)),
                Command::Load,
            ]);
            let frame = wait(&mut game, |f| f.notice.contains("失敗"));
            assert_eq!(
                (frame.dialogue.page, frame.volume, frame.loads),
                (Page::River, 70, 1)
            );
        }
        std::fs::write(&path, original).unwrap();
        drop(game);
        let mut reopened = Game::new(path.clone(), false).unwrap();
        reopened.step(vec![Command::Load]);
        assert_eq!(
            wait(&mut reopened, |f| f.loads == 1).dialogue.page,
            Page::Choice
        );
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn unknown_versions_story_keys_and_fields_fail_validation() {
        let state = Checkpoint {
            version: 1,
            story: story::fingerprint(),
            page: Page::Intro,
            volume: 50,
            muted: false,
            music_paused: false,
        };
        let good = serde_json::to_value(state).unwrap();
        for (field, value) in [
            ("version", serde_json::json!(2)),
            ("story", serde_json::json!("other")),
            ("page", serde_json::json!("removed")),
            ("extra", serde_json::json!(1)),
        ] {
            let mut bad = good.clone();
            bad[field] = value;
            assert!(
                Checkpoint::decode(&serde_json::to_vec(&bad).unwrap(), &story::fingerprint())
                    .is_err()
            );
        }
    }
}
