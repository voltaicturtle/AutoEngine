//! Layout and small drawing helpers.

use autoengine::hex::Hex;
use macroquad::prelude::*;

use crate::scene::{SQRT3, View};
use crate::App;
use crate::theme::{self, ui};

/// Where everything goes for the current window size.
///
/// The board and bench live in the 3D scene; the methods here translate
/// between board coordinates, world space and the window. The shop and the
/// rest of the interface are flat and laid out around the 3D picture.
pub struct Layout {
    pub left_w: f32,
    pub right_w: f32,
    pub cols: i32,
    pub rows: i32,
    pub board_mid_x: f32,
    pub view: View,
    /// World position of the middle of hex (0, 0) before centring.
    origin: Vec3,
    pub bench_slots: usize,
    shop_slots: usize,
}

/// Height of the units' feet above the plane the tiles are drawn around.
pub const TILE_TOP: f32 = 0.0;
const BENCH_SPACING: f32 = 1.55;

impl Layout {
    pub fn new(app: &App) -> Self {
        let rules = app.net.state.content.as_ref().map(|c| c.rules().clone()).unwrap_or_default();
        let (cols, rows) = (rules.board_cols as i32, rules.board_rows as i32);
        let (left_w, right_w) = (210.0, 300.0);
        let (w, h) = (screen_width(), screen_height());
        let center_w = (w - left_w - right_w).max(100.0);
        // The scene takes the space between the top bar and the item row.
        let area = Rect::new(left_w, 44.0, center_w, (h - 44.0 - 172.0).max(100.0));

        // Centre the board on the origin.
        let raw = |col: i32, row: i32| {
            let hex = Hex::from_offset(col, row);
            vec3(SQRT3 * (hex.q as f32 + hex.r as f32 / 2.0), 0.0, 1.5 * hex.r as f32)
        };
        let origin = (raw(0, 0) + raw(cols - 1, rows - 1)) / 2.0;
        let mut layout = Layout {
            left_w,
            right_w,
            cols,
            rows,
            board_mid_x: left_w + center_w / 2.0,
            view: View::fitted(area, Vec3::ZERO, &[], 0.0, 1.0),
            origin,
            bench_slots: rules.bench_size,
            shop_slots: rules.shop_size,
        };

        // Everything that has to stay on screen: the corners of the board,
        // the ends of the bench, and room for a unit standing at each.
        let mut fit = Vec::new();
        for (col, row) in [(0, 0), (cols - 1, 0), (0, rows - 1), (cols - 1, rows - 1), (0, 1.min(rows - 1)), (cols - 1, 1.min(rows - 1))] {
            for (dx, dz) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                fit.push(layout.hex_world(col, row) + vec3(dx, 0.0, dz));
            }
            fit.push(layout.hex_world(col, row) + vec3(0.0, 2.2, 0.0));
        }
        if layout.bench_slots > 0 {
            for i in [0, layout.bench_slots - 1] {
                fit.push(layout.bench_world(i) + vec3(0.0, 0.0, 0.9));
                fit.push(layout.bench_world(i) + vec3(i as f32 - 0.5, 0.0, 0.0).clamp_length_max(0.9));
            }
        }
        let low = fit.iter().fold(Vec3::splat(f32::MAX), |a, p| a.min(*p));
        let high = fit.iter().fold(Vec3::splat(f32::MIN), |a, p| a.max(*p));
        let target = vec3(0.0, 0.0, (low.z + high.z) / 2.0);
        layout.view = View::fitted(area, target, &fit, app.camera_yaw, app.camera_zoom);
        layout
    }

    /// World position of a point in axial hex coordinates.
    pub fn axial_world(&self, q: f32, r: f32) -> Vec3 {
        vec3(SQRT3 * (q + r / 2.0), TILE_TOP, 1.5 * r) - self.origin
    }

    /// World position of the middle of the hex at `(column, row)`.
    pub fn hex_world(&self, col: i32, row: i32) -> Vec3 {
        let h = Hex::from_offset(col, row);
        self.axial_world(h.q as f32, h.r as f32)
    }

    /// Rotates a world position half a turn around the board's centre.
    pub fn flip(&self, p: Vec3) -> Vec3 {
        vec3(-p.x, p.y, -p.z)
    }

    /// Where bench slot `i` is: a row of pads in front of the board.
    pub fn bench_world(&self, i: usize) -> Vec3 {
        let z = self.hex_world(0, self.rows - 1).z + 2.5;
        let x = (i as f32 - (self.bench_slots as f32 - 1.0) / 2.0) * BENCH_SPACING;
        vec3(x, TILE_TOP, z)
    }

    /// The hex under a window position, if any.
    pub fn hex_at(&self, p: Vec2) -> Option<(i32, i32)> {
        let ground = self.view.ground_at(p)?;
        let mut best = None;
        for row in 0..self.rows {
            for col in 0..self.cols {
                let d = self.hex_world(col, row).distance(ground);
                if d < 0.92 && best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, (col, row)));
                }
            }
        }
        best.map(|(_, pos)| pos)
    }

    /// The bench slot under a window position, if any.
    pub fn bench_at(&self, p: Vec2) -> Option<usize> {
        let ground = self.view.ground_at(p)?;
        (0..self.bench_slots).find(|i| {
            let d = ground - self.bench_world(*i);
            d.x.abs() < BENCH_SPACING / 2.0 && d.z.abs() < 0.85
        })
    }

    /// Bottom edge of the 3D picture.
    pub fn scene_bottom(&self) -> f32 {
        self.view.area.y + self.view.area.h
    }

    pub fn item_slot(&self, i: usize) -> Rect {
        Rect::new(self.left_w + 72.0 + i as f32 * 42.0, self.scene_bottom() + 6.0, 36.0, 36.0)
    }

    pub fn shop_area(&self) -> Rect {
        let top = self.item_slot(0).y + 36.0 + 10.0;
        Rect::new(self.left_w + 10.0, top, screen_width() - self.left_w - self.right_w - 20.0, screen_height() - top - 8.0)
    }

    pub fn shop_card(&self, i: usize) -> Rect {
        let area = self.shop_area();
        let buttons_w = 150.0;
        let gap = 8.0;
        let w = ((area.w - buttons_w) / self.shop_slots.max(1) as f32 - gap).min(150.0);
        Rect::new(area.x + buttons_w + i as f32 * (w + gap), area.y, w, area.h.min(108.0))
    }

    pub fn buttons(&self) -> Rect {
        let area = self.shop_area();
        Rect::new(area.x, area.y, 138.0, area.h)
    }
}

