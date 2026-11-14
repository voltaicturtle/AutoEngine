//! The planning phase: board, bench, items, shop, dragging and the pick overlays.

use super::*;

/// What is being dragged with the mouse.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Drag {
    Unit(u32),
    Item(usize),
}

pub(crate) fn draw_planning(app: &mut App, layout: &Layout) {
    let Some(player) = app.shown_player().cloned() else { return };
    let own = app.viewing_self() && player.alive;
    let (mx, my) = mouse_position();
    let mouse = vec2(mx, my);
    let view = &layout.view;
    let dragged = app.drag;
    let dragged_unit = match dragged {
        Some(Drag::Unit(uid)) => Some(uid),
        _ => None,
    };

    // Everyone standing somewhere: the unit, where, and which way it faces.
    let mut standing: Vec<(&OwnedUnit, Vec3, f32)> = Vec::new();
    for (pos, unit) in &player.board {
        standing.push((unit, layout.hex_world(pos.0, pos.1), std::f32::consts::PI));
    }
    for (i, slot) in player.bench.iter().enumerate().take(layout.bench_slots) {
        if let Some(unit) = slot {
            standing.push((unit, layout.bench_world(i), 0.0));
        }
    }
    standing.retain(|(unit, _, _)| dragged_unit != Some(unit.uid));

    // The 3D scene.
    view.begin();
    let mut ground = Ground::default();
    let drop_hex = dragged_unit.and_then(|_| layout.hex_at(mouse)).filter(|(_, row)| *row >= layout.rows / 2);
    board_tiles(&mut ground, layout, true, drop_hex);
    let drop_bench = dragged_unit.and_then(|_| layout.bench_at(mouse));
    for i in 0..layout.bench_slots {
        let fill = if drop_bench == Some(i) { ui().tile_highlight } else { ui().bench };
        ground.hex_tile(layout.bench_world(i), 0.72, 0.14, fill);
    }
    for (_, at, _) in &standing {
        if app.settings.shadows {
            ground.disc(*at + RING_LIFT * 0.5, 0.48, ui().shadow);
        }
        ground.ring(*at + RING_LIFT, 0.50, 0.62, ui().friend);
    }
    ground.draw();
    for (unit, at, yaw) in &standing {
        let look = app.look_of(&unit.unit);
        // Offset each unit's idle so they do not sway in step.
        let pose = look.pose("idle", get_time() as f32 + unit.uid as f32 * 0.37, true);
        app.draw_model(&look, *at, *yaw, look.scale * star_scale(unit.star), [1.0; 3], 0.0, pose);
    }
    let held = dragged_unit.and_then(|uid| player.unit(uid)).zip(view.ground_at(mouse));
    if let Some((unit, at)) = held {
        let look = app.look_of(&unit.unit);
        let pose = look.pose("walk", get_time() as f32, true);
        app.draw_model(&look, at + vec3(0.0, 0.45, 0.0), std::f32::consts::PI, look.scale * star_scale(unit.star), [1.0; 3], 0.12, pose);
    }
    view.end();

    if !own {
        text_centered(&format!("{}'s board", player.name), layout.board_mid_x, view.area.y + 22.0, 20.0, ui().muted);
    }

    // Flat markers on top of the scene, and what the mouse is over.
    let mut hovered: Option<(f32, OwnedUnit)> = None;
    for (unit, at, _) in &standing {
        let size = view.pixels_per_unit(*at);
        draw_pips(view.project(*at) + vec2(0.0, size * 0.42), unit.star, unit.items.len());
        let height = app.look_of(&unit.unit).height() * star_scale(unit.star);
        let body = view.project(*at + vec3(0.0, height * 0.5, 0.0));
        let reach = (mouse - body) / vec2(size * 0.55, size * height * 0.5);
        if reach.length() < 1.0 && hovered.as_ref().is_none_or(|(best, _)| reach.length() < *best) {
            hovered = Some((reach.length(), (*unit).clone()));
        }
    }
    let hovered = hovered.map(|(_, unit)| unit);
    let pressed_unit = hovered.as_ref().filter(|_| is_mouse_button_pressed(MouseButton::Left)).map(|u| u.uid);
    if layout.bench_slots > 0 {
        let first = view.project(layout.bench_world(0) + vec3(-1.0, 0.0, 0.0));
        label("Bench", (first.x - 50.0).max(layout.left_w + 6.0), first.y + 5.0, 18.0, ui().muted);
    }

    // Item inventory.
    let mut pressed_item = None;
    for (i, item) in player.items.iter().enumerate().take(12) {
        let r = layout.item_slot(i);
        if dragged == Some(Drag::Item(i)) {
            continue;
        }
        draw_item(app, item, r);
        if r.contains(mouse) && is_mouse_button_pressed(MouseButton::Left) {
            pressed_item = Some(i);
        }
    }
    if !player.items.is_empty() {
        label("Items", layout.item_slot(0).x - 62.0, layout.item_slot(0).y + 24.0, 18.0, ui().muted);
    }

    if own {
        if let Some(uid) = pressed_unit {
            app.drag = Some(Drag::Unit(uid));
        } else if let Some(i) = pressed_item {
            app.drag = Some(Drag::Item(i));
        }
        draw_shop(app, layout, &player);
        handle_drop(app, layout, &player, mouse);
    }

    // Whatever is being dragged follows the mouse.
    match app.drag {
        Some(Drag::Unit(uid)) => {
            // Over the scene the model itself is in hand; elsewhere a token.
            if let Some(unit) = player.unit(uid)
                && view.ground_at(mouse).is_none()
            {
                draw_unit_token(mouse, 26.0, &unit_label(app, unit), unit.star, unit.items.len(), true, None);
            }
        }
        Some(Drag::Item(i)) => {
            if let Some(item) = player.items.get(i) {
                draw_item(app, item, Rect::new(mouse.x - 18.0, mouse.y - 18.0, 36.0, 36.0));
            }
        }
        None => {
            if let Some(unit) = hovered {
                draw_unit_tooltip(app, &unit, mouse);
            }
        }
    }

    draw_choice(app, layout);
}

