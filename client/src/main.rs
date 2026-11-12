//! The graphical client.
//!
//! This program contains no game rules. It connects to an
//! `autoengine-server`, draws what the server reports and sends the player's
//! clicks back as commands. Rendering goes through OpenGL (via macroquad).
//!
//! Usage: `autoengine-client [--addr 127.0.0.1:7777] [--name NAME] [--spectate]
//!         [--autoplay] [--no-ready] [--screenshot SECONDS:FILE]...`
//!
//! Controls: drag units between the bench and your half of the board, drag a
//! unit onto the shop to sell it, drag items onto units, click shop cards to
//! buy. Click a name on the scoreboard to look at that player's board.
//! Space skips a fight replay.

mod draw;

use autoengine::client::ClientState;
use autoengine::content::BoardUnit;
use autoengine::game::{Command, Opponent, OwnedUnit, Player, PlayerId, Slot};
use autoengine::hex::Hex;
use autoengine::net::Connection;
use autoengine::playback::{Flash, Playback};
use autoengine::protocol::{ClientMsg, PROTOCOL_VERSION, ViewPhase};
use macroquad::prelude::*;

use draw::{Layout, button, draw_unit_token, label, panel, text_centered, unit_color};

const BG: Color = Color::new(0.07, 0.08, 0.11, 1.0);
const PANEL: Color = Color::new(0.12, 0.14, 0.19, 1.0);
const PANEL_LIGHT: Color = Color::new(0.18, 0.21, 0.28, 1.0);
const TEXT: Color = Color::new(0.90, 0.92, 0.96, 1.0);
const MUTED: Color = Color::new(0.58, 0.62, 0.70, 1.0);
const GOLD_COLOR: Color = Color::new(0.98, 0.80, 0.25, 1.0);
const GOOD: Color = Color::new(0.35, 0.82, 0.45, 1.0);
const BAD: Color = Color::new(0.92, 0.33, 0.33, 1.0);
const ACCENT: Color = Color::new(0.35, 0.62, 0.98, 1.0);

struct Options {
    addr: String,
    name: String,
    spectate: bool,
    /// Play on its own: start the game, buy, place units and ready up.
    autoplay: bool,
    /// With `autoplay`, leave readying up to the round timer.
    no_ready: bool,
    /// `(seconds after launch, file)` pairs.
    screenshots: Vec<(f64, String)>,
    quit_after_screenshots: bool,
}

fn parse_options() -> Result<Options, String> {
    let usage = "usage: autoengine-client [--addr HOST:PORT] [--name NAME] [--spectate] [--autoplay] \
                 [--no-ready] [--screenshot SECONDS:FILE]...";
    let mut o = Options {
        addr: "127.0.0.1:7777".into(),
        name: "Player".into(),
        spectate: false,
        autoplay: false,
        no_ready: false,
        screenshots: Vec::new(),
        quit_after_screenshots: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(usage.to_string());
        match arg.as_str() {
            "--addr" => o.addr = value()?,
            "--name" => o.name = value()?,
            "--spectate" => o.spectate = true,
            "--autoplay" => o.autoplay = true,
            "--no-ready" => o.no_ready = true,
            "--screenshot" => {
                let v = value()?;
                let (secs, file) = v.split_once(':').ok_or(usage)?;
                o.screenshots.push((secs.parse().map_err(|_| usage)?, file.to_string()));
                o.quit_after_screenshots = true;
            }
            _ => return Err(usage.to_string()),
        }
    }
    Ok(o)
}

fn window_conf() -> Conf {
    Conf {
        window_title: "AutoEngine".to_owned(),
        window_width: 1280,
        window_height: 800,
        high_dpi: false,
        ..Default::default()
    }
}

/// What is being dragged with the mouse.
#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Unit(u32),
    Item(usize),
}

/// A fight replay in progress.
struct Show {
    playback: Playback,
    /// Draw the board rotated half a turn so our units are at the bottom.
    flipped: bool,
    time_ms: f32,
    title: String,
    done_for_ms: f32,
}

struct App {
    options: Options,
    conn: Option<Connection>,
    state: ClientState,
    drag: Option<Drag>,
    show: Option<Show>,
    /// Whose board is on screen; `None` means our own.
    viewing: Option<PlayerId>,
    /// Round the autopilot is working on, how far it got, and when.
    autoplay_step: (Option<usize>, u8, f64),
    connect_error: Option<String>,
    error_shown_at: f64,
    last_reconnect_at: f64,
    /// Time left in the current phase as last reported, and when that was.
    countdown: (u64, f64),
}