pub fn panel(x: f32, y: f32, w: f32, h: f32, color: Color) {
    draw_rectangle(x, y, w, h, color);
}

/// Text with its baseline at `y`.
pub fn label(text: &str, x: f32, y: f32, size: f32, color: Color) {
    theme::text(text, x, y, size, color);
}

pub fn text_centered(text: &str, x: f32, y: f32, size: f32, color: Color) {
    theme::text(text, x - theme::text_width(text, size) / 2.0, y, size, color);
}

/// Word-wrapped text, top-left at `(x, y)`.
pub fn wrapped(text: &str, x: f32, y: f32, width: f32, size: f32, color: Color) {
    let mut line = String::new();
    let mut row = 0.0;
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if theme::text_width(&candidate, size) > width && !line.is_empty() {
            theme::text(&line, x, y + row * (size + 2.0) + size, size, color);
            row += 1.0;
            line = word.to_string();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        theme::text(&line, x, y + row * (size + 2.0) + size, size, color);
    }
}

/// Draws a button and reports whether it was clicked this frame.
pub fn button(text: &str, x: f32, y: f32, w: f32, h: f32, enabled: bool) -> bool {
    let (mx, my) = mouse_position();
    let hover = enabled && mx >= x && mx <= x + w && my >= y && my <= y + h;
    let fill = if hover { Color::new(0.30, 0.38, 0.52, 1.0) } else { ui().panel_light };
    draw_rectangle(x, y, w, h, fill);
    draw_rectangle_lines(x, y, w, h, 1.0, if enabled { ui().text } else { ui().muted });
    text_centered(text, x + w / 2.0, y + h / 2.0 + 6.0, 19.0, if enabled { ui().text } else { ui().muted });
    hover && is_mouse_button_pressed(MouseButton::Left)
}

/// A stable colour for a name, so the same unit always looks the same.
pub fn unit_color(name: &str) -> Color {
    let mut hash: u32 = 2166136261;
    for b in name.bytes() {
        hash = (hash ^ b as u32).wrapping_mul(16777619);
    }
    let hue = (hash % 360) as f32 / 360.0;
    macroquad::color::hsl_to_rgb(hue, 0.55, 0.42)
}