pub(crate) fn draw_item(app: &App, item: &str, r: Rect) {
    let name = app.net.state.display_name(item);
    if let Some(icon) = app.icons.borrow_mut().get(item) {
        draw_texture_ex(&icon, r.x, r.y, WHITE, DrawTextureParams { dest_size: Some(vec2(r.w, r.h)), ..Default::default() });
        draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, ui().gold);
        return;
    }
    draw_rectangle(r.x, r.y, r.w, r.h, unit_color(&name));
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, ui().gold);
    let initials: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect();
    text_centered(&initials, r.x + r.w / 2.0, r.y + r.h / 2.0 + 6.0, 18.0, WHITE);
}

pub(crate) fn draw_unit_tooltip(app: &App, unit: &OwnedUnit, at: Vec2) {
    let Some(content) = &app.net.state.content else { return };
    let Some(def) = content.unit(&unit.unit) else { return };
    let mut lines = vec![format!("{}  ({} star, cost {})", def.name, unit.star, def.cost)];
    let traits: Vec<String> = def.traits.iter().map(|t| app.net.state.display_name(t)).collect();
    if !traits.is_empty() {
        lines.push(traits.join(", "));
    }
    if let Some(a) = &def.ability {
        lines.push(format!("Ability: {}", a.name));
    }
    for item in &unit.items {
        lines.push(format!("Item: {}", app.net.state.display_name(item)));
    }
    let width = lines.iter().map(|l| theme::text_width(l, 17.0)).fold(0.0, f32::max) + 20.0;
    let height = lines.len() as f32 * 20.0 + 12.0;
    let x = (at.x + 16.0).min(screen_width() - width - 4.0);
    let y = (at.y + 16.0).min(screen_height() - height - 4.0);
    draw_rectangle(x, y, width, height, Color::new(0.05, 0.06, 0.08, 0.95));
    draw_rectangle_lines(x, y, width, height, 1.0, ui().muted);
    for (i, line) in lines.iter().enumerate() {
        label(line, x + 10.0, y + 22.0 + i as f32 * 20.0, 17.0, if i == 0 { ui().text } else { ui().muted });
    }
}

pub(crate) fn draw_shop(app: &mut App, layout: &Layout, player: &Player) {
    let Some(content) = &app.net.state.content else { return };
    let economy = content.rules().economy.clone();
    let mut buy = None;
    let (mx, my) = mouse_position();
    for (i, offer) in player.shop.iter().enumerate() {
        let r = layout.shop_card(i);
        let Some(def) = offer.as_ref().and_then(|id| content.unit(id)) else {
            panel(r.x, r.y, r.w, r.h, Color::new(0.10, 0.11, 0.15, 1.0));
            continue;
        };
        let affordable = player.gold >= def.cost as i64;
        let hover = r.contains(vec2(mx, my)) && app.drag.is_none();
        panel(r.x, r.y, r.w, r.h, if hover { ui().panel_light } else { ui().panel });
        draw_rectangle(r.x, r.y, r.w, 5.0, unit_color(&def.name));
        if let Some(icon) = app.icons.borrow_mut().get(&def.id) {
            let params = DrawTextureParams { dest_size: Some(vec2(44.0, 44.0)), ..Default::default() };
            draw_texture_ex(&icon, r.x + 6.0, r.y + 16.0, WHITE, params);
        } else {
            draw_unit_token(vec2(r.x + 28.0, r.y + 38.0), 18.0, &def.name, 1, 0, true, None);
        }
        label(&def.name, r.x + 8.0, r.y + 76.0, 16.0, if affordable { ui().text } else { ui().muted });
        let traits: Vec<String> = def.traits.iter().map(|t| app.net.state.display_name(t)).collect();
        label(&traits.join(", "), r.x + 8.0, r.y + 94.0, 14.0, ui().muted);
        label(&format!("{}g", def.cost), r.x + r.w - 30.0, r.y + 42.0, 20.0, ui().gold);
        if hover && is_mouse_button_pressed(MouseButton::Left) {
            buy = Some(i);
        }
    }
    let b = layout.buttons();
    let reroll = button(&format!("Reroll ({}g)", economy.reroll_cost), b.x, b.y, b.w, 30.0, player.gold >= economy.reroll_cost);
    let xp = button(&format!("Buy XP ({}g)", economy.xp_cost), b.x, b.y + 36.0, b.w, 30.0, player.gold >= economy.xp_cost);
    let ready = button(if player.ready { "Ready!" } else { "Ready" }, b.x, b.y + 72.0, b.w, 34.0, !player.ready);
    if let Some(slot) = buy {
        app.command(Command::BuyUnit { slot });
        app.play(Sfx::Buy);
    }
    if reroll {
        app.command(Command::Reroll);
    }
    if xp {
        app.command(Command::BuyXp);
    }
    if ready {
        app.command(Command::Ready);
    }
}

