//! The graphical client.
//!
//! This program contains no game rules. It connects to an
//! `autoengine-server`, draws what the server reports and sends the player's
//! clicks back as commands. Rendering goes through OpenGL (via macroquad).
//!
//! The board is a 3D scene: units are models loaded through Assimp from
//! the files the content names (see `model.rs`), with a stand-in figure for
//! units that have none. The rest of the interface is drawn flat on top.
//!
//! Usage: `autoengine-client [--addr 127.0.0.1:7777] [--name NAME] [--spectate]
//!         [--assets DIR] [--settings FILE] [--replay FILE] [--autoplay] [--no-ready]
//!         [--menu] [--stats] [--screenshot SECONDS:FILE]...`
//!
//! `--replay FILE` watches a fight the server saved (`--record`) without
//! connecting to anything. Settings live in `autoengine-client.ron` (see
//! `settings.rs`) and can be changed from the menu.
//!
//! Controls: drag units between the bench and your half of the board, drag a
//! unit onto the shop to sell it, drag items onto units, click shop cards to
//! buy. Click a name on the scoreboard to look at that player's board.
//! Space skips a fight replay. Drag with the right mouse button to turn the
//! camera, scroll to zoom, Home to reset it. Escape opens the menu and F3
//! shows frame-rate and scene statistics.

mod draw;
mod fx;
mod model;
mod scene;
mod settings;
mod sound;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;

use autoengine::client::ClientState;
use autoengine::combat::UnitId;
use autoengine::content::{BoardUnit, UnitDef};
use autoengine::game::{Command, Opponent, OwnedUnit, Player, PlayerId, Slot};
use autoengine::hex::Hex;
use autoengine::net::Connection;
use autoengine::playback::{Flash, Playback};
use autoengine::protocol::{ClientMsg, PROTOCOL_VERSION, ViewPhase};
use macroquad::prelude::*;

use draw::{Layout, button, draw_health_bar, draw_pips, draw_unit_token, label, panel, text_centered, unit_color};
use model::{Model, Pose, RawModel};
use scene::Ground;
use settings::Settings;
use sound::{Sfx, Sounds};
use autoengine::replay::ReplayFile;

const BG: Color = Color::new(0.07, 0.08, 0.11, 1.0);
const PANEL: Color = Color::new(0.12, 0.14, 0.19, 1.0);
const PANEL_LIGHT: Color = Color::new(0.18, 0.21, 0.28, 1.0);
const TEXT: Color = Color::new(0.90, 0.92, 0.96, 1.0);
const MUTED: Color = Color::new(0.58, 0.62, 0.70, 1.0);
const GOLD_COLOR: Color = Color::new(0.98, 0.80, 0.25, 1.0);
const GOOD: Color = Color::new(0.35, 0.82, 0.45, 1.0);
const BAD: Color = Color::new(0.92, 0.33, 0.33, 1.0);
const ACCENT: Color = Color::new(0.35, 0.62, 0.98, 1.0);

#[derive(Clone)]
struct Options {
    /// Server address and player name; the settings file fills these in
    /// when the command line does not.
    addr: Option<String>,
    name: Option<String>,
    settings: PathBuf,
    /// A saved fight to watch instead of connecting to a server.
    replay: Option<PathBuf>,
    /// Start with the menu open, or with the statistics overlay on.
    menu: bool,
    stats: bool,
    spectate: bool,
    /// Folder that model files named by the content are looked up in.
    assets: PathBuf,
    /// Play on its own: start the game, buy, place units and ready up.
    autoplay: bool,
    /// With `autoplay`, leave readying up to the round timer.
    no_ready: bool,
    /// `(seconds after launch, file)` pairs.
    screenshots: Vec<(f64, String)>,
    quit_after_screenshots: bool,
}

