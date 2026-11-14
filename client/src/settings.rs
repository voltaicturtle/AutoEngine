//! Player settings, kept in a small JSON file next to where the client runs.
//!
//! Every field has a default, so an old or partial file still loads, and a
//! missing file is not an error. The file is rewritten whenever a setting
//! changes in the pause menu.

use std::path::Path;

use std::collections::BTreeMap;

use macroquad::input::KeyCode;
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
    /// Which key does what: action name to key name. Actions are `menu`,
    /// `stats`, `skip`, `camera_reset` and `console`; see [`key_code`] for
    /// the key names. Actions left out keep their default key.
    pub keys: BTreeMap<String, String>,
}

/// Something the player can do with a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Menu,
    Stats,
    Skip,
    CameraReset,
    Console,
}

impl Action {
    pub const ALL: [Action; 5] = [Action::Menu, Action::Stats, Action::Skip, Action::CameraReset, Action::Console];

    pub fn name(self) -> &'static str {
        match self {
            Action::Menu => "menu",
            Action::Stats => "stats",
            Action::Skip => "skip",
            Action::CameraReset => "camera_reset",
            Action::Console => "console",
        }
    }

    pub fn default_key(self) -> &'static str {
        match self {
            Action::Menu => "Escape",
            Action::Stats => "F3",
            Action::Skip => "Space",
            Action::CameraReset => "Home",
            Action::Console => "F1",
        }
    }
}

/// The key with this name: a letter (`A`), a digit (`5`), `F1` to `F12`,
/// or one of `Space`, `Escape`, `Enter`, `Tab`, `Backspace`, `Home`, `End`,
/// `Insert`, `Delete`, `PageUp`, `PageDown`, `Up`, `Down`, `Left`, `Right`,
/// `Backquote`, `Minus`, `Equal`. Case does not matter.
pub fn key_code(name: &str) -> Option<KeyCode> {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z];
    const DIGITS: [KeyCode; 10] = [Key0, Key1, Key2, Key3, Key4, Key5, Key6, Key7, Key8, Key9];
    const FUNCTION: [KeyCode; 12] = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    let lower = name.trim().to_lowercase();
    let bytes = lower.as_bytes();
    if let [c] = bytes {
        if c.is_ascii_lowercase() {
            return Some(LETTERS[(c - b'a') as usize]);
        }
        if c.is_ascii_digit() {
            return Some(DIGITS[(c - b'0') as usize]);
        }
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()) {
        return FUNCTION.get(n.wrapping_sub(1)).copied();
    }
    Some(match lower.as_str() {
        "space" => Space,
        "escape" | "esc" => Escape,
        "enter" | "return" => Enter,
        "tab" => Tab,
        "backspace" => Backspace,
        "home" => Home,
        "end" => End,
        "insert" => Insert,
        "delete" => Delete,
        "pageup" => PageUp,
        "pagedown" => PageDown,
        "up" => Up,
        "down" => Down,
        "left" => Left,
        "right" => Right,
        "backquote" | "grave" => GraveAccent,
        "minus" => Minus,
        "equal" => Equal,
        _ => return None,
    })
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
            keys: Action::ALL.iter().map(|a| (a.name().to_string(), a.default_key().to_string())).collect(),
        }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Result<Settings, String> {
        let mut settings: Settings = serde_json::from_str(text).map_err(|e| e.to_string())?;
        settings.volume = settings.volume.min(100);
        settings.window_width = settings.window_width.clamp(640, 7680);
        settings.window_height = settings.window_height.clamp(480, 4320);
        settings.camera_turn_percent = settings.camera_turn_percent.clamp(10, 400);
        for (action, key) in &settings.keys {
            if !Action::ALL.iter().any(|a| a.name() == action) {
                return Err(format!("there is no action called \"{action}\""));
            }
            if key_code(key).is_none() {
                return Err(format!("there is no key called \"{key}\""));
            }
        }
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

    /// The key bound to an action.
    pub fn key(&self, action: Action) -> KeyCode {
        self.keys
            .get(action.name())
            .and_then(|name| key_code(name))
            .or_else(|| key_code(action.default_key()))
            .expect("default keys have names")
    }

    /// The name of the key bound to an action, for showing to the player.
    pub fn key_name(&self, action: Action) -> &str {
        self.keys.get(action.name()).map_or(action.default_key(), String::as_str)
    }

    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).expect("settings are plain data")
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
        let settings = Settings::parse(r#"{"volume": 250, "shadows": false, "window_width": 10}"#).unwrap();
        assert_eq!(settings.volume, 100);
        assert!(!settings.shadows);
        assert_eq!(settings.window_width, 640);
        assert_eq!(settings.name, Settings::default().name);
    }

    #[test]
    fn keys_can_be_rebound_by_name() {
        let settings = Settings::parse(r#"{"keys": {"skip": "enter", "console": "backquote"}}"#).unwrap();
        assert_eq!(settings.key(Action::Skip), KeyCode::Enter);
        assert_eq!(settings.key(Action::Console), KeyCode::GraveAccent);
        // Actions the file leaves out keep their defaults.
        assert_eq!(settings.key(Action::Menu), KeyCode::Escape);
        assert_eq!(settings.key_name(Action::Stats), "F3");
        assert_eq!(key_code("f12"), Some(KeyCode::F12));
        assert_eq!(key_code("q"), Some(KeyCode::Q));
        assert_eq!(key_code("7"), Some(KeyCode::Key7));
        assert_eq!(key_code("f13"), None);
        assert!(Settings::parse(r#"{"keys": {"skip": "Banana"}}"#).unwrap_err().contains("Banana"));
        assert!(Settings::parse(r#"{"keys": {"jump": "Space"}}"#).unwrap_err().contains("jump"));
    }

    #[test]
    fn a_missing_file_is_fine_and_a_broken_one_is_reported() {
        let dir = std::env::temp_dir();
        let (settings, problem) = Settings::load(&dir.join("autoengine-no-such-settings.json"));
        assert_eq!((settings, problem), (Settings::default(), None));
        let broken = dir.join(format!("autoengine-broken-settings-{}.json", std::process::id()));
        std::fs::write(&broken, r#"{"volume": "loud"}"#).unwrap();
        let (settings, problem) = Settings::load(&broken);
        assert_eq!(settings, Settings::default());
        assert!(problem.is_some());
        std::fs::remove_file(&broken).unwrap();
    }
}
