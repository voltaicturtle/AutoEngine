//! Layout and small drawing helpers.

use autoengine::hex::Hex;
use macroquad::prelude::*;

use crate::{App, BG, GOOD, MUTED, PANEL_LIGHT, TEXT};

const SQRT3: f32 = 1.732_050_8;

/// Where everything goes for the current window size.
pub struct Layout {
    pub left_w: f32,
    pub right_w: f32,
    pub cols: i32,
    pub rows: i32,
    /// Hex radius (center to corner).
    pub hex: f32,
    pub board_top: f32,
    pub board_mid_x: f32,
    board_left: f32,
    pub bench_slots: usize,
    shop_slots: usize,
}

impl Layout {
    pub fn new(app: &App) -> Self {
        let rules = app.state.content.as_ref().map(|c| c.rules().clone()).unwrap_or_default();
        let (cols, rows) = (rules.board_cols as i32, rules.board_rows as i32);
        let (left_w, right_w) = (210.0, 300.0);
        let (w, h) = (screen_width(), screen_height());
        let center_w = w - left_w - right_w;
        // Fit the board into the space above the bench and shop.
        let room_h = h - 44.0 - 30.0 - 250.0;
        let by_height = room_h / (1.5 * (rows - 1) as f32 + 2.0);
        let by_width = (center_w - 140.0) / (SQRT3 * (cols as f32 + 0.5));
        let hex = by_height.min(by_width).clamp(12.0, 60.0);
        let board_w = SQRT3 * hex * (cols as f32 + 0.5);
        let board_mid_x = left_w + center_w / 2.0;
        Layout {
            left_w,
            right_w,
            cols,
            rows,
            hex,
            board_top: 44.0 + 30.0,
            board_mid_x,
            board_left: board_mid_x - board_w / 2.0,
            bench_slots: rules.bench_size,
            shop_slots: rules.shop_size,
        }
    }

    /// Screen position of a point in axial hex coordinates.
    pub fn axial_center(&self, q: f32, r: f32) -> Vec2 {
        vec2(
            self.board_left + SQRT3 * self.hex * (q + r / 2.0 + 0.5),
            self.board_top + self.hex + 1.5 * self.hex * r,
        )
    }

    /// Screen position of the hex at `(column, row)`.
    pub fn hex_center(&self, col: i32, row: i32) -> Vec2 {
        let h = Hex::from_offset(col, row);
        self.axial_center(h.q as f32, h.r as f32)
    }

    /// Rotates a board position half a turn around the board's center.
    pub fn flip(&self, p: Vec2) -> Vec2 {
        let a = self.hex_center(0, 0);
        let b = self.hex_center(self.cols - 1, self.rows - 1);
        a + b - p
    }

    /// The hex under a screen position, if any.
    pub fn hex_at(&self, p: Vec2) -> Option<(i32, i32)> {
        let mut best = None;
        for row in 0..self.rows {
            for col in 0..self.cols {
                let d = self.hex_center(col, row).distance(p);
                if d < self.hex * 0.9 && best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, (col, row)));
                }
            }
        }
        best.map(|(_, pos)| pos)
    }

    fn board_bottom(&self) -> f32 {
        self.board_top + 1.5 * self.hex * (self.rows - 1) as f32 + 2.0 * self.hex
    }

    pub fn bench_slot(&self, i: usize) -> Rect {
        let size = 54.0;
        let gap = 6.0;
        let total = self.bench_slots as f32 * (size + gap) - gap;
        Rect::new(self.board_mid_x - total / 2.0 + i as f32 * (size + gap), self.board_bottom() + 10.0, size, size)
    }

    pub fn item_slot(&self, i: usize) -> Rect {
        let first = self.bench_slot(0);
        Rect::new(first.x + i as f32 * 42.0, first.y + first.h + 8.0, 36.0, 36.0)
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
    draw_text(text, x, y, size, color);
}

pub fn text_centered(text: &str, x: f32, y: f32, size: f32, color: Color) {
    let m = measure_text(text, None, size as u16, 1.0);
    draw_text(text, x - m.width / 2.0, y, size, color);
}

/// Word-wrapped text, top-left at `(x, y)`.
pub fn wrapped(text: &str, x: f32, y: f32, width: f32, size: f32, color: Color) {
    let mut line = String::new();
    let mut row = 0.0;
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if measure_text(&candidate, None, size as u16, 1.0).width > width && !line.is_empty() {
            draw_text(&line, x, y + row * (size + 2.0) + size, size, color);
            row += 1.0;
            line = word.to_string();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        draw_text(&line, x, y + row * (size + 2.0) + size, size, color);
    }
}

/// Draws a button and reports whether it was clicked this frame.
pub fn button(text: &str, x: f32, y: f32, w: f32, h: f32, enabled: bool) -> bool {
    let (mx, my) = mouse_position();
    let hover = enabled && mx >= x && mx <= x + w && my >= y && my <= y + h;
    let fill = if hover { Color::new(0.30, 0.38, 0.52, 1.0) } else { PANEL_LIGHT };
    draw_rectangle(x, y, w, h, fill);
    draw_rectangle_lines(x, y, w, h, 1.0, if enabled { TEXT } else { MUTED });
    text_centered(text, x + w / 2.0, y + h / 2.0 + 6.0, 19.0, if enabled { TEXT } else { MUTED });
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
    draw_circle(c.x, c.y, radius + 1.0, BG);
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
        draw_rectangle(x - 1.0, y - 1.0, w + 2.0, 7.0, BG);
        let health = health.clamp(0.0, 1.0);
        draw_rectangle(x, y, w * health, 5.0, if friendly { GOOD } else { Color::new(0.92, 0.33, 0.33, 1.0) });
        let shield = shield.clamp(0.0, 1.0 - health).max(shield.min(0.35).min(1.0)).min(1.0);
        if shield > 0.0 {
            let start = (health - shield).max(0.0).min(1.0 - shield);
            let start = if health + shield <= 1.0 { health } else { start };
            draw_rectangle(x + w * start, y, w * shield.min(1.0 - start), 5.0, Color::new(0.92, 0.94, 0.98, 0.95));
        }
    }
}