fn parse_options() -> Result<Options, String> {
    let usage = "usage: autoengine-client [--addr HOST:PORT] [--name NAME] [--spectate] [--assets DIR] [--settings FILE] [--replay FILE] [--autoplay] \
                 [--no-ready] [--menu] [--stats] [--screenshot SECONDS:FILE]...";
    let mut o = Options {
        addr: None,
        name: None,
        settings: PathBuf::from("autoengine-client.ron"),
        replay: None,
        menu: false,
        stats: false,
        spectate: false,
        assets: PathBuf::from("content"),
        autoplay: false,
        no_ready: false,
        screenshots: Vec::new(),
        quit_after_screenshots: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(usage.to_string());
        match arg.as_str() {
            "--addr" => o.addr = Some(value()?),
            "--name" => o.name = Some(value()?),
            "--settings" => o.settings = PathBuf::from(value()?),
            "--replay" => o.replay = Some(PathBuf::from(value()?)),
            "--menu" => o.menu = true,
            "--stats" => o.stats = true,
            "--spectate" => o.spectate = true,
            "--assets" => o.assets = PathBuf::from(value()?),
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

/// Options, settings and any problem reading the settings file; or the
/// usage text when the command line is wrong.
type Startup = Result<(Options, Settings, Option<String>), String>;

/// The command line and the settings file, read once: the window needs
/// them before `main` starts.
fn startup() -> &'static Startup {
    static STARTUP: OnceLock<Startup> = OnceLock::new();
    STARTUP.get_or_init(|| {
        let options = parse_options()?;
        let (settings, problem) = Settings::load(&options.settings);
        Ok((options, settings, problem))
    })
}

fn window_conf() -> Conf {
    let settings = startup().as_ref().map(|s| s.1.clone()).unwrap_or_default();
    Conf {
        window_title: "AutoEngine".to_owned(),
        window_width: settings.window_width,
        window_height: settings.window_height,
        fullscreen: settings.fullscreen,
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
    /// Which way each unit is turned, as a yaw angle.
    facing: HashMap<UnitId, f32>,
    /// Replay time up to which effects and sounds have been set off.
    fx_seen_ms: u32,
    /// Whether the result has been announced with a sound.
    announced: bool,
}

impl Show {
    fn new(playback: Playback, flipped: bool, title: String) -> Self {
        Show { playback, flipped, time_ms: 0.0, title, done_for_ms: 0.0, facing: HashMap::new(), fx_seen_ms: 0, announced: false }
    }
}

/// Height of a unit at 100% scale, in hex radii.
const UNIT_HEIGHT: f32 = 1.7;
const FRIEND: Color = Color::new(0.35, 0.62, 0.98, 1.0);
const ENEMY: Color = Color::new(0.92, 0.33, 0.33, 1.0);

/// How to draw one kind of unit.
#[derive(Clone)]
struct Look {
    model: Rc<Model>,
    scale: f32,
    yaw: f32,
    /// Whether to play animation clips (a setting).
    animate: bool,
}

impl Look {
    fn height(&self) -> f32 {
        self.model.height * self.scale
    }

    /// A moment of the named clip, if the model has it.
    fn pose(&self, clip: &str, time: f32, looped: bool) -> Option<Pose> {
        self.model.clip(clip).filter(|_| self.animate).map(|clip| Pose { clip, time, looped })
    }
}

/// Models loaded so far. Each file is read once, the first time a unit
/// that uses it comes on screen.
struct Models {
    assets: PathBuf,
    files: HashMap<String, Option<Rc<Model>>>,
    figures: HashMap<String, Rc<Model>>,
}

impl Models {
    fn look(&mut self, def: Option<&UnitDef>, name: &str) -> Look {
        let wanted = def.and_then(|d| d.model.as_ref());
        let loaded = wanted.and_then(|m| {
            let assets = &self.assets;
            self.files
                .entry(m.file.clone())
                .or_insert_with(|| match model::load_geometry(&assets.join(&m.file)) {
                    Ok(raw) => Some(Rc::new(Model::upload(raw))),
                    Err(e) => {
                        eprintln!("model {e}; drawing a stand-in");
                        None
                    }
                })
                .clone()
        });
        let model = loaded.unwrap_or_else(|| {
            self.figures
                .entry(name.to_string())
                .or_insert_with(|| Rc::new(Model::upload(RawModel::figure(unit_color(name)))))
                .clone()
        });
        Look {
            model,
            scale: UNIT_HEIGHT * wanted.map_or(100, |m| m.scale_percent) as f32 / 100.0,
            yaw: (wanted.map_or(0, |m| m.yaw_degrees) as f32).to_radians(),
            animate: true,
        }
    }
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
    models: RefCell<Models>,
    /// Camera angle around the board, and distance as a multiple of the
    /// distance that just fits the board.
    camera_yaw: f32,
    camera_zoom: f32,
    settings: Settings,
    particles: fx::Particles,
    sounds: Sounds,
    menu_open: bool,
    /// A saved fight being watched instead of a live game.
    replay: Option<ReplayFile>,
    /// Units and vertices drawn this frame, for the statistics overlay.
    drawn: Cell<(usize, usize)>,
    /// Recent frame times in milliseconds.
    frame_ms: Vec<f32>,
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
        let addr = self.options.addr.clone().unwrap_or_else(|| self.settings.addr.clone());
        match Connection::connect(&addr) {
            Ok(conn) => {
                self.conn = Some(conn);
                self.connect_error = None;
                let hello = if self.options.spectate {
                    ClientMsg::Spectate { version: PROTOCOL_VERSION }
                } else {
                    ClientMsg::Join {
                        name: self.options.name.clone().unwrap_or_else(|| self.settings.name.clone()),
                        version: PROTOCOL_VERSION,
                        token: self.state.token.clone(),
                    }
                };
                self.send(hello);
            }
            Err(e) => {
                self.conn = None;
                self.connect_error = Some(format!("cannot reach the server at {addr}: {e}"));
            }
        }
    }

    /// The look of a unit given its content id.
    fn look_of(&self, unit_id: &str) -> Look {
        let def = self.state.content.as_ref().and_then(|c| c.unit(unit_id));
        let name = def.map_or(unit_id, |d| d.name.as_str());
        let mut look = self.models.borrow_mut().look(def, name);
        look.animate = self.settings.animations;
        look
    }

    /// The look of a unit given the name fights report it under.
    fn look_of_name(&self, name: &str) -> Look {
        let def = self.state.content.as_ref().and_then(|c| c.units().find(|u| u.name == name));
        let mut look = self.models.borrow_mut().look(def, name);
        look.animate = self.settings.animations;
        look
    }

    /// Draws a unit's model and counts it for the statistics overlay.
    #[allow(clippy::too_many_arguments)]
    fn draw_model(&self, look: &Look, at: Vec3, yaw: f32, scale: f32, tint: [f32; 3], flash: f32, pose: Option<Pose>) {
        let (units, vertices) = self.drawn.get();
        self.drawn.set((units + 1, vertices + look.model.vertex_count()));
        look.model.draw(at, yaw + look.yaw, scale, tint, flash, pose);
    }

    fn play(&mut self, sfx: Sfx) {
        self.sounds.play(sfx, self.settings.volume);
    }

    /// Writes the settings file after a change.
    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save(&self.options.settings) {
            self.connect_error = Some(format!("could not save {}: {e}", self.options.settings.display()));
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
            self.show = Some(Show::new(Playback::new(&record), flipped, title));
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
    let (options, mut settings, problem) = match startup() {
        Ok(start) => start.clone(),
        Err(usage) => {
            eprintln!("{usage}");
            return;
        }
    };
    if let Some(problem) = &problem {
        eprintln!("{problem}");
    }
    settings.show_stats |= options.stats;
    let replay = match options.replay.as_deref().map(ReplayFile::load) {
        Some(Err(e)) => {
            eprintln!("{e}");
            return;
        }
        Some(Ok(replay)) => Some(replay),
        None => None,
    };
    let mut app = App {
        settings,
        particles: fx::Particles::default(),
        sounds: Sounds::load().await,
        menu_open: options.menu,
        replay,
        drawn: Cell::new((0, 0)),
        frame_ms: Vec::new(),
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
        models: RefCell::new(Models { assets: options.assets.clone(), files: HashMap::new(), figures: HashMap::new() }),
        camera_yaw: 0.0,
        camera_zoom: 1.0,
        options,
    };
    if let Some(replay) = &app.replay {
        match replay.content.clone().into_content() {
            Ok(content) => app.state.content = Some(content),
            Err(e) => {
                eprintln!("the replay's content is invalid: {e:?}");
                return;
            }
        }
    } else {
        app.connect();
    }
    app.connect_error = app.connect_error.take().or(problem);
    // Room for a whole model part in one draw call.
    macroquad::window::gl_set_drawcall_buffer_capacity(model::MAX_PART_VERTICES + 16, model::MAX_PART_INDICES + 16);

    let mut shots = std::mem::take(&mut app.options.screenshots);
    shots.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut shots = shots.into_iter().peekable();

    loop {
        if app.replay.is_none() {
            app.pump_network();
            if app.options.autoplay {
                app.autoplay();
            }
        }
        clear_background(BG);
        app.drawn.set((0, 0));
        app.frame_ms.push(get_frame_time() * 1000.0);
        if app.frame_ms.len() > 120 {
            app.frame_ms.remove(0);
        }
        app.particles.update(get_frame_time());
        if is_key_pressed(KeyCode::Escape) {
            app.menu_open = !app.menu_open;
        }
        if is_key_pressed(KeyCode::F3) {
            app.settings.show_stats = !app.settings.show_stats;
            app.save_settings();
        }
        // Camera: right-drag turns it, the wheel zooms, Home resets.
        let turn = mouse_delta_position();
        if is_mouse_button_down(MouseButton::Right) {
            app.camera_yaw += turn.x * 2.5 * app.settings.camera_turn_percent as f32 / 100.0;
        }
        let wheel = mouse_wheel().1;
        if wheel != 0.0 {
            app.camera_zoom = (app.camera_zoom - wheel.signum() * 0.08).clamp(0.55, 1.8);
        }
        if is_key_pressed(KeyCode::Home) {
            (app.camera_yaw, app.camera_zoom) = (0.0, 1.0);
        }
        let layout = Layout::new(&app);
        if app.menu_open {
            // The game carries on underneath; the menu has the screen.
            draw_menu(&mut app);
        } else if app.replay.is_some() {
            draw_replay_viewer(&mut app, &layout);
        } else if app.state.view.is_none() {
            draw_lobby(&mut app);
        } else {
            draw_game(&mut app, &layout);
        }
        if app.settings.show_stats && !app.menu_open {
            draw_stats(&app, &layout);
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

/// Moves the fight on screen forward by one frame, and ends it when it has
/// been over for a moment or Space skips it.
fn advance_show(app: &mut App) {
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
        app.particles.clear();
    }
}

/// Watching a saved fight: no server, no shop, just the replay on a loop.
fn draw_replay_viewer(app: &mut App, layout: &Layout) {
    let Some(replay) = &app.replay else { return };
    let (title, events) = (replay.title.clone(), replay.record.events.len());
    if app.show.is_none() {
        app.show = Some(Show::new(Playback::new(&replay.record), false, title.clone()));
    }
    let (w, h) = (screen_width(), screen_height());
    panel(0.0, 0.0, w, 44.0, PANEL);
    label("Replay", 16.0, 29.0, 22.0, TEXT);
    text_centered(&title, w / 2.0, 29.0, 22.0, ACCENT);
    panel(0.0, 44.0, layout.left_w, h - 44.0, PANEL);
    panel(w - layout.right_w, 44.0, layout.right_w, h - 44.0, PANEL);
    let x = w - layout.right_w + 14.0;
    label("Saved fight", x, 74.0, 22.0, TEXT);
    let file = app.options.replay.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    draw::wrapped(&file, x, 86.0, layout.right_w - 28.0, 16.0, MUTED);
    label(&format!("{events} events"), x, 150.0, 16.0, MUTED);
    for (i, line) in ["Space: restart", "Right-drag: turn the camera", "Scroll: zoom", "Esc: menu", "F3: statistics"].iter().enumerate() {
        label(line, x, 190.0 + i as f32 * 22.0, 16.0, MUTED);
    }
    advance_show(app);
    draw_replay(app, layout);
}

/// The menu: settings that take effect at once and are saved to the file.
fn draw_menu(app: &mut App) {
    let (w, h) = (screen_width(), screen_height());
    draw_rectangle(0.0, 0.0, w, h, Color::new(0.03, 0.04, 0.06, 0.82));
    let (pw, ph) = (460.0, 470.0);
    let (x, y) = (w / 2.0 - pw / 2.0, h / 2.0 - ph / 2.0);
    panel(x, y, pw, ph, PANEL);
    draw_rectangle_lines(x, y, pw, ph, 1.0, MUTED);
    text_centered("Menu", w / 2.0, y + 40.0, 30.0, TEXT);

    let before = app.settings.clone();
    let on_off = |on: bool| if on { "On" } else { "Off" };
    let mut row = 0.0;
    let mut toggle = |name: &str, value: &mut bool| {
        let ry = y + 70.0 + row * 46.0;
        row += 1.0;
        label(name, x + 30.0, ry + 25.0, 20.0, TEXT);
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
        label(name, x + 30.0, ry + 25.0, 20.0, TEXT);
        let minus = button("-", x + pw - 150.0, ry, 36.0, 36.0, true);
        text_centered(&shown, x + pw - 90.0, ry + 25.0, 20.0, TEXT);
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
    let by = y + ph - 58.0;
    if button("Resume", x + 30.0, by, 190.0, 40.0, true) {
        app.menu_open = false;
    }
    if button("Quit", x + pw - 220.0, by, 190.0, 40.0, true) {
        macroquad::miniquad::window::order_quit();
    }
}

/// Frame rate and what the scene is costing, in the corner of the 3D view.
fn draw_stats(app: &App, layout: &Layout) {
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
        label(line, x + 8.0, y + 20.0 + i as f32 * 18.0, 16.0, GOOD);
    }
}

fn draw_game(app: &mut App, layout: &Layout) {
    draw_top_bar(app);
    draw_traits(app, layout);
    draw_scoreboard(app, layout);

    advance_show(app);

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
/// Queues the board's tiles. `own_half` tints the near half, where the
/// viewer may place units; `highlight` marks the hex under a dragged unit.
fn board_tiles(ground: &mut Ground, layout: &Layout, own_half: bool, highlight: Option<(i32, i32)>) {
    for row in 0..layout.rows {
        for col in 0..layout.cols {
            let mine = own_half && row >= layout.rows / 2;
            let mut fill = if mine { Color::new(0.24, 0.31, 0.43, 1.0) } else { Color::new(0.19, 0.22, 0.29, 1.0) };
            if (col + row) % 2 == 0 {
                fill = Color::new(fill.r * 0.93, fill.g * 0.93, fill.b * 0.93, 1.0);
            }
            if highlight == Some((col, row)) {
                fill = Color::new(0.45, 0.62, 0.85, 1.0);
            }
            ground.hex_tile(layout.hex_world(col, row), 0.94, 0.22, fill);
        }
    }
}

fn unit_label(app: &App, unit: &OwnedUnit) -> String {
    app.state.display_name(&unit.unit)
}

/// Stars make a unit a little bigger.
fn star_scale(star: u8) -> f32 {
    1.0 + 0.14 * star.saturating_sub(1) as f32
}

const RING_LIFT: Vec3 = Vec3::new(0.0, 0.03, 0.0);
const SHADOW: Color = Color::new(0.0, 0.0, 0.0, 0.38);

fn draw_planning(app: &mut App, layout: &Layout) {
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
        let fill = if drop_bench == Some(i) { Color::new(0.45, 0.62, 0.85, 1.0) } else { Color::new(0.16, 0.18, 0.24, 1.0) };
        ground.hex_tile(layout.bench_world(i), 0.72, 0.14, fill);
    }
    for (_, at, _) in &standing {
        if app.settings.shadows {
            ground.disc(*at + RING_LIFT * 0.5, 0.48, SHADOW);
        }
        ground.ring(*at + RING_LIFT, 0.50, 0.62, FRIEND);
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
        text_centered(&format!("{}'s board", player.name), layout.board_mid_x, view.area.y + 22.0, 20.0, MUTED);
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
        label("Bench", (first.x - 50.0).max(layout.left_w + 6.0), first.y + 5.0, 18.0, MUTED);
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
fn handle_drop(app: &mut App, layout: &Layout, player: &Player, mouse: Vec2) {
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
    let y0 = layout.view.area.y + 70.0;
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
    let Some(mut show) = app.show.take() else { return };
    let view = &layout.view;
    let now = show.playback.time_ms();
    let my_team = if show.flipped { 1 } else { 0 };
    let flipped = show.flipped;
    let place = |q: f32, r: f32| {
        let p = layout.axial_world(q, r);
        if flipped { layout.flip(p) } else { p }
    };
    let world_of = |playback: &Playback, id| {
        playback.unit(id).map(|u| {
            let (q, r) = u.position_at(now);
            place(q, r)
        })
    };
    let yaw_towards = |from: Vec3, to: Vec3| {
        let d = to - from;
        (d.x.abs() + d.z.abs() > 1e-3).then(|| d.x.atan2(d.z))
    };

    // Which way everyone faces: where they walk, then whom they hit.
    for u in show.playback.units().filter(|u| u.alive) {
        let ahead = if u.team.0 == my_team { std::f32::consts::PI } else { 0.0 };
        let facing = show.facing.entry(u.id).or_insert(ahead);
        if u.pos != u.prev_pos
            && now.saturating_sub(u.moved_at_ms) < autoengine::playback::MOVE_ANIMATION_MS
            && let Some(yaw) = yaw_towards(place(u.prev_pos.q as f32, u.prev_pos.r as f32), place(u.pos.q as f32, u.pos.r as f32))
        {
            *facing = yaw;
        }
    }
    // Attacks: turn to the target and lunge at it; hits flash white.
    let mut lunge: HashMap<UnitId, Vec3> = HashMap::new();
    let mut hit: HashMap<UnitId, f32> = HashMap::new();
    // Seconds since each unit last attacked.
    let mut attacked: HashMap<UnitId, f32> = HashMap::new();
    for (age, flash) in show.playback.recent_flashes(900) {
        match flash {
            Flash::Attack { source, target } => {
                if let (Some(a), Some(b)) = (world_of(&show.playback, *source), world_of(&show.playback, *target))
                    && let Some(yaw) = yaw_towards(a, b)
                {
                    let since = attacked.entry(*source).or_insert(f32::MAX);
                    *since = since.min(age as f32 / 1000.0);
                    if age < 400 {
                        show.facing.insert(*source, yaw);
                    }
                    if age < 180 {
                        let reach = (std::f32::consts::PI * age as f32 / 180.0).sin() * 0.35;
                        lunge.insert(*source, (b - a).normalize_or_zero() * reach);
                    }
                }
            }
            Flash::Damage { target, amount, .. } if age < 140 && *amount > 0 => {
                let strength = 0.55 * (1.0 - age as f32 / 140.0);
                let entry = hit.entry(*target).or_insert(0.0);
                *entry = entry.max(strength);
            }
            _ => {}
        }
    }

    // Set off particles and sounds for what happened since the last frame.
    let fresh = now.saturating_sub(show.fx_seen_ms).min(300);
    show.fx_seen_ms = now;
    let chest = vec3(0.0, 0.9, 0.0);
    let mut cues: Vec<Sfx> = Vec::new();
    for (age, flash) in show.playback.recent_flashes(fresh) {
        if age >= fresh {
            continue;
        }
        use autoengine::effect::DamageKind;
        use fx::Burst;
        let particles = app.settings.particles;
        match flash {
            Flash::Attack { source, target } => {
                if let (Some(a), Some(b)) = (world_of(&show.playback, *source), world_of(&show.playback, *target))
                    && a.distance(b) > 2.2
                    && particles
                {
                    app.particles.bolt(a + chest, b + chest, 0.12, Color::new(1.0, 0.85, 0.45, 1.0));
                }
            }
            Flash::Damage { target, amount, kind, crit, .. } => {
                cues.push(Sfx::Hit);
                if let Some(at) = world_of(&show.playback, *target).filter(|_| particles && *amount > 0) {
                    let color = match kind {
                        DamageKind::Physical => Color::new(1.0, 0.60, 0.25, 1.0),
                        DamageKind::Magic => Color::new(0.50, 0.65, 1.0, 1.0),
                        DamageKind::True => WHITE,
                    };
                    let count = if *crit { 16 } else { 7 };
                    app.particles.burst(at + chest, Burst { count, color, speed: 2.4, life: 0.45, size: 0.13, gravity: 6.0 });
                }
            }
            Flash::Heal { target, .. } => {
                cues.push(Sfx::Heal);
                if let Some(at) = world_of(&show.playback, *target).filter(|_| particles) {
                    let color = Color::new(0.45, 1.0, 0.55, 1.0);
                    app.particles.burst(at + chest * 0.5, Burst { count: 9, color, speed: 0.9, life: 0.9, size: 0.12, gravity: -2.5 });
                }
            }
            Flash::Cast { unit } => {
                cues.push(Sfx::Cast);
                if let Some(at) = world_of(&show.playback, *unit).filter(|_| particles) {
                    let color = Color::new(0.50, 0.78, 1.0, 1.0);
                    app.particles.burst(at + chest, Burst { count: 22, color, speed: 2.6, life: 0.6, size: 0.14, gravity: 0.0 });
                }
            }
            Flash::Death { unit } => {
                cues.push(Sfx::Death);
                if let Some(at) = world_of(&show.playback, *unit).filter(|_| particles) {
                    let color = Color::new(0.75, 0.75, 0.80, 0.9);
                    app.particles.burst(at + chest * 0.6, Burst { count: 26, color, speed: 1.5, life: 0.9, size: 0.2, gravity: -0.8 });
                }
            }
        }
    }
    if show.playback.is_finished() && !show.announced {
        show.announced = true;
        if app.state.seat.is_some() {
            cues.push(if show.playback.winner().is_some_and(|t| t.0 == my_team) { Sfx::Victory } else { Sfx::Defeat });
        }
    }
    cues.dedup();
    for cue in cues {
        app.play(cue);
    }

    // The 3D scene.
    view.begin();
    let mut ground = Ground::default();
    board_tiles(&mut ground, layout, false, None);
    for u in show.playback.units().filter(|u| u.alive) {
        let (q, r) = u.position_at(now);
        let at = place(q, r) + RING_LIFT;
        if app.settings.shadows {
            ground.disc(at - RING_LIFT * 0.5, 0.48, SHADOW);
        }
        ground.ring(at, 0.50, 0.62, if u.team.0 == my_team { FRIEND } else { ENEMY });
        if u.casting {
            let pulse = 0.72 + 0.06 * (now as f32 / 90.0).sin();
            ground.ring(at, pulse, pulse + 0.10, ACCENT);
        }
    }
    for (age, flash) in show.playback.recent_flashes(450) {
        if let Flash::Cast { unit } = flash
            && let Some(at) = world_of(&show.playback, *unit)
        {
            let t = age as f32 / 450.0;
            let radius = 0.7 + t * 1.3;
            ground.ring(at + RING_LIFT * 2.0, radius, radius + 0.14 * (1.0 - t), Color::new(0.45, 0.72, 1.0, 1.0));
        }
    }
    ground.draw();
    for u in show.playback.units().filter(|u| u.alive) {
        let (q, r) = u.position_at(now);
        let at = place(q, r) + lunge.get(&u.id).copied().unwrap_or(Vec3::ZERO);
        let look = app.look_of_name(&u.name);
        let yaw = show.facing.get(&u.id).copied().unwrap_or(0.0);
        let stunned = u.stunned_until_ms > now;
        let tint = if stunned { [0.62, 0.62, 0.70] } else { [1.0; 3] };
        // Animated models act out what the unit is doing.
        let seconds = now as f32 / 1000.0;
        let walking = u.pos != u.prev_pos && now.saturating_sub(u.moved_at_ms) < autoengine::playback::MOVE_ANIMATION_MS;
        let swing = attacked.get(&u.id).and_then(|age| look.pose("attack", *age, false)).filter(|p| p.time < look.model.clip_duration(p.clip));
        let pose = if stunned {
            look.pose("idle", 0.0, false)
        } else if u.casting {
            look.pose("cast", seconds, true)
        } else if swing.is_some() {
            swing
        } else if walking {
            look.pose("walk", seconds, true)
        } else {
            look.pose("idle", seconds + u.id.0 as f32 * 0.37, true)
        };
        app.draw_model(&look, at, yaw, look.scale, tint, hit.get(&u.id).copied().unwrap_or(0.0), pose);
    }
    app.particles.draw(&view.camera);
    view.end();

    // Flat markers on top: health, numbers, lines.
    text_centered(&show.title, layout.board_mid_x, view.area.y + 24.0, 22.0, TEXT);
    let head_of = |playback: &Playback, id| {
        playback.unit(id).zip(world_of(playback, id)).map(|(u, at)| {
            let height = app.look_of_name(&u.name).height();
            (view.project(at + vec3(0.0, height + 0.25, 0.0)), view.project(at + vec3(0.0, height * 0.55, 0.0)), view.pixels_per_unit(at))
        })
    };
    for u in show.playback.units().filter(|u| u.alive) {
        let Some((head, _, size)) = head_of(&show.playback, u.id) else { continue };
        let full = u.max_health.max(1) as f32;
        draw_health_bar(head, (size * 1.1).clamp(28.0, 70.0), u.health as f32 / full, u.shield as f32 / full, u.team.0 == my_team);
        if u.stunned_until_ms > now {
            text_centered("stun", head.x, head.y - 8.0, 14.0, GOLD_COLOR);
        }
    }

    for (age, flash) in show.playback.recent_flashes(900) {
        let age = age as f32;
        match flash {
            Flash::Attack { source, target } if age < 160.0 => {
                if let (Some((_, a, _)), Some((_, b, _))) = (head_of(&show.playback, *source), head_of(&show.playback, *target)) {
                    let alpha = 1.0 - age / 160.0;
                    draw_line(a.x, a.y, b.x, b.y, 2.0, Color::new(1.0, 1.0, 1.0, alpha * 0.6));
                }
            }
            Flash::Damage { target, amount, absorbed, kind, crit } => {
                if let Some((p, _, _)) = head_of(&show.playback, *target) {
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
                    text_centered(&shown.to_string(), p.x + jitter, p.y - 12.0 - age * 0.03, size, color);
                }
            }
            Flash::Heal { target, amount } => {
                if let Some((_, p, _)) = head_of(&show.playback, *target) {
                    let alpha = (1.0 - age / 900.0).max(0.0);
                    text_centered(&format!("+{amount}"), p.x, p.y - age * 0.03, 17.0, Color::new(0.4, 1.0, 0.5, alpha));
                }
            }
            Flash::Death { unit } if age < 500.0 => {
                if let Some(at) = world_of(&show.playback, *unit) {
                    let p = view.project(at + vec3(0.0, 0.4, 0.0));
                    let alpha = 1.0 - age / 500.0;
                    let s = view.pixels_per_unit(at) * 0.4;
                    let c = Color::new(0.9, 0.3, 0.3, alpha);
                    draw_line(p.x - s, p.y - s, p.x + s, p.y + s, 4.0, c);
                    draw_line(p.x - s, p.y + s, p.x + s, p.y - s, 4.0, c);
                }
            }
            _ => {}
        }
    }

    let secs = (now as f32 / 1000.0).min(show.playback.duration_ms() as f32 / 1000.0);
    let footer_y = layout.scene_bottom() - 14.0;
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
    app.show = Some(show);
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
