//! The look of the client, as data.
//!
//! `theme.ron` in the assets folder sets the interface colours, the board
//! colours, the light, the font, and what each particle effect looks like.
//! Every field is optional, and the file is reloaded while the client runs
//! whenever it changes, so an artist can tune the look with the game open.

use std::cell::{Cell, RefCell};
use std::path::Path;

use macroquad::prelude::*;
use serde::Deserialize;

use crate::fx::Burst;

/// The interface colours in use. Cheap to copy; read it with [`ui`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub background: Color,
    pub panel: Color,
    pub panel_light: Color,
    pub text: Color,
    pub muted: Color,
    pub gold: Color,
    pub good: Color,
    pub bad: Color,
    pub accent: Color,
    /// Rings under the viewer's units and under the opponent's.
    pub friend: Color,
    pub enemy: Color,
    pub shadow: Color,
    pub tile: Color,
    /// The half of the board the viewer may place units on.
    pub tile_own: Color,
    pub tile_highlight: Color,
    pub bench: Color,
}

/// Particle effects by what sets them off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effects {
    pub hit_physical: Burst,
    pub hit_magic: Burst,
    pub hit_true: Burst,
    /// Critical hits throw this many times the particles, in percent.
    pub crit_percent: u32,
    pub heal: Burst,
    pub cast: Burst,
    pub death: Burst,
    pub bolt: Color,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    /// Points from the scene towards the light.
    pub direction: Vec3,
    pub ambient: f32,
    pub diffuse: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub palette: Palette,
    pub effects: Effects,
    pub light: Light,
    /// A TrueType font file, relative to the assets folder.
    pub font: Option<String>,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
}

impl Default for Theme {
    fn default() -> Self {
        let burst = |count, color, speed, life, size, gravity| Burst { count, color, speed, life, size, gravity };
        Theme {
            palette: Palette {
                background: rgb(18, 20, 28),
                panel: rgb(31, 36, 48),
                panel_light: rgb(46, 54, 71),
                text: rgb(230, 235, 245),
                muted: rgb(148, 158, 179),
                gold: rgb(250, 204, 64),
                good: rgb(89, 209, 115),
                bad: rgb(235, 84, 84),
                accent: rgb(89, 158, 250),
                friend: rgb(89, 158, 250),
                enemy: rgb(235, 84, 84),
                shadow: Color::new(0.0, 0.0, 0.0, 0.38),
                tile: rgb(48, 56, 74),
                tile_own: rgb(61, 79, 110),
                tile_highlight: rgb(115, 158, 217),
                bench: rgb(41, 46, 61),
            },
            effects: Effects {
                hit_physical: burst(7, rgb(255, 153, 64), 2.4, 0.45, 0.13, 6.0),
                hit_magic: burst(7, rgb(128, 166, 255), 2.4, 0.45, 0.13, 6.0),
                hit_true: burst(7, WHITE, 2.4, 0.45, 0.13, 6.0),
                crit_percent: 230,
                heal: burst(9, rgb(115, 255, 140), 0.9, 0.9, 0.12, -2.5),
                cast: burst(22, rgb(128, 199, 255), 2.6, 0.6, 0.14, 0.0),
                death: burst(26, Color::new(0.75, 0.75, 0.80, 0.9), 1.5, 0.9, 0.2, -0.8),
                bolt: rgb(255, 217, 115),
            },
            light: Light { direction: vec3(0.35, 0.85, 0.40), ambient: 0.50, diffuse: 0.62 },
            font: None,
        }
    }
}

/// Parses `#rrggbb` or `#rrggbbaa`.
pub fn parse_color(text: &str) -> Result<Color, String> {
    let hex = text.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or(""), 16).map_err(|_| format!("\"{text}\" is not a colour like #1a2b3c"));
    match hex.len() {
        6 => Ok(Color::from_rgba(byte(0)?, byte(2)?, byte(4)?, 255)),
        8 => Ok(Color::from_rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => Err(format!("\"{text}\" is not a colour like #1a2b3c")),
    }
}