impl App {
    fn send(&mut self, message: ClientMsg) {
        if let Some(conn) = &mut self.conn
            && conn.send(&message).is_err()
        {
            self.connect_error = Some("lost the connection to the server".into());
        }
    }

    /// Connects and introduces ourselves; on a reconnect the token from the
    /// first welcome gets our seat back.
    fn connect(&mut self) {
        self.last_reconnect_at = get_time();
        match Connection::connect(&self.options.addr) {
            Ok(conn) => {
                self.conn = Some(conn);
                self.connect_error = None;
                let hello = if self.options.spectate {
                    ClientMsg::Spectate { version: PROTOCOL_VERSION }
                } else {
                    ClientMsg::Join {
                        name: self.options.name.clone(),
                        version: PROTOCOL_VERSION,
                        token: self.state.token.clone(),
                    }
                };
                self.send(hello);
            }
            Err(e) => {
                self.conn = None;
                self.connect_error = Some(format!("cannot reach the server at {}: {e}", self.options.addr));
            }
        }
    }

    fn command(&mut self, command: Command) {
        self.send(ClientMsg::Command(command));
    }

    fn me(&self) -> Option<&Player> {
        self.state.me()
    }

    /// The player whose board is on screen.
    fn shown_player(&self) -> Option<&Player> {
        let view = self.state.view.as_ref()?;
        let id = self.viewing.or(self.state.seat).or_else(|| view.players.iter().find(|p| p.alive).map(|p| p.id))?;
        view.players.get(id.0 as usize)
    }

    fn viewing_self(&self) -> bool {
        self.state.seat.is_some() && (self.viewing.is_none() || self.viewing == self.state.seat)
    }

    fn pump_network(&mut self) {
        let lost = self.conn.as_ref().is_none_or(|c| c.is_closed()) || self.connect_error.is_some();
        if lost {
            // Keep trying to get back in.
            if self.connect_error.is_none() {
                self.connect_error = Some("lost the connection to the server; retrying".into());
            }
            if get_time() - self.last_reconnect_at > 2.0 {
                self.connect();
            }
        }
        let Some(conn) = &mut self.conn else { return };
        for message in conn.poll() {
            self.state.apply(message);
        }
        if let Some(view) = &self.state.view {
            let remaining = match view.phase {
                ViewPhase::Planning { remaining_ms } | ViewPhase::Showing { remaining_ms } => remaining_ms,
                _ => 0,
            };
            if remaining != self.countdown.0 {
                self.countdown = (remaining, get_time());
            }
        }
        if self.state.error.is_some() && self.error_shown_at == 0.0 {
            self.error_shown_at = get_time();
        }
        if self.state.error.is_some() && get_time() - self.error_shown_at > 4.0 {
            self.state.error = None;
            self.error_shown_at = 0.0;
        }

        // Start the next replay once the current one is over.
        if self.show.is_none()
            && let Some(record) = self.state.combats.pop_front()
        {
            // Spectators receive every fight of the round; show the first.
            if self.state.seat.is_none() {
                self.state.combats.clear();
            }
            let me = self.state.seat;
            let flipped = me.is_some() && record.away == Opponent::Player(me.unwrap());
            let away = match &record.away {
                Opponent::Player(p) => self.state.player_name(*p),
                Opponent::Ghost(p) => format!("a copy of {}", self.state.player_name(*p)),
                Opponent::Neutral => "the neutral camp".to_string(),
            };
            let title = format!("{} vs {}", self.state.player_name(record.home), away);
            self.show = Some(Show { playback: Playback::new(&record), flipped, time_ms: 0.0, title, done_for_ms: 0.0 });
        }
    }

