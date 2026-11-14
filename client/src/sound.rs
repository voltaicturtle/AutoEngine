//! Sound effects.
//!
//! Each effect is read from `sounds/<name>.wav` or `sounds/<name>.ogg` in
//! the assets folder (`hit`, `cast`, `heal`, `death`, `buy`, `victory`,
//! `defeat`). An effect with no file is synthesised instead, so the game
//! makes noise before any audio has been recorded. Playback needs the `audio` cargo feature, which is off by default
//! because it needs the system's audio library to build and a sound device
//! to run. Without it every call here does nothing.

#![cfg_attr(not(feature = "audio"), allow(dead_code))]

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sfx {
    Hit,
    Cast,
    Heal,
    Death,
    Buy,
    Victory,
    Defeat,
}

impl Sfx {
    /// The file name an artist uses to replace this effect.
    pub fn name(self) -> &'static str {
        match self {
            Sfx::Hit => "hit",
            Sfx::Cast => "cast",
            Sfx::Heal => "heal",
            Sfx::Death => "death",
            Sfx::Buy => "buy",
            Sfx::Victory => "victory",
            Sfx::Defeat => "defeat",
        }
    }
}

/// The recording for an effect, if the assets folder has one.
pub fn file_bytes(assets: &std::path::Path, sfx: Sfx) -> Option<Vec<u8>> {
    ["wav", "ogg"].iter().find_map(|ext| std::fs::read(assets.join("sounds").join(format!("{}.{ext}", sfx.name()))).ok())
}

pub const ALL: [Sfx; 7] = [Sfx::Hit, Sfx::Cast, Sfx::Heal, Sfx::Death, Sfx::Buy, Sfx::Victory, Sfx::Defeat];

const RATE: u32 = 22_050;