// The file format: everything optional, colours as hex strings.

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ThemeFile {
    colors: std::collections::BTreeMap<String, String>,
    effects: std::collections::BTreeMap<String, BurstFile>,
    crit_percent: Option<u32>,
    bolt: Option<String>,
    light: Option<LightFile>,
    font: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct BurstFile {
    count: Option<usize>,
    color: Option<String>,
    /// Hundredths of a hex radius per second, and so on: whole numbers
    /// keep the file free of decimal fiddling.
    speed_percent: Option<u32>,
    life_ms: Option<u32>,
    size_percent: Option<u32>,
    /// Negative floats upwards.
    gravity_percent: Option<i32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LightFile {
    direction: (i32, i32, i32),
    ambient_percent: u32,
    diffuse_percent: u32,
}

impl Theme {
    pub fn parse(text: &str) -> Result<Theme, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let file: ThemeFile = options.from_str(text).map_err(|e| e.to_string())?;
        let mut theme = Theme::default();
        for (name, value) in &file.colors {
            let color = parse_color(value)?;
            let p = &mut theme.palette;
            let slot = match name.as_str() {
                "background" => &mut p.background,
                "panel" => &mut p.panel,
                "panel_light" => &mut p.panel_light,
                "text" => &mut p.text,
                "muted" => &mut p.muted,
                "gold" => &mut p.gold,
                "good" => &mut p.good,
                "bad" => &mut p.bad,
                "accent" => &mut p.accent,
                "friend" => &mut p.friend,
                "enemy" => &mut p.enemy,
                "shadow" => &mut p.shadow,
                "tile" => &mut p.tile,
                "tile_own" => &mut p.tile_own,
                "tile_highlight" => &mut p.tile_highlight,
                "bench" => &mut p.bench,
                other => return Err(format!("there is no colour called \"{other}\"")),
            };
            *slot = color;
        }
        for (name, file) in &file.effects {
            let e = &mut theme.effects;
            let burst = match name.as_str() {
                "hit_physical" => &mut e.hit_physical,
                "hit_magic" => &mut e.hit_magic,
                "hit_true" => &mut e.hit_true,
                "heal" => &mut e.heal,
                "cast" => &mut e.cast,
                "death" => &mut e.death,
                other => return Err(format!("there is no effect called \"{other}\"")),
            };
            if let Some(count) = file.count {
                burst.count = count.min(500);
            }
            if let Some(color) = &file.color {
                burst.color = parse_color(color)?;
            }
            if let Some(v) = file.speed_percent {
                burst.speed = v as f32 / 100.0;
            }
            if let Some(v) = file.life_ms {
                burst.life = (v as f32 / 1000.0).max(0.01);
            }
            if let Some(v) = file.size_percent {
                burst.size = v as f32 / 100.0;
            }
            if let Some(v) = file.gravity_percent {
                burst.gravity = v as f32 / 100.0;
            }
        }
        if let Some(v) = file.crit_percent {
            theme.effects.crit_percent = v.clamp(100, 1000);
        }
        if let Some(color) = &file.bolt {
            theme.effects.bolt = parse_color(color)?;
        }
        if let Some(light) = &file.light {
            let (x, y, z) = light.direction;
            let direction = vec3(x as f32, y as f32, z as f32);
            if direction.length() < 0.5 {
                return Err("the light needs a direction".into());
            }
            theme.light = Light {
                direction,
                ambient: light.ambient_percent.min(200) as f32 / 100.0,
                diffuse: light.diffuse_percent.min(200) as f32 / 100.0,
            };
        }
        theme.font = file.font;
        Ok(theme)
    }

    /// Reads `theme.ron` from the assets folder; no file means the defaults.
    pub fn load(assets: &Path) -> Result<Theme, String> {
        let path = assets.join("theme.ron");
        match std::fs::read_to_string(&path) {
            Ok(text) => Theme::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Theme::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

thread_local! {
    static PALETTE: Cell<Palette> = Cell::new(Theme::default().palette);
    static FONT: RefCell<Option<Font>> = const { RefCell::new(None) };
}

/// The interface colours currently in use.
pub fn ui() -> Palette {
    PALETTE.with(|p| p.get())
}

pub fn set_palette(palette: Palette) {
    PALETTE.with(|p| p.set(palette));
}

pub fn set_font(font: Option<Font>) {
    FONT.with(|f| *f.borrow_mut() = font);
}

/// Draws text in the theme's font, with its baseline at `y`.
pub fn text(text: &str, x: f32, y: f32, size: f32, color: Color) {
    FONT.with(|f| {
        draw_text_ex(text, x, y, TextParams { font: f.borrow().as_ref(), font_size: size as u16, color, ..Default::default() });
    });
}

/// Width of text in the theme's font.
pub fn text_width(text: &str, size: f32) -> f32 {
    FONT.with(|f| measure_text(text, f.borrow().as_ref(), size as u16, 1.0).width)
}

/// When a file was last changed, for noticing edits.
pub fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_parse_from_hex() {
        assert_eq!(parse_color("#ff0000").unwrap(), Color::from_rgba(255, 0, 0, 255));
        assert_eq!(parse_color("00ff0080").unwrap(), Color::from_rgba(0, 255, 0, 128));
        assert!(parse_color("#12345").is_err() && parse_color("#gggggg").is_err());
    }

    #[test]
    fn a_theme_overrides_only_what_it_names() {
        let theme = Theme::parse(
            r##"(
                colors: {"background": "#000000", "friend": "#00ff00"},
                effects: {"heal": (count: 40, color: "#ffffff", gravity_percent: -500)},
                light: (direction: (1, 2, 0), ambient_percent: 30, diffuse_percent: 90),
                font: "fonts/title.ttf",
            )"##,
        )
        .unwrap();
        let default = Theme::default();
        assert_eq!(theme.palette.background, BLACK);
        assert_eq!(theme.palette.friend, Color::from_rgba(0, 255, 0, 255));
        assert_eq!(theme.palette.text, default.palette.text);
        assert_eq!(theme.effects.heal.count, 40);
        assert_eq!(theme.effects.heal.gravity, -5.0);
        assert_eq!(theme.effects.heal.speed, default.effects.heal.speed);
        assert_eq!(theme.effects.cast, default.effects.cast);
        assert_eq!(theme.light.ambient, 0.3);
        assert_eq!(theme.font.as_deref(), Some("fonts/title.ttf"));
        assert_eq!(Theme::parse("()").unwrap(), default);
    }

    #[test]
    fn theme_mistakes_are_named() {
        assert!(Theme::parse(r##"(colors: {"backgroud": "#000000"})"##).unwrap_err().contains("backgroud"));
        assert!(Theme::parse(r##"(effects: {"explode": ()})"##).unwrap_err().contains("explode"));
        assert!(Theme::parse(r##"(colors: {"text": "red"})"##).unwrap_err().contains("red"));
        assert!(Theme::parse("(colours: {})").is_err());
    }

    #[test]
    fn the_sample_theme_loads() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../content");
        Theme::load(&assets).unwrap();
        assert_eq!(Theme::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap(), Theme::default());
    }
}
