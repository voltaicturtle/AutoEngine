//! The look of the client, in code.
//!
//! [`Theme::default`] sets the interface colours, the board colours, the
//! light, the font, and what each particle effect looks like. To restyle the
//! client, change the values there (or build a `Theme` and apply it the way
//! `main.rs` applies the default).

use std::cell::{Cell, RefCell};
use std::path::Path;

use macroquad::prelude::*;

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
    fn the_default_palette_is_the_one_in_use() {
        assert_eq!(ui(), Theme::default().palette);
        let mut dark = Theme::default().palette;
        dark.background = BLACK;
        set_palette(dark);
        assert_eq!(ui().background, BLACK);
    }
}