    /// A small built-in autopilot, for demos and for testing the client.
    fn autoplay(&mut self) {
        let Some(view) = &self.state.view else {
            if self.state.welcomed && !self.state.started && self.state.seat.is_some() && get_time() > 1.0 {
                self.send(ClientMsg::Start);
                self.state.started = true;
            }
            return;
        };
        let (Some(me), Some(content)) = (self.me().cloned(), &self.state.content) else { return };
        if !matches!(view.phase, ViewPhase::Planning { .. }) || self.show.is_some() {
            return;
        }
        if view.awaiting.contains(&me.id) {
            if let Some(option) = view.options.iter().position(Option::is_some) {
                self.command(Command::Pick { option });
            }
            return;
        }
        // Shop first, then arrange a moment later once the server has
        // confirmed the purchases.
        let now = get_time();
        if self.autoplay_step.0 != Some(view.round_index) {
            self.autoplay_step = (Some(view.round_index), 0, now);
        }
        let (_, step, since) = self.autoplay_step;
        let rules = content.rules().clone();
        let limit = rules.levels[(me.level as usize - 1).min(rules.levels.len() - 1)].board_size;
        let mut commands = Vec::new();
        if step == 0 {
            let mut gold = me.gold;
            for (slot, offer) in me.shop.iter().enumerate() {
                let cost = offer.as_ref().and_then(|u| content.unit(u)).map(|u| u.cost as i64);
                if let Some(cost) = cost
                    && gold >= cost
                {
                    gold -= cost;
                    commands.push(Command::BuyUnit { slot });
                }
            }
            self.autoplay_step = (Some(view.round_index), 1, now);
            for c in commands {
                self.command(c);
            }
            return;
        }
        if step != 1 || now - since < 0.4 {
            return;
        }
        self.autoplay_step.1 = 2;
        // Field benched units on free hexes, front row first.
        let mut fielded = me.board.len();
        let mut free: Vec<(i32, i32)> = ((rules.board_rows / 2) as i32..rules.board_rows as i32)
            .flat_map(|r| (0..rules.board_cols as i32).map(move |c| (c, r)))
            .filter(|pos| !me.board.contains_key(pos))
            .collect();
        free.sort_by_key(|(c, r)| (*r, (c - rules.board_cols as i32 / 2).abs()));
        let mut free = free.into_iter();
        for unit in me.bench.iter().flatten() {
            if fielded >= limit {
                break;
            }
            if let Some((c, r)) = free.next() {
                commands.push(Command::MoveUnit { uid: unit.uid, to: Slot::Board(c, r) });
                fielded += 1;
            }
        }
        if let Some(unit) = me.board.values().next() {
            for _ in 0..me.items.len() {
                commands.push(Command::EquipItem { item: 0, uid: unit.uid });
            }
        }
        if !self.options.no_ready {
            commands.push(Command::Ready);
        }
        for c in commands {
            self.command(c);
        }
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let options = match parse_options() {
        Ok(o) => o,
        Err(usage) => {
            eprintln!("{usage}");
            return;
        }
    };
    let mut app = App {
        conn: None,
        state: ClientState::new(),
        drag: None,
        show: None,
        viewing: None,
        autoplay_step: (None, 0, 0.0),
        connect_error: None,
        error_shown_at: 0.0,
        last_reconnect_at: 0.0,
        countdown: (0, 0.0),
        options,
    };
    app.connect();

    let mut shots = std::mem::take(&mut app.options.screenshots);
    shots.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut shots = shots.into_iter().peekable();

    loop {
        app.pump_network();
        if app.options.autoplay {
            app.autoplay();
        }
        clear_background(BG);
        let layout = Layout::new(&app);
        if app.state.view.is_none() {
            draw_lobby(&mut app);
        } else {
            draw_game(&mut app, &layout);
        }
        draw_banner(&app);

        if let Some((at, _)) = shots.peek()
            && get_time() >= *at
        {
            let (_, file) = shots.next().unwrap();
            get_screen_data().export_png(&file);
            println!("saved {file}");
        }
        if app.options.quit_after_screenshots && shots.peek().is_none() {
            break;
        }
        next_frame().await
    }
}

/// Errors from the server or the connection, across the top of the screen.
fn draw_banner(app: &App) {
    let message = app.connect_error.as_ref().or(app.state.error.as_ref());
    if let Some(message) = message {
        let w = screen_width();
        draw_rectangle(w / 2.0 - 300.0, 50.0, 600.0, 34.0, Color::new(0.45, 0.12, 0.12, 0.95));
        text_centered(message, w / 2.0, 72.0, 20.0, TEXT);
    }
}

fn draw_lobby(app: &mut App) {
    let (w, h) = (screen_width(), screen_height());
    text_centered("AutoEngine", w / 2.0, h * 0.22, 64.0, TEXT);
    if !app.state.welcomed {
        let line = if app.connect_error.is_some() { "Not connected" } else { "Connecting..." };
        text_centered(line, w / 2.0, h * 0.32, 26.0, MUTED);
        return;
    }
    text_centered("Waiting for players", w / 2.0, h * 0.32, 26.0, MUTED);
    let seats = app.state.lobby.clone();
    for (i, seat) in seats.iter().enumerate() {
        let y = h * 0.40 + i as f32 * 34.0;
        panel(w / 2.0 - 180.0, y, 360.0, 28.0, PANEL);
        let mine = app.state.seat == Some(PlayerId(i as u8));
        match seat {
            Some(name) => label(name, w / 2.0 - 168.0, y + 20.0, 20.0, if mine { GOLD_COLOR } else { TEXT }),
            None => label("open seat (a bot will play it)", w / 2.0 - 168.0, y + 20.0, 20.0, MUTED),
        }
    }
    let y = h * 0.40 + seats.len() as f32 * 34.0 + 24.0;
    if app.state.seat.is_some() {
        if button("Start game", w / 2.0 - 90.0, y, 180.0, 44.0, true) {
            app.send(ClientMsg::Start);
        }
    } else {
        text_centered("Spectating: waiting for the game to start", w / 2.0, y + 24.0, 20.0, MUTED);
    }
}

fn draw_game(app: &mut App, layout: &Layout) {
    draw_top_bar(app);
    draw_traits(app, layout);
    draw_scoreboard(app, layout);

    let dt_ms = get_frame_time() * 1000.0;
    let skip = is_key_pressed(KeyCode::Space);
    let mut finished = false;
    if let Some(show) = &mut app.show {
        show.time_ms += dt_ms;
        show.playback.advance_to(show.time_ms as u32);
        show.playback.forget_flashes_older_than(1500);
        if show.playback.is_finished() {
            show.done_for_ms += dt_ms;
        }
        finished = show.done_for_ms > 1500.0 || skip;
    }
    if finished {
        app.show = None;
    }

    if app.show.is_some() {
        draw_replay(app, layout);
    } else {
        draw_planning(app, layout);
    }
    draw_game_over(app);
}

fn draw_top_bar(app: &App) {
    let Some(view) = &app.state.view else { return };
    let w = screen_width();
    draw_rectangle(0.0, 0.0, w, 44.0, PANEL);
    label(&format!("Round {}  ({})", view.round_name, view.round_kind), 16.0, 29.0, 24.0, TEXT);
    let phase = match view.phase {
        ViewPhase::Lobby => "Lobby".to_string(),
        ViewPhase::Planning { .. } if app.show.is_some() => "Fight".to_string(),
        ViewPhase::Planning { .. } => {
            let left = app.countdown.0 as f64 / 1000.0 - (get_time() - app.countdown.1);
            format!("Planning  {:.0}s", left.max(0.0))
        }
        ViewPhase::Showing { .. } => "Fight".to_string(),
        ViewPhase::Over { .. } => "Game over".to_string(),
    };
    text_centered(&phase, w / 2.0, 29.0, 24.0, ACCENT);
    if let Some(me) = app.me() {
        let xp_needed = app
            .state
            .content
            .as_ref()
            .and_then(|c| c.rules().levels.get(me.level as usize - 1))
            .map(|l| l.xp_to_next)
            .unwrap_or(0);
        let x = w - 520.0;
        label(&format!("Health {}", me.health.max(0)), x, 29.0, 22.0, if me.health > 30 { GOOD } else { BAD });
        label(&format!("Gold {}", me.gold), x + 140.0, 29.0, 22.0, GOLD_COLOR);
        label(&format!("Level {}  ({}/{} xp)", me.level, me.xp, xp_needed), x + 250.0, 29.0, 22.0, TEXT);
    } else {
        label("Spectating", w - 140.0, 29.0, 22.0, MUTED);
    }
}

fn draw_traits(app: &App, layout: &Layout) {
    panel(0.0, 44.0, layout.left_w, screen_height() - 44.0, PANEL);
    label("Traits", 14.0, 74.0, 22.0, TEXT);
    let (Some(player), Some(content)) = (app.shown_player(), &app.state.content) else { return };
    let units: Vec<BoardUnit> = player.board_units();
    let Ok(mut traits) = content.active_traits(&units) else { return };
    traits.sort_by_key(|t| (t.tier.is_none(), std::cmp::Reverse(t.count)));
    let mut y = 104.0;
    for t in traits {
        let Some(def) = content.trait_def(&t.id) else { continue };
        let steps: Vec<String> = def.breakpoints.iter().map(|b| b.count.to_string()).collect();
        let color = if t.tier.is_some() { GOLD_COLOR } else { MUTED };
        label(&format!("{}  {}", t.count, def.name), 14.0, y, 20.0, color);
        label(&format!("({})", steps.join(" / ")), 150.0, y, 16.0, MUTED);
        y += 26.0;
    }
    if !player.augments.is_empty() {
        y += 14.0;
        label("Augments", 14.0, y, 22.0, TEXT);
        y += 28.0;
        for a in &player.augments {
            label(&app.state.display_name(a), 14.0, y, 18.0, ACCENT);
            y += 22.0;
        }
    }
}

fn draw_scoreboard(app: &mut App, layout: &Layout) {
    let x = screen_width() - layout.right_w;
    panel(x, 44.0, layout.right_w, screen_height() - 44.0, PANEL);
    label("Players", x + 14.0, 74.0, 22.0, TEXT);
    let Some(view) = &app.state.view else { return };
    let mut players: Vec<&Player> = view.players.iter().collect();
    players.sort_by_key(|p| (!p.alive, std::cmp::Reverse(p.health), p.placement));
    let mut y = 88.0;
    let mut clicked = None;
    let (mx, my) = mouse_position();
    for p in players {
        let me = app.state.seat == Some(p.id);
        let shown = app.shown_player().map(|s| s.id) == Some(p.id);
        let hover = mx >= x + 8.0 && mx <= x + layout.right_w - 8.0 && my >= y && my <= y + 34.0;
        panel(x + 8.0, y, layout.right_w - 16.0, 34.0, if shown || hover { PANEL_LIGHT } else { PANEL });
        let color = if !p.alive { MUTED } else if me { GOLD_COLOR } else { TEXT };
        label(&p.name, x + 16.0, y + 16.0, 18.0, color);
        if p.alive {
            let frac = (p.health.max(0) as f32 / 100.0).min(1.0);
            draw_rectangle(x + 16.0, y + 22.0, 150.0, 6.0, BG);
            draw_rectangle(x + 16.0, y + 22.0, 150.0 * frac, 6.0, if p.health > 30 { GOOD } else { BAD });
            label(&format!("{}", p.health.max(0)), x + 174.0, y + 30.0, 16.0, TEXT);
            label(&format!("L{}", p.level), x + 206.0, y + 30.0, 16.0, MUTED);
            if p.ready {
                label("ok", x + 174.0, y + 14.0, 14.0, GOOD);
            }
        } else if let Some(place) = p.placement {
            label(&format!("out, place {place}"), x + 130.0, y + 16.0, 16.0, MUTED);
        }
        if hover && is_mouse_button_pressed(MouseButton::Left) {
            clicked = Some(p.id);
        }
        y += 38.0;
    }
    if let Some(id) = clicked {
        app.viewing = if Some(id) == app.state.seat { None } else { Some(id) };
    }

    y += 12.0;
    label("Log", x + 14.0, y + 14.0, 22.0, TEXT);
    y += 24.0;
    let room = ((screen_height() - y - 8.0) / 18.0) as usize;
    let skip = app.state.log.len().saturating_sub(room);
    for line in app.state.log.iter().skip(skip) {
        let mut line = line.clone();
        if measure_text(&line, None, 14, 1.0).width > layout.right_w - 24.0 {
            while measure_text(&line, None, 14, 1.0).width > layout.right_w - 36.0 && line.len() > 4 {
                line.pop();
            }
            line.push_str("...");
        }
        y += 18.0;
        label(&line, x + 14.0, y, 14.0, MUTED);
    }
}

/// The hex grid itself. `own_half` highlights the rows a player may use.
fn draw_board(layout: &Layout, own_half: bool) {
    for row in 0..layout.rows {
        for col in 0..layout.cols {
            let c = layout.hex_center(col, row);
            let mine = row >= layout.rows / 2;
            let fill = if own_half && mine { Color::new(0.17, 0.22, 0.30, 1.0) } else { Color::new(0.13, 0.15, 0.20, 1.0) };
            draw_poly(c.x, c.y, 6, layout.hex - 1.5, 30.0, fill);
            draw_poly_lines(c.x, c.y, 6, layout.hex - 1.5, 30.0, 1.5, Color::new(0.26, 0.30, 0.40, 1.0));
        }
    }
}

fn unit_label(app: &App, unit: &OwnedUnit) -> String {
    app.state.display_name(&unit.unit)
}

fn draw_planning(app: &mut App, layout: &Layout) {
    let Some(player) = app.shown_player().cloned() else { return };
    let own = app.viewing_self() && player.alive;
    let (mx, my) = mouse_position();
    let mouse = vec2(mx, my);
    draw_board(layout, true);
    if !own {
        text_centered(&format!("{}'s board", player.name), layout.board_mid_x, layout.board_top - 6.0, 20.0, MUTED);
    }

    let dragged = app.drag;
    let mut hovered: Option<OwnedUnit> = None;
    let mut pressed_unit = None;

    // Units on the board.
    for (pos, unit) in &player.board {
        let c = layout.hex_center(pos.0, pos.1);
        if dragged == Some(Drag::Unit(unit.uid)) {
            continue;
        }
        draw_unit_token(c, layout.hex * 0.62, &unit_label(app, unit), unit.star, unit.items.len(), true, None);
        if mouse.distance(c) < layout.hex * 0.7 {
            hovered = Some(unit.clone());
            if is_mouse_button_pressed(MouseButton::Left) {
                pressed_unit = Some(unit.uid);
            }
        }
    }

    // Bench.
    for i in 0..layout.bench_slots {
        let r = layout.bench_slot(i);
        panel(r.x, r.y, r.w, r.h, PANEL);
        if let Some(Some(unit)) = player.bench.get(i) {
            if dragged == Some(Drag::Unit(unit.uid)) {
                continue;
            }
            let c = r.center();
            draw_unit_token(c, r.w * 0.40, &unit_label(app, unit), unit.star, unit.items.len(), true, None);
            if r.contains(mouse) {
                hovered = Some(unit.clone());
                if is_mouse_button_pressed(MouseButton::Left) {
                    pressed_unit = Some(unit.uid);
                }
            }
        }
    }
    label("Bench", layout.bench_slot(0).x - 62.0, layout.bench_slot(0).y + 34.0, 18.0, MUTED);

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
        label("Items", layout.item_slot(0).x - 62.0, layout.item_slot(0).y + 24.0, 18.0, MUTED);
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
            if let Some(unit) = player.unit(uid) {
                draw_unit_token(mouse, layout.hex * 0.62, &unit_label(app, unit), unit.star, unit.items.len(), true, None);
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

fn draw_item(app: &App, item: &str, r: Rect) {
    let name = app.state.display_name(item);
    draw_rectangle(r.x, r.y, r.w, r.h, unit_color(&name));
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, GOLD_COLOR);
    let initials: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect();
    text_centered(&initials, r.x + r.w / 2.0, r.y + r.h / 2.0 + 6.0, 18.0, WHITE);
}

fn draw_unit_tooltip(app: &App, unit: &OwnedUnit, at: Vec2) {
    let Some(content) = &app.state.content else { return };
    let Some(def) = content.unit(&unit.unit) else { return };
    let mut lines = vec![format!("{}  ({} star, cost {})", def.name, unit.star, def.cost)];
    let traits: Vec<String> = def.traits.iter().map(|t| app.state.display_name(t)).collect();
    if !traits.is_empty() {
        lines.push(traits.join(", "));
    }
    if let Some(a) = &def.ability {
        lines.push(format!("Ability: {}", a.name));
    }
    for item in &unit.items {
        lines.push(format!("Item: {}", app.state.display_name(item)));
    }
    let width = lines.iter().map(|l| measure_text(l, None, 17, 1.0).width).fold(0.0, f32::max) + 20.0;
    let height = lines.len() as f32 * 20.0 + 12.0;
    let x = (at.x + 16.0).min(screen_width() - width - 4.0);
    let y = (at.y + 16.0).min(screen_height() - height - 4.0);
    draw_rectangle(x, y, width, height, Color::new(0.05, 0.06, 0.08, 0.95));
    draw_rectangle_lines(x, y, width, height, 1.0, MUTED);
    for (i, line) in lines.iter().enumerate() {
        label(line, x + 10.0, y + 22.0 + i as f32 * 20.0, 17.0, if i == 0 { TEXT } else { MUTED });
    }
}

fn draw_shop(app: &mut App, layout: &Layout, player: &Player) {
    let Some(content) = &app.state.content else { return };
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
        panel(r.x, r.y, r.w, r.h, if hover { PANEL_LIGHT } else { PANEL });
        draw_rectangle(r.x, r.y, r.w, 5.0, unit_color(&def.name));
        draw_unit_token(vec2(r.x + 28.0, r.y + 38.0), 18.0, &def.name, 1, 0, true, None);
        label(&def.name, r.x + 8.0, r.y + 76.0, 16.0, if affordable { TEXT } else { MUTED });
        let traits: Vec<String> = def.traits.iter().map(|t| app.state.display_name(t)).collect();
        label(&traits.join(", "), r.x + 8.0, r.y + 94.0, 14.0, MUTED);
        label(&format!("{}g", def.cost), r.x + r.w - 30.0, r.y + 42.0, 20.0, GOLD_COLOR);
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
fn handle_drop(app: &mut App, layout: &Layout, player: &Player, mouse: Vec2) {
    if !is_mouse_button_released(MouseButton::Left) {
        return;
    }
    let Some(drag) = app.drag.take() else { return };
    let over_hex = layout.hex_at(mouse);
    let over_bench = (0..layout.bench_slots).find(|i| layout.bench_slot(*i).contains(mouse));
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
fn draw_choice(app: &mut App, layout: &Layout) {
    let Some(view) = &app.state.view else { return };
    if view.options.is_empty() || !matches!(view.phase, ViewPhase::Planning { .. }) {
        return;
    }
    let Some(me) = app.state.seat else { return };
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
            Some(p) => format!("Waiting for {} to choose", app.state.player_name(*p)),
            None => return,
        }
    };
    let n = options.len() as f32;
    let (cw, ch, gap) = (176.0, 120.0, 10.0);
    let per_row = 4.0_f32.min(n);
    let rows = (n / per_row).ceil();
    let total_w = per_row * cw + (per_row - 1.0) * gap;
    let x0 = layout.board_mid_x - total_w / 2.0;
    let y0 = layout.board_top + 40.0;
    draw_rectangle(x0 - 16.0, y0 - 44.0, total_w + 32.0, rows * (ch + gap) + 56.0, Color::new(0.04, 0.05, 0.07, 0.94));
    text_centered(&title, layout.board_mid_x, y0 - 14.0, 24.0, if my_turn { GOLD_COLOR } else { MUTED });
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
            text_centered("taken", r.x + r.w / 2.0, r.y + r.h / 2.0, 18.0, MUTED);
            continue;
        };
        let hover = my_turn && r.contains(vec2(mx, my));
        panel(r.x, r.y, r.w, r.h, if hover { PANEL_LIGHT } else { PANEL });
        let parts: Vec<&str> = option.split(" + ").collect();
        let name = app.state.display_name(parts[0]);
        draw_rectangle(r.x, r.y, r.w, 5.0, unit_color(&name));
        label(&name, r.x + 10.0, r.y + 30.0, 18.0, TEXT);
        let content = app.state.content.as_ref();
        if let Some(item) = parts.get(1) {
            label(&format!("with {}", app.state.display_name(item)), r.x + 10.0, r.y + 52.0, 15.0, GOLD_COLOR);
        } else if let Some(aug) = content.and_then(|c| c.augment(parts[0])) {
            draw::wrapped(&aug.description, r.x + 10.0, r.y + 52.0, r.w - 20.0, 15.0, MUTED);
        }
        if hover && is_mouse_button_pressed(MouseButton::Left) {
            pick = Some(i);
        }
    }
    if let Some(option) = pick {
        app.command(Command::Pick { option });
    }
}