/// The waveform of an effect as samples from -1 to 1.
pub fn synth(sfx: Sfx) -> Vec<f32> {
    let tone = |freq: f32, t: f32| (std::f32::consts::TAU * freq * t).sin();
    // Cheap repeatable noise.
    let noise = |i: usize| ((i as u32).wrapping_mul(2_654_435_761) >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
    let (seconds, wave): (f32, Box<dyn Fn(usize, f32) -> f32>) = match sfx {
        Sfx::Hit => (0.12, Box::new(move |i, t| (noise(i) * 0.6 + tone(140.0 - 400.0 * t, t) * 0.7) * (1.0 - t / 0.12).powi(2))),
        Sfx::Cast => (0.45, Box::new(move |_, t| (tone(520.0 + 900.0 * t, t) * 0.5 + tone(780.0 + 1350.0 * t, t) * 0.25) * (1.0 - t / 0.45))),
        Sfx::Heal => (0.35, Box::new(move |_, t| tone(if t < 0.15 { 660.0 } else { 880.0 }, t) * 0.5 * (1.0 - t / 0.35))),
        Sfx::Death => (0.40, Box::new(move |i, t| (tone(220.0 - 380.0 * t, t) * 0.6 + noise(i) * 0.25) * (1.0 - t / 0.40))),
        Sfx::Buy => (0.09, Box::new(move |_, t| tone(1200.0, t) * 0.4 * (1.0 - t / 0.09))),
        Sfx::Victory => (0.9, Box::new(move |_, t| tone([523.0, 659.0, 784.0][((t / 0.3) as usize).min(2)], t) * 0.5 * (1.0 - t / 0.9).sqrt())),
        Sfx::Defeat => (0.9, Box::new(move |_, t| tone([392.0, 330.0, 262.0][((t / 0.3) as usize).min(2)], t) * 0.5 * (1.0 - t / 0.9).sqrt())),
    };
    let count = (seconds * RATE as f32) as usize;
    (0..count).map(|i| wave(i, i as f32 / RATE as f32).clamp(-1.0, 1.0)).collect()
}

/// The effect as a 16-bit mono WAV file.
pub fn wav(sfx: Sfx) -> Vec<u8> {
    let samples = synth(sfx);
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend(b"RIFF");
    out.extend((36 + data_len).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes()); // PCM
    out.extend(1u16.to_le_bytes()); // mono
    out.extend(RATE.to_le_bytes());
    out.extend((RATE * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(data_len.to_le_bytes());
    for s in samples {
        out.extend(((s * i16::MAX as f32) as i16).to_le_bytes());
    }
    out
}

#[cfg(feature = "audio")]
mod backend {
    use std::collections::HashMap;

    use macroquad::audio::{PlaySoundParams, Sound, load_sound_from_bytes, play_sound};

    use super::{ALL, Sfx, file_bytes, wav};

    #[derive(Default)]
    pub struct Sounds {
        loaded: HashMap<Sfx, Sound>,
        /// When each effect last played, to keep a flurry of hits from
        /// turning into noise.
        last: HashMap<Sfx, f64>,
    }

    impl Sounds {
        pub async fn load(assets: &std::path::Path) -> Sounds {
            let mut sounds = Sounds::default();
            for sfx in ALL {
                let bytes = file_bytes(assets, sfx).unwrap_or_else(|| wav(sfx));
                match load_sound_from_bytes(&bytes).await {
                    Ok(sound) => {
                        sounds.loaded.insert(sfx, sound);
                    }
                    Err(e) => eprintln!("sound {sfx:?}: {e}"),
                }
            }
            sounds
        }

        pub fn play(&mut self, sfx: Sfx, volume_percent: u8) {
            let now = macroquad::time::get_time();
            if volume_percent == 0 || self.last.get(&sfx).is_some_and(|t| now - t < 0.05) {
                return;
            }
            if let Some(sound) = self.loaded.get(&sfx) {
                self.last.insert(sfx, now);
                play_sound(sound, PlaySoundParams { looped: false, volume: volume_percent as f32 / 100.0 });
            }
        }

        pub const AVAILABLE: bool = true;
    }
}

#[cfg(not(feature = "audio"))]
mod backend {
    use super::Sfx;

    #[derive(Default)]
    pub struct Sounds;

    impl Sounds {
        pub async fn load(_assets: &std::path::Path) -> Sounds {
            Sounds
        }

        pub fn play(&mut self, _sfx: Sfx, _volume_percent: u8) {}

        pub const AVAILABLE: bool = false;
    }
}

pub use backend::Sounds;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_in_the_assets_folder_replaces_the_synthesised_effect() {
        let assets = std::env::temp_dir().join(format!("autoengine-sounds-{}", std::process::id()));
        std::fs::create_dir_all(assets.join("sounds")).unwrap();
        assert_eq!(file_bytes(&assets, Sfx::Hit), None);
        std::fs::write(assets.join("sounds/hit.ogg"), b"ogg").unwrap();
        assert_eq!(file_bytes(&assets, Sfx::Hit).as_deref(), Some(&b"ogg"[..]));
        std::fs::write(assets.join("sounds/hit.wav"), b"wav").unwrap();
        assert_eq!(file_bytes(&assets, Sfx::Hit).as_deref(), Some(&b"wav"[..]), "wav wins");
        assert_eq!(file_bytes(&assets, Sfx::Cast), None);
        std::fs::remove_dir_all(&assets).unwrap();
    }

    #[test]
    fn every_effect_is_a_valid_wav() {
        for sfx in ALL {
            let samples = synth(sfx);
            assert!(samples.len() > 1_000, "{sfx:?} is audible");
            assert!(samples.iter().all(|s| (-1.0..=1.0).contains(s)));
            assert!(samples.iter().any(|s| s.abs() > 0.1));
            let file = wav(sfx);
            assert_eq!(&file[..4], b"RIFF");
            assert_eq!(&file[8..12], b"WAVE");
            assert_eq!(file.len(), 44 + samples.len() * 2);
            assert_eq!(u32::from_le_bytes(file[4..8].try_into().unwrap()) as usize, file.len() - 8);
        }
    }
}