/// Draws a unit as a round token: body colour by name, ring by side, the
/// name's initials, star pips, item pips, and optionally health and shield
/// bars (`(health, shield)` as fractions of maximum health).
pub fn draw_unit_token(
    c: Vec2,
    radius: f32,
    name: &str,
    star: u8,
    items: usize,
    friendly: bool,
    bars: Option<(f32, f32)>,
) {
    let ring = if friendly { Color::new(0.35, 0.62, 0.98, 1.0) } else { Color::new(0.92, 0.33, 0.33, 1.0) };
    draw_circle(c.x, c.y, radius + 4.0, ring);
    draw_circle(c.x, c.y, radius + 1.0, ui().background);
    draw_circle(c.x, c.y, radius, unit_color(name));
    let initials: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect();
    let size = (radius * 0.95).max(12.0);
    text_centered(&initials, c.x, c.y + size * 0.3, size, WHITE);
    for i in 0..star.min(5) {
        let x = c.x + (i as f32 - (star as f32 - 1.0) / 2.0) * 9.0;
        draw_circle(x, c.y + radius + 7.0, 3.5, Color::new(0.98, 0.80, 0.25, 1.0));
    }
    for i in 0..items.min(3) {
        let x = c.x + radius - 2.0;
        draw_rectangle(x, c.y - radius + i as f32 * 9.0, 7.0, 7.0, Color::new(0.98, 0.80, 0.25, 1.0));
    }
    if let Some((health, shield)) = bars {
        let w = radius * 2.2;
        let (x, y) = (c.x - w / 2.0, c.y - radius - 11.0);
        draw_rectangle(x - 1.0, y - 1.0, w + 2.0, 7.0, ui().background);
        let health = health.clamp(0.0, 1.0);
        draw_rectangle(x, y, w * health, 5.0, if friendly { ui().good } else { Color::new(0.92, 0.33, 0.33, 1.0) });
        let shield = shield.clamp(0.0, 1.0 - health).max(shield.min(0.35).min(1.0)).min(1.0);
        if shield > 0.0 {
            let start = (health - shield).max(0.0).min(1.0 - shield);
            let start = if health + shield <= 1.0 { health } else { start };
            draw_rectangle(x + w * start, y, w * shield.min(1.0 - start), 5.0, Color::new(0.92, 0.94, 0.98, 0.95));
        }
    }
}

/// Star pips with item pips after them, centred on `c`.
pub fn draw_pips(c: Vec2, star: u8, items: usize) {
    let (star, items) = (star.min(5) as usize, items.min(3));
    let width = (star + items) as f32 * 9.0;
    for i in 0..star + items {
        let x = c.x - width / 2.0 + 4.5 + i as f32 * 9.0;
        if i < star {
            draw_circle(x, c.y, 4.5, ui().background);
            draw_circle(x, c.y, 3.5, Color::new(0.98, 0.80, 0.25, 1.0));
        } else {
            draw_rectangle(x - 4.0, c.y - 4.0, 8.0, 8.0, ui().background);
            draw_rectangle(x - 3.0, c.y - 3.0, 6.0, 6.0, Color::new(0.70, 0.86, 0.98, 1.0));
        }
    }
}

/// A health bar centred on `c`; `health` and `shield` are fractions of
/// maximum health. The shield is drawn after the health, or over its end
/// when there is no room left.
pub fn draw_health_bar(c: Vec2, w: f32, health: f32, shield: f32, friendly: bool) {
    let (x, y) = (c.x - w / 2.0, c.y - 3.0);
    draw_rectangle(x - 1.0, y - 1.0, w + 2.0, 8.0, ui().background);
    let health = health.clamp(0.0, 1.0);
    draw_rectangle(x, y, w * health, 6.0, if friendly { ui().good } else { Color::new(0.92, 0.33, 0.33, 1.0) });
    let shield = shield.clamp(0.0, 1.0);
    if shield > 0.0 {
        let start = health.min(1.0 - shield);
        draw_rectangle(x + w * start, y, w * shield, 6.0, Color::new(0.92, 0.94, 0.98, 0.95));
    }
}
