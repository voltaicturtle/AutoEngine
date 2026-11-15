//! Typing: the developer console, a line sent to the server as a cheat
//! command, and the chat line.

use super::*;

/// The cheat console: the bound key opens it, Enter sends the line to the
/// server as a developer command, Escape or the key again closes it.
pub(crate) fn update_console(app: &mut App) {
    if app.pressed(Action::Console) {
        app.console = if app.console.is_some() { None } else { Some(String::new()) };
        while get_char_pressed().is_some() {}
        return;
    }
    let Some(line) = &mut app.console else { return };
    while let Some(c) = get_char_pressed() {
        if (c.is_ascii_alphanumeric() || c == ' ' || c == '_' || c == '-') && line.len() < 60 {
            line.push(c);
        }
    }
    if is_key_pressed(KeyCode::Backspace) {
        line.pop();
    }
    if is_key_pressed(KeyCode::Escape) {
        app.console = None;
        app.menu_open = false;
    } else if is_key_pressed(KeyCode::Enter) {
        let line = std::mem::take(line);
        match DevCommand::parse(&line) {
            Ok(command) => {
                app.net.state.error = None;
                app.send(ClientMsg::Dev(command));
            }
            Err(e) => app.net.state.error = Some(e),
        }
    }
}

pub(crate) fn draw_console(app: &App, layout: &Layout) {
    let Some(line) = &app.console else { return };
    let area = layout.view.area;
    let y = area.y + area.h - 34.0;
    draw_rectangle(area.x, y, area.w, 34.0, Color::new(0.0, 0.0, 0.0, 0.85));
    let cursor = if (get_time() * 2.0) as i64 % 2 == 0 { "_" } else { " " };
    label(&format!("> {line}{cursor}"), area.x + 10.0, y + 23.0, 20.0, ui().good);
    let hint = "gold N   xp N   unit ID   item ID   augment ID   hurt N   skip";
    label(hint, area.x + area.w - theme::text_width(hint, 14.0) - 10.0, y + 22.0, 14.0, ui().muted);
}

/// The chat line: the bound key (Enter) opens it, Enter sends what was
/// typed to everyone, Escape closes it. `/mute NAME` and `/unmute NAME`
/// are handled here instead of being sent.
pub(crate) fn update_chat(app: &mut App) {
    let Some(line) = &mut app.chat else {
        if app.net.state.welcomed && app.pressed(Action::Chat) {
            app.chat = Some(String::new());
            while get_char_pressed().is_some() {}
        }
        return;
    };
    while let Some(c) = get_char_pressed() {
        if !c.is_control() && line.chars().count() < autoengine::protocol::MAX_CHAT_CHARS {
            line.push(c);
        }
    }
    if is_key_pressed(KeyCode::Backspace) {
        line.pop();
    }
    if is_key_pressed(KeyCode::Escape) {
        app.chat = None;
        app.menu_open = false;
    } else if is_key_pressed(KeyCode::Enter) {
        let line = app.chat.take().unwrap_or_default();
        // `/mute NAME` and `/unmute NAME` hide or show someone's chat here.
        if let Some(name) = line.strip_prefix("/mute ") {
            app.net.state.muted.insert(name.trim().to_string());
        } else if let Some(name) = line.strip_prefix("/unmute ") {
            app.net.state.muted.remove(name.trim());
        } else {
            app.net.state.say(&line);
        }
    }
}

pub(crate) fn draw_chat(app: &App, layout: &Layout) {
    let Some(line) = &app.chat else { return };
    let area = layout.view.area;
    let y = area.y + area.h - 34.0;
    draw_rectangle(area.x, y, area.w, 34.0, Color::new(0.0, 0.0, 0.0, 0.85));
    let cursor = if (get_time() * 2.0) as i64 % 2 == 0 { "_" } else { " " };
    label(&format!("say: {line}{cursor}"), area.x + 10.0, y + 23.0, 20.0, ui().text);
}