fn draw_replay(app: &mut App, layout: &Layout) {
    let Some(show) = &app.show else { return };
    draw_board(layout, false);
    text_centered(&show.title, layout.board_mid_x, layout.board_top - 6.0, 22.0, TEXT);
    let now = show.playback.time_ms();
    let my_team = if show.flipped { 1 } else { 0 };
    let place = |q: f32, r: f32| {
        let p = layout.axial_center(q, r);
        if show.flipped { layout.flip(p) } else { p }
    };
    let pos_of = |id| {
        show.playback.unit(id).map(|u| {
            let (q, r) = u.position_at(now);
            place(q, r)
        })
    };

    for u in show.playback.units().filter(|u| u.alive) {
        let (q, r) = u.position_at(now);
        let c = place(q, r);
        let health = (u.health as f32 / u.max_health.max(1) as f32, u.shield as f32 / u.max_health.max(1) as f32);
        draw_unit_token(c, layout.hex * 0.60, &u.name, 0, 0, u.team.0 == my_team, Some(health));
        if u.casting {
            draw_circle_lines(c.x, c.y, layout.hex * 0.72, 3.0, ACCENT);
        }
        if u.stunned_until_ms > now {
            text_centered("stun", c.x, c.y - layout.hex * 0.95, 14.0, GOLD_COLOR);
        }
    }

    for (age, flash) in show.playback.recent_flashes(900) {
        let age = age as f32;
        match flash {
            Flash::Attack { source, target } if age < 160.0 => {
                if let (Some(a), Some(b)) = (pos_of(*source), pos_of(*target)) {
                    let alpha = 1.0 - age / 160.0;
                    draw_line(a.x, a.y, b.x, b.y, 2.5, Color::new(1.0, 1.0, 1.0, alpha * 0.8));
                }
            }
            Flash::Damage { target, amount, absorbed, kind, crit } => {
                if let Some(p) = pos_of(*target) {
                    let alpha = (1.0 - age / 900.0).max(0.0);
                    let mut color = match kind {
                        autoengine::effect::DamageKind::Physical => Color::new(1.0, 0.55, 0.30, alpha),
                        autoengine::effect::DamageKind::Magic => Color::new(0.55, 0.65, 1.0, alpha),
                        autoengine::effect::DamageKind::True => Color::new(1.0, 1.0, 1.0, alpha),
                    };
                    let shown = amount + absorbed;
                    if *amount == 0 {
                        color = Color::new(0.8, 0.8, 0.8, alpha);
                    }
                    // Spread numbers out a little so simultaneous hits stay readable.
                    let jitter = ((shown * 37) % 56) as f32 - 28.0;
                    let size = if *crit { 24.0 } else { 17.0 };
                    text_centered(&shown.to_string(), p.x + jitter, p.y - layout.hex * 0.9 - age * 0.03, size, color);
                }
            }
            Flash::Heal { target, amount } => {
                if let Some(p) = pos_of(*target) {
                    let alpha = (1.0 - age / 900.0).max(0.0);
                    text_centered(&format!("+{amount}"), p.x, p.y + layout.hex * 0.2 - age * 0.03, 17.0, Color::new(0.4, 1.0, 0.5, alpha));
                }
            }
            Flash::Cast { unit } if age < 450.0 => {
                if let Some(p) = pos_of(*unit) {
                    let t = age / 450.0;
                    draw_circle_lines(p.x, p.y, layout.hex * (0.7 + t * 1.2), 3.0, Color::new(0.35, 0.62, 0.98, 1.0 - t));
                }
            }
            Flash::Death { unit } if age < 500.0 => {
                if let Some(p) = pos_of(*unit) {
                    let alpha = 1.0 - age / 500.0;
                    let s = layout.hex * 0.4;
                    let c = Color::new(0.9, 0.3, 0.3, alpha);
                    draw_line(p.x - s, p.y - s, p.x + s, p.y + s, 4.0, c);
                    draw_line(p.x - s, p.y + s, p.x + s, p.y - s, 4.0, c);
                }
            }
            _ => {}
        }
    }

    let secs = (now as f32 / 1000.0).min(show.playback.duration_ms() as f32 / 1000.0);
    let footer_y = layout.bench_slot(0).y + 30.0;
    if show.playback.is_finished() {
        let result = match show.playback.winner() {
            Some(team) if app.state.seat.is_none() => format!("Team {} wins", team.0 + 1),
            Some(team) if team.0 == my_team => "Victory".to_string(),
            Some(_) => "Defeat".to_string(),
            None => "Draw".to_string(),
        };
        text_centered(&result, layout.board_mid_x, footer_y, 36.0, GOLD_COLOR);
    } else {
        text_centered(&format!("{secs:.1}s   (Space to skip)"), layout.board_mid_x, footer_y, 18.0, MUTED);
    }
}