/// Turns the end of a drag into a command.
pub(crate) fn handle_drop(app: &mut App, layout: &Layout, player: &Player, mouse: Vec2) {
    if !is_mouse_button_released(MouseButton::Left) {
        return;
    }
    let Some(drag) = app.drag.take() else { return };
    let over_hex = layout.hex_at(mouse);
    let over_bench = layout.bench_at(mouse);
    match drag {
        Drag::Unit(uid) => {
            if let Some((col, row)) = over_hex {
                app.command(Command::MoveUnit { uid, to: Slot::Board(col, row) });
            } else if let Some(i) = over_bench {
                app.command(Command::MoveUnit { uid, to: Slot::Bench(i) });
            } else if layout.shop_area().contains(mouse) {
                app.command(Command::SellUnit { uid });
            }
        }
        Drag::Item(item) => {
            let on_board = over_hex.and_then(|pos| player.board.get(&pos)).map(|u| u.uid);
            let on_bench = over_bench.and_then(|i| player.bench.get(i).cloned().flatten()).map(|u| u.uid);
            if let Some(uid) = on_board.or(on_bench) {
                app.command(Command::EquipItem { item, uid });
            }
        }
    }
}

/// Options offered by draft and choice rounds.
pub(crate) fn draw_choice(app: &mut App, layout: &Layout) {
    let Some(view) = &app.net.state.view else { return };
    if view.options.is_empty() || !matches!(view.phase, ViewPhase::Planning { .. }) {
        return;
    }
    let Some(me) = app.net.state.seat else { return };
    let my_turn = view.awaiting.contains(&me);
    // In a draft the options stay up while others pick; in a private choice
    // they disappear once we have chosen.
    if !my_turn && view.round_kind != "draft" {
        return;
    }
    let options = view.options.clone();
    let title = if my_turn {
        "Choose one".to_string()
    } else {
        match view.awaiting.first() {
            Some(p) => format!("Waiting for {} to choose", app.net.state.player_name(*p)),
            None => return,
        }
    };
    let n = options.len() as f32;
    let (cw, ch, gap) = (176.0, 120.0, 10.0);
    let per_row = 4.0_f32.min(n);
    let rows = (n / per_row).ceil();
    let total_w = per_row * cw + (per_row - 1.0) * gap;
    let x0 = layout.board_mid_x - total_w / 2.0;
    let y0 = layout.view.area.y + 70.0;
    draw_rectangle(x0 - 16.0, y0 - 44.0, total_w + 32.0, rows * (ch + gap) + 56.0, Color::new(0.04, 0.05, 0.07, 0.94));
    text_centered(&title, layout.board_mid_x, y0 - 14.0, 24.0, if my_turn { ui().gold } else { ui().muted });
    let (mx, my) = mouse_position();
    let mut pick = None;
    for (i, option) in options.iter().enumerate() {
        let r = Rect::new(
            x0 + (i as f32 % per_row) * (cw + gap),
            y0 + (i as f32 / per_row).floor() * (ch + gap),
            cw,
            ch,
        );
        let Some(option) = option else {
            panel(r.x, r.y, r.w, r.h, Color::new(0.09, 0.10, 0.13, 1.0));
            text_centered("taken", r.x + r.w / 2.0, r.y + r.h / 2.0, 18.0, ui().muted);
            continue;
        };
        let hover = my_turn && r.contains(vec2(mx, my));
        panel(r.x, r.y, r.w, r.h, if hover { ui().panel_light } else { ui().panel });
        let parts: Vec<&str> = option.split(" + ").collect();
        let name = app.net.state.display_name(parts[0]);
        draw_rectangle(r.x, r.y, r.w, 5.0, unit_color(&name));
        label(&name, r.x + 10.0, r.y + 30.0, 18.0, ui().text);
        let content = app.net.state.content.as_ref();
        if let Some(item) = parts.get(1) {
            label(&format!("with {}", app.net.state.display_name(item)), r.x + 10.0, r.y + 52.0, 15.0, ui().gold);
        } else if let Some(aug) = content.and_then(|c| c.augment(parts[0])) {
            draw::wrapped(&aug.description, r.x + 10.0, r.y + 52.0, r.w - 20.0, 15.0, ui().muted);
        }
        if hover && is_mouse_button_pressed(MouseButton::Left) {
            pick = Some(i);
        }
    }
    if let Some(option) = pick {
        app.command(Command::Pick { option });
    }
}
