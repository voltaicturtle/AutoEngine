//! The settings menu and the statistics overlay.

use super::*;

/// The menu: settings that take effect at once and are saved to the file.
pub(crate) fn draw_menu(app: &mut App) {
    let (w, h) = (screen_width(), screen_height());
    draw_rectangle(0.0, 0.0, w, h, Color::new(0.03, 0.04, 0.06, 0.82));
    let (pw, ph) = (460.0, 530.0);
    let (x, y) = (w / 2.0 - pw / 2.0, h / 2.0 - ph / 2.0);
    panel(x, y, pw, ph, ui().panel);
    draw_rectangle_lines(x, y, pw, ph, 1.0, ui().muted);
    text_centered("Menu", w / 2.0, y + 40.0, 30.0, ui().text);

    let before = app.settings.clone();
    let on_off = |on: bool| if on { "On" } else { "Off" };
    let mut row = 0.0;
    let mut toggle = |name: &str, value: &mut bool| {
        let ry = y + 70.0 + row * 46.0;
        row += 1.0;
        label(name, x + 30.0, ry + 25.0, 20.0, ui().text);
        if button(on_off(*value), x + pw - 150.0, ry, 120.0, 36.0, true) {
            *value = !*value;
        }
    };
    toggle("Particles", &mut app.settings.particles);
    toggle("Shadows", &mut app.settings.shadows);
    toggle("Animations", &mut app.settings.animations);
    toggle("Statistics (F3)", &mut app.settings.show_stats);
    toggle("Fullscreen", &mut app.settings.fullscreen);

    let stepper = |name: &str, shown: String, row: f32| -> i32 {
        let ry = y + 70.0 + row * 46.0;
        label(name, x + 30.0, ry + 25.0, 20.0, ui().text);
        let minus = button("-", x + pw - 150.0, ry, 36.0, 36.0, true);
        text_centered(&shown, x + pw - 90.0, ry + 25.0, 20.0, ui().text);
        let plus = button("+", x + pw - 66.0, ry, 36.0, 36.0, true);
        plus as i32 - minus as i32
    };
    let volume = if Sounds::AVAILABLE { "Volume" } else { "Volume (sound not built in)" };
    let step = stepper(volume, app.settings.volume.to_string(), 5.0);
    app.settings.volume = (app.settings.volume as i32 + step * 10).clamp(0, 100) as u8;
    let step = stepper("Camera turn speed", format!("{}%", app.settings.camera_turn_percent), 6.0);
    app.settings.camera_turn_percent = (app.settings.camera_turn_percent as i32 + step * 25).clamp(25, 400) as u32;

    if app.settings != before {
        if app.settings.fullscreen != before.fullscreen {
            set_fullscreen(app.settings.fullscreen);
        }
        if app.settings.volume != before.volume {
            app.play(Sfx::Buy);
        }
        app.save_settings();
    }
    // Keys are rebound in the settings file.
    let keys: Vec<String> = Action::ALL.iter().map(|a| format!("{} {}", a.name().replace('_', " "), app.settings.key_name(*a))).collect();
    draw::wrapped(&format!("Keys: {}", keys.join(", ")), x + 30.0, y + 398.0, pw - 60.0, 15.0, ui().muted);
    let by = y + ph - 58.0;
    if button("Resume", x + 30.0, by, 190.0, 40.0, true) {
        app.menu_open = false;
    }
    if button("Quit", x + pw - 220.0, by, 190.0, 40.0, true) {
        macroquad::miniquad::window::order_quit();
    }
}

/// Frame rate and what the scene is costing, in the corner of the 3D view.
pub(crate) fn draw_stats(app: &App, layout: &Layout) {
    let frames = &app.frame_ms;
    let average = frames.iter().sum::<f32>() / frames.len().max(1) as f32;
    let worst = frames.iter().copied().fold(0.0, f32::max);
    let (units, vertices) = app.drawn.get();
    let link = if app.replay.is_some() {
        "replay file"
    } else if app.conn.as_ref().is_some_and(|c| !c.is_closed()) {
        "connected"
    } else {
        "not connected"
    };
    let lines = [
        format!("{} fps", get_fps()),
        format!("frame {average:.1} ms, worst {worst:.1} ms"),
        format!("{units} models, {vertices} vertices"),
        format!("{} particles", app.particles.len()),
        format!("{}x{}", screen_width() as i32, screen_height() as i32),
        link.to_string(),
    ];
    let (x, y) = (layout.view.area.x + 8.0, layout.view.area.y + 8.0);
    draw_rectangle(x, y, 250.0, lines.len() as f32 * 18.0 + 10.0, Color::new(0.0, 0.0, 0.0, 0.6));
    for (i, line) in lines.iter().enumerate() {
        label(line, x + 8.0, y + 20.0 + i as f32 * 18.0, 16.0, ui().good);
    }
}