fn draw_game_over(app: &App) {
    let Some(view) = &app.state.view else { return };
    let ViewPhase::Over { winner } = view.phase else { return };
    if app.show.is_some() {
        return;
    }
    let (w, h) = (screen_width(), screen_height());
    draw_rectangle(w / 2.0 - 240.0, h / 2.0 - 200.0, 480.0, 400.0, Color::new(0.04, 0.05, 0.07, 0.96));
    let title = match winner {
        Some(id) if Some(id) == app.state.seat => "You win!".to_string(),
        Some(id) => format!("{} wins", app.state.player_name(id)),
        None => "Game over".to_string(),
    };
    text_centered(&title, w / 2.0, h / 2.0 - 150.0, 40.0, GOLD_COLOR);
    let mut players: Vec<&Player> = view.players.iter().collect();
    players.sort_by_key(|p| p.placement.unwrap_or(u32::MAX));
    for (i, p) in players.iter().enumerate() {
        let y = h / 2.0 - 100.0 + i as f32 * 30.0;
        let color = if Some(p.id) == app.state.seat { GOLD_COLOR } else { TEXT };
        label(&format!("{}.", p.placement.unwrap_or(0)), w / 2.0 - 180.0, y, 22.0, MUTED);
        label(&p.name, w / 2.0 - 140.0, y, 22.0, color);
        label(&format!("level {}", p.level), w / 2.0 + 80.0, y, 18.0, MUTED);
    }
}

/// Converts an axial hex to the `(q, r)` floats the layout works in.
#[allow(dead_code)]
fn axial(hex: Hex) -> (f32, f32) {
    (hex.q as f32, hex.r as f32)
}
