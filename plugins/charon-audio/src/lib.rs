//! Prepared sounds and bounded playback commands. No game content or render loop.
use rodio::{buffer::SamplesBuffer, Decoder, DeviceSinkBuilder, Player, Source};
use std::{
    io::Cursor,
    num::NonZero,
    sync::{
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MAX_SAMPLES: usize = 16 * 1024 * 1024;
const MAX_VOICES: usize = 16;
#[derive(Clone)]
pub struct Clip(SamplesBuffer);
impl Clip {
    /// Decode WAV or Ogg Vorbis before playback. Short, resident clips only;
    /// streaming long music is a separate future facility.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("encoded audio exceeds limit".into());
        }
        let decoder = Decoder::try_from(Cursor::new(bytes.to_vec()))?;
        let channels = decoder.channels().get();
        let rate = decoder.sample_rate().get();
        let samples: Vec<_> = decoder.take(MAX_SAMPLES + 1).collect();
        Self::pcm(channels, rate, samples)
    }
    pub fn pcm(channels: u16, rate: u32, samples: Vec<f32>) -> Result<Self> {
        if !(1..=2).contains(&channels)
            || !(8_000..=192_000).contains(&rate)
            || samples.is_empty()
            || samples.len() > MAX_SAMPLES
            || !samples.len().is_multiple_of(channels as usize)
            || samples.iter().any(|s| !s.is_finite() || s.abs() > 1.)
        {
            return Err("invalid audio channels, rate, length or samples".into());
        }
        Ok(Self(SamplesBuffer::new(
            NonZero::new(channels).unwrap(),
            NonZero::new(rate).unwrap(),
            samples,
        )))
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    master: f32,
    music: f32,
    effects: f32,
}
impl Levels {
    pub fn new(master: f32, music: f32, effects: f32) -> Result<Self> {
        if [master, music, effects]
            .iter()
            .any(|v| !v.is_finite() || !(0. ..=1.).contains(v))
        {
            return Err("audio volume must be finite and between zero and one".into());
        }
        Ok(Self {
            master,
            music,
            effects,
        })
    }
    fn gains(self) -> [f32; 2] {
        [self.master * self.music, self.master * self.effects]
    }
}
impl Default for Levels {
    fn default() -> Self {
        Self {
            master: 0.5,
            music: 0.5,
            effects: 0.7,
        }
    }
}
pub enum Command {
    Music(Clip),
    Effect(Clip),
    Levels(Levels),
    PauseMusic(bool),
    StopMusic,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceState {
    Starting,
    Ready,
    Unavailable(String),
}
#[derive(Clone, Debug)]
pub struct Status {
    pub device: DeviceState,
    pub music: bool,
    pub paused: bool,
    pub effects: usize,
    pub rejected_effects: u64,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            device: DeviceState::Starting,
            music: false,
            paused: false,
            effects: 0,
            rejected_effects: 0,
        }
    }
}
pub struct Audio {
    sender: Option<SyncSender<Command>>,
    status: Arc<Mutex<Status>>,
    thread: Option<JoinHandle<()>>,
}
impl Audio {
    /// Opening the default OS device occurs on a dedicated worker. Failure is
    /// visible through status; applications may continue with explicit silence.
    pub fn start() -> std::io::Result<Self> {
        let (sender, input) = mpsc::sync_channel(32);
        let status = Arc::new(Mutex::new(Status::default()));
        let output = status.clone();
        let thread = thread::Builder::new()
            .name("charon-audio".into())
            .spawn(move || {
                let error_output = output.clone();
                let stream = DeviceSinkBuilder::from_default_device().and_then(|builder| {
                    builder
                        .with_error_callback(move |error| {
                            let mut status = error_output.lock().unwrap();
                            status.device = DeviceState::Unavailable(error.to_string());
                            status.music = false;
                            status.effects = 0;
                        })
                        .open_stream()
                });
                let mut stream = match stream {
                    Ok(stream) => stream,
                    Err(error) => {
                        output.lock().unwrap().device = DeviceState::Unavailable(error.to_string());
                        return;
                    }
                };
                stream.log_on_drop(false);
                let mut levels = Levels::default();
                let mut music: Option<Player> = None;
                let mut effects: Vec<Player> = Vec::new();
                output.lock().unwrap().device = DeviceState::Ready;
                loop {
                    effects.retain(|p| !p.empty());
                    match input.recv_timeout(Duration::from_millis(20)) {
                        Ok(Command::Music(clip)) => {
                            if let Some(old) = music.take() {
                                old.stop();
                            }
                            let player = Player::connect_new(stream.mixer());
                            player.set_volume(levels.gains()[0]);
                            player.append(clip.0.repeat_infinite());
                            music = Some(player);
                        }
                        Ok(Command::Effect(clip)) => {
                            if effects.len() == MAX_VOICES {
                                output.lock().unwrap().rejected_effects += 1;
                            } else {
                                let player = Player::connect_new(stream.mixer());
                                player.set_volume(levels.gains()[1]);
                                player.append(clip.0);
                                effects.push(player);
                            }
                        }
                        Ok(Command::Levels(new)) => {
                            levels = new;
                            if let Some(player) = &music {
                                player.set_volume(levels.gains()[0]);
                            }
                            for player in &effects {
                                player.set_volume(levels.gains()[1]);
                            }
                        }
                        Ok(Command::PauseMusic(pause)) => {
                            if let Some(player) = &music {
                                if pause {
                                    player.pause();
                                } else {
                                    player.play();
                                }
                            }
                        }
                        Ok(Command::StopMusic) => {
                            if let Some(player) = music.take() {
                                player.stop();
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    let mut state = output.lock().unwrap();
                    if matches!(state.device, DeviceState::Unavailable(_)) {
                        break;
                    }
                    state.music = music.is_some();
                    state.paused = music.as_ref().is_some_and(Player::is_paused);
                    state.effects = effects.len();
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            status,
            thread: Some(thread),
        })
    }
    pub fn submit(&self, command: Command) -> Result<()> {
        if let DeviceState::Unavailable(error) = self.status().device {
            return Err(error.into());
        }
        self.sender
            .as_ref()
            .expect("live audio")
            .try_send(command)
            .map_err(|e| e.to_string().into())
    }
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
}
impl Drop for Audio {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prepared_clips_validate_decode_and_apply_bus_gain() {
        assert!(Clip::pcm(2, 48_000, vec![0.]).is_err());
        assert!(Clip::pcm(1, 48_000, vec![f32::NAN]).is_err());
        assert!(Levels::new(1.1, 1., 1.).is_err());
        assert!(Clip::decode(b"broken").is_err());
        // A tiny game-independent PCM16 WAV fixture.
        let mut wav = b"RIFF".to_vec();
        wav.extend(40_u32.to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16_u32.to_le_bytes());
        wav.extend(1_u16.to_le_bytes());
        wav.extend(1_u16.to_le_bytes());
        wav.extend(8000_u32.to_le_bytes());
        wav.extend(16000_u32.to_le_bytes());
        wav.extend(2_u16.to_le_bytes());
        wav.extend(16_u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(4_u32.to_le_bytes());
        wav.extend(16384_i16.to_le_bytes());
        wav.extend((-16384_i16).to_le_bytes());
        let clip = Clip::decode(&wav).unwrap();
        let gain = Levels::new(0.5, 0.4, 0.8).unwrap().gains();
        let music: Vec<_> = clip.0.clone().amplify(gain[0]).collect();
        assert!((music[0] - 0.1).abs() < 0.0001 && (music[1] + 0.1).abs() < 0.0001);
        assert!(clip
            .0
            .amplify(Levels::new(0., 1., 1.).unwrap().gains()[1])
            .all(|s| s == 0.));
    }
}
