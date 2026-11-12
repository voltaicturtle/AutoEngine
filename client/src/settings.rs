//! Player settings, kept in a small RON file next to where the client runs.
//!
//! Every field has a default, so an old or partial file still loads, and a
//! missing file is not an error. The file is rewritten whenever a setting
//! changes in the pause menu.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Name to join with, unless `--name` says otherwise.
    pub name: String,
    /// Server to connect to, unless `--addr` says otherwise.
    pub addr: String,
    pub window_width: i32,
    pub window_height: i32,
    pub fullscreen: bool,
    /// Sound volume, 0 to 100.
    pub volume: u8,
    pub particles: bool,
    pub shadows: bool,
    /// Play models' animation clips.
    pub animations: bool,
    /// Show the frame-rate and scene statistics overlay.
    pub show_stats: bool,
    /// How fast a right-drag turns the camera, as a percentage.
    pub camera_turn_percent: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            name: "Player".into(),
            addr: "127.0.0.1:7777".into(),
            window_width: 1280,
            window_height: 800,
            fullscreen: false,
            volume: 70,
            particles: true,
            shadows: true,
            animations: true,
            show_stats: false,
            camera_turn_percent: 100,
        }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Result<Settings, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut settings: Settings = options.from_str(text).map_err(|e| e.to_string())?;
        settings.volume = settings.volume.min(100);
        settings.window_width = settings.window_width.clamp(640, 7680);
        settings.window_height = settings.window_height.clamp(480, 4320);
        settings.camera_turn_percent = settings.camera_turn_percent.clamp(10, 400);
        Ok(settings)
    }

    /// Loads the file; a missing file gives the defaults. A broken file
    /// also gives the defaults, along with a message saying why.
    pub fn load(path: &Path) -> (Settings, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match Settings::parse(&text) {
                Ok(settings) => (settings, None),
                Err(e) => (Settings::default(), Some(format!("{}: {e}; using default settings", path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
            Err(e) => (Settings::default(), Some(format!("{}: {e}; using default settings", path.display()))),
        }
    }

    pub fn to_text(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).expect("settings are plain data")
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_text() + "\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_round_trip() {
        let settings = Settings { name: "Ana".into(), volume: 35, particles: false, ..Settings::default() };
        assert_eq!(Settings::parse(&settings.to_text()).unwrap(), settings);
    }

    #[test]
    fn missing_fields_take_defaults_and_values_are_clamped() {
        let settings = Settings::parse("(volume: 250, shadows: false, window_width: 10)").unwrap();
        assert_eq!(settings.volume, 100);
        assert!(!settings.shadows);
        assert_eq!(settings.window_width, 640);
        assert_eq!(settings.name, Settings::default().name);
    }

    #[test]
    fn a_missing_file_is_fine_and_a_broken_one_is_reported() {
        let dir = std::env::temp_dir();
        let (settings, problem) = Settings::load(&dir.join("autoengine-no-such-settings.ron"));
        assert_eq!((settings, problem), (Settings::default(), None));
        let broken = dir.join(format!("autoengine-broken-settings-{}.ron", std::process::id()));
        std::fs::write(&broken, "(volume: loud)").unwrap();
        let (settings, problem) = Settings::load(&broken);
        assert_eq!(settings, Settings::default());
        assert!(problem.is_some());
        std::fs::remove_file(&broken).unwrap();
    }
}
