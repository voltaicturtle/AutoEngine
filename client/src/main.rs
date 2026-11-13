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
//! This file holds start-up, the main loop, networking and the panels around
//! the board. The rest is split by job: `planning` (board, bench, shop),
//! `fight` (fight playback), `assets` (models and icons), `viewer` (the asset
//! viewer), `menu` and `console`. `docs/client.md` has the full list.
//!
//! Usage: `autoengine-client [--addr 127.0.0.1:7777] [--name NAME] [--spectate]
//!         [--assets DIR] [--settings FILE] [--replay FILE] [--view FILE] [--autoplay] [--no-ready]
//!         [--menu] [--stats] [--screenshot SECONDS:FILE]...`
//!
//! `--view FILE` opens the asset viewer on a model file: it shows the model
//! with its statistics, plays its animation clips, previews the particle
//! effects, and reloads when the file changes.
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

mod assets;
mod console;
mod draw;
mod fight;
mod fx;
mod menu;
mod model;
mod planning;
mod scene;
mod settings;
mod sound;
mod theme;
mod viewer;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;

use autoengine::combat::UnitId;
use autoengine::content::{BoardUnit, UnitDef};
use autoengine::game::{Command, Opponent, OwnedUnit, Player, PlayerId, Slot};
use autoengine::hex::Hex;
use autoengine::net::{GameClient, Status};
use autoengine::playback::{Flash, Playback};
use autoengine::protocol::DevCommand;
use autoengine::protocol::{ClientMsg, ViewPhase};
use macroquad::prelude::*;

use draw::{Layout, button, draw_health_bar, draw_pips, draw_unit_token, label, panel, text_centered, unit_color};
use model::{Model, Pose, RawModel};
use scene::Ground;
use settings::{Action, Settings};
use theme::{Theme, ui};
use sound::{Sfx, Sounds};
use autoengine::replay::ReplayFile;

use assets::*;
use console::*;
use fight::*;
use menu::*;
use planning::*;
use viewer::*;

#[derive(Clone)]
struct Options {
    /// Server address and player name; the settings file fills these in
    /// when the command line does not.
    addr: Option<String>,
    name: Option<String>,
    settings: PathBuf,
    /// A saved fight to watch instead of connecting to a server.
    replay: Option<PathBuf>,
    /// A model file to open in the asset viewer.
    view: Option<PathBuf>,
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
    let usage = "usage: autoengine-client [--addr HOST:PORT] [--name NAME] [--spectate] [--assets DIR] [--settings FILE] [--replay FILE] [--view FILE] [--autoplay] \
                 [--no-ready] [--menu] [--stats] [--screenshot SECONDS:FILE]...";
    let mut o = Options {
        addr: None,
        name: None,
        settings: PathBuf::from("autoengine-client.ron"),
        replay: None,
        view: None,
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
            "--view" => o.view = Some(PathBuf::from(value()?)),
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

struct App {
    options: Options,
    /// The connection to the server and everything it has told us.
    net: GameClient,
    drag: Option<Drag>,
    show: Option<Show>,
    /// Whose board is on screen; `None` means our own.
    viewing: Option<PlayerId>,
    /// Round the autopilot is working on, how far it got, and when.
    autoplay_step: (Option<usize>, u8, f64),
    connect_error: Option<String>,
    error_shown_at: f64,
    /// Time left in the current phase as last reported, and when that was.
    countdown: (u64, f64),
    models: RefCell<Models>,
    /// Camera angle around the board, and distance as a multiple of the
    /// distance that just fits the board.
    camera_yaw: f32,
    camera_zoom: f32,
    settings: Settings,
    theme: Theme,
    /// When `theme.ron` was last changed, and when assets were last checked.
    theme_seen: Option<std::time::SystemTime>,
    assets_checked_at: f64,
    icons: RefCell<Icons>,
    particles: fx::Particles,
    sounds: Sounds,
    menu_open: bool,
    /// A saved fight being watched instead of a live game.
    replay: Option<ReplayFile>,
    viewer: Option<Viewer>,
    /// The cheat console's input line while it is open.
    console: Option<String>,
    /// Units and vertices drawn this frame, for the statistics overlay.
    drawn: Cell<(usize, usize)>,
    /// Recent frame times in milliseconds.
    frame_ms: Vec<f32>,
}

impl App {
    fn send(&mut self, message: ClientMsg) {
        self.net.send(message);
    }

    /// Starts connecting. The client joins by itself, and after a lost
    /// connection reconnects into the same seat.
    fn connect(&mut self) {
        let addr = self.options.addr.clone().unwrap_or_else(|| self.settings.addr.clone());
        self.net = if self.options.spectate {
            GameClient::spectate(addr)
        } else {
            GameClient::join(addr, self.options.name.clone().unwrap_or_else(|| self.settings.name.clone()))
        };
    }

    /// The look of a unit given its content id.
    fn look_of(&self, unit_id: &str) -> Look {
        let def = self.net.state.content.as_ref().and_then(|c| c.unit(unit_id));
        let name = def.map_or(unit_id, |d| d.name.as_str());
        let mut look = self.models.borrow_mut().look(def, name);
        look.animate = self.settings.animations;
        look
    }

    /// The look of a unit given the name fights report it under.
    fn look_of_name(&self, name: &str) -> Look {
        let def = self.net.state.content.as_ref().and_then(|c| c.units().find(|u| u.name == name));
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

    /// Loads `theme.ron` and puts it into effect. A broken file keeps the
    /// current look and says what is wrong.
    fn load_theme(&mut self) {
        self.theme_seen = theme::modified(&self.options.assets.join("theme.ron"));
        match Theme::load(&self.options.assets) {
            Ok(new) => {
                if new.font != self.theme.font || self.assets_checked_at == 0.0 {
                    let font = new.font.as_ref().and_then(|file| {
                        let path = self.options.assets.join(file);
                        let loaded = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| load_ttf_font_from_bytes(&b).map_err(|e| e.to_string()));
                        loaded.map_err(|e| self.connect_error = Some(format!("font {}: {e}", path.display()))).ok()
                    });
                    theme::set_font(font);
                }
                theme::set_palette(new.palette);
                model::set_light(new.light.direction, new.light.ambient, new.light.diffuse);
                self.theme = new;
            }
            Err(e) => self.connect_error = Some(e),
        }
    }

    /// Picks up edits to the theme, models and icons, twice a second.
    fn refresh_assets(&mut self) {
        if get_time() - self.assets_checked_at < 0.5 {
            return;
        }
        if let Some(viewer) = &mut self.viewer
            && theme::modified(&viewer.path) != viewer.seen
        {
            viewer.load();
        }
        if theme::modified(&self.options.assets.join("theme.ron")) != self.theme_seen {
            self.load_theme();
        }
        self.assets_checked_at = get_time();
        self.models.borrow_mut().drop_changed();
        self.icons.borrow_mut().drop_changed();
    }

    /// Whether the key bound to an action was pressed this frame. Typing
    /// in the console does not set off other actions.
    fn pressed(&self, action: Action) -> bool {
        (self.console.is_none() || action == Action::Console) && is_key_pressed(self.settings.key(action))
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
        self.net.state.me()
    }

    /// The player whose board is on screen.
    fn shown_player(&self) -> Option<&Player> {
        let view = self.net.state.view.as_ref()?;
        let id = self.viewing.or(self.net.state.seat).or_else(|| view.players.iter().find(|p| p.alive).map(|p| p.id))?;
        view.players.get(id.0 as usize)
    }

    fn viewing_self(&self) -> bool {
        self.net.state.seat.is_some() && (self.viewing.is_none() || self.viewing == self.net.state.seat)
    }

    fn pump_network(&mut self) {
        self.net.update();
        if let Some(view) = &self.net.state.view {
            let remaining = match view.phase {
                ViewPhase::Planning { remaining_ms } | ViewPhase::Showing { remaining_ms } => remaining_ms,
                _ => 0,
            };
            if remaining != self.countdown.0 {
                self.countdown = (remaining, get_time());
            }
        }
        if self.net.state.error.is_some() && self.error_shown_at == 0.0 {
            self.error_shown_at = get_time();
        }
        if self.net.state.error.is_some() && get_time() - self.error_shown_at > 4.0 {
            self.net.state.error = None;
            self.error_shown_at = 0.0;
        }

        // Start the next replay once the current one is over.
        if self.show.is_none()
            && let Some(record) = self.net.state.combats.pop_front()
        {
            // Spectators receive every fight of the round; show the first.
            if self.net.state.seat.is_none() {
                self.net.state.combats.clear();
            }
            let me = self.net.state.seat;
            let flipped = me.is_some() && record.away == Opponent::Player(me.unwrap());
            let away = match &record.away {
                Opponent::Player(p) => self.net.state.player_name(*p),
                Opponent::Ghost(p) => format!("a copy of {}", self.net.state.player_name(*p)),
                Opponent::Neutral => "the neutral camp".to_string(),
            };
            let title = format!("{} vs {}", self.net.state.player_name(record.home), away);
            self.show = Some(Show::new(Playback::new(&record), flipped, title));
        }
    }

    /// A small built-in autopilot, for demos and for testing the client.
    fn autoplay(&mut self) {
        let Some(view) = &self.net.state.view else {
            if self.net.state.welcomed && !self.net.state.started && self.net.state.seat.is_some() && get_time() > 1.0 {
                self.send(ClientMsg::Start);
                self.net.state.started = true;
            }
            return;
        };
        let (Some(me), Some(content)) = (self.me().cloned(), &self.net.state.content) else { return };
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
        theme: Theme::default(),
        theme_seen: None,
        assets_checked_at: 0.0,
        icons: RefCell::new(Icons { assets: options.assets.clone(), cache: HashMap::new() }),
        settings,
        particles: fx::Particles::default(),
        sounds: Sounds::load(&options.assets).await,
        viewer: options.view.clone().map(Viewer::new),
        console: None,
        menu_open: options.menu,
        replay,
        drawn: Cell::new((0, 0)),
        frame_ms: Vec::new(),
        net: GameClient::offline(),
        drag: None,
        show: None,
        viewing: None,
        autoplay_step: (None, 0, 0.0),
        connect_error: None,
        error_shown_at: 0.0,
        countdown: (0, 0.0),
        models: RefCell::new(Models { assets: options.assets.clone(), files: HashMap::new(), figures: HashMap::new() }),
        camera_yaw: 0.0,
        camera_zoom: 1.0,
        options,
    };
    if let Some(replay) = &app.replay {
        match replay.content.clone().into_content() {
            Ok(content) => app.net.state.content = Some(content),
            Err(e) => {
                eprintln!("the replay's content is invalid: {e:?}");
                return;
            }
        }
    } else if app.viewer.is_none() {
        app.connect();
    }
    app.connect_error = app.connect_error.take().or(problem);
    app.load_theme();
    // Room for a whole model part in one draw call.
    macroquad::window::gl_set_drawcall_buffer_capacity(model::MAX_PART_VERTICES + 16, model::MAX_PART_INDICES + 16);

    let mut shots = std::mem::take(&mut app.options.screenshots);
    shots.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut shots = shots.into_iter().peekable();

    loop {
        if app.replay.is_none() && app.viewer.is_none() {
            app.pump_network();
            if app.options.autoplay {
                app.autoplay();
            }
        }
        app.refresh_assets();
        clear_background(ui().background);
        app.drawn.set((0, 0));
        app.frame_ms.push(get_frame_time() * 1000.0);
        if app.frame_ms.len() > 120 {
            app.frame_ms.remove(0);
        }
        app.particles.update(get_frame_time());
        if app.pressed(Action::Menu) {
            app.menu_open = !app.menu_open;
        }
        update_console(&mut app);
        if app.pressed(Action::Stats) {
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
        if app.pressed(Action::CameraReset) {
            (app.camera_yaw, app.camera_zoom) = (0.0, 1.0);
        }
        let layout = Layout::new(&app);
        if app.menu_open {
            // The game carries on underneath; the menu has the screen.
            draw_menu(&mut app);
        } else if app.viewer.is_some() {
            draw_asset_viewer(&mut app);
        } else if app.replay.is_some() {
            draw_replay_viewer(&mut app, &layout);
        } else if app.net.state.view.is_none() {
            draw_lobby(&mut app);
        } else {
            draw_game(&mut app, &layout);
        }
        if app.settings.show_stats && !app.menu_open {
            draw_stats(&app, &layout);
        }
        draw_console(&app, &layout);
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
    let offline = app.replay.is_some() || app.viewer.is_some();
    let link = match app.net.status() {
        Status::Reconnecting { reason, .. } if !offline => Some(reason),
        _ => None,
    };
    let message = app.connect_error.as_ref().or(link).or(app.net.state.error.as_ref());
    if let Some(message) = message {
        let w = screen_width();
        let width = (theme::text_width(message, 20.0) + 40.0).clamp(600.0, w - 20.0);
        draw_rectangle(w / 2.0 - width / 2.0, 50.0, width, 34.0, Color::new(0.45, 0.12, 0.12, 0.95));
        text_centered(message, w / 2.0, 72.0, 20.0, ui().text);
    }
}

fn draw_lobby(app: &mut App) {
    let (w, h) = (screen_width(), screen_height());
    text_centered("AutoEngine", w / 2.0, h * 0.22, 64.0, ui().text);
    if !app.net.state.welcomed {
        let line = if app.net.is_connected() || *app.net.status() == Status::Connecting { "Connecting..." } else { "Not connected" };
        text_centered(line, w / 2.0, h * 0.32, 26.0, ui().muted);
        return;
    }
    text_centered("Waiting for players", w / 2.0, h * 0.32, 26.0, ui().muted);
    let seats = app.net.state.lobby.clone();
    for (i, seat) in seats.iter().enumerate() {
        let y = h * 0.40 + i as f32 * 34.0;
        panel(w / 2.0 - 180.0, y, 360.0, 28.0, ui().panel);
        let mine = app.net.state.seat == Some(PlayerId(i as u8));
        match seat {
            Some(name) => label(name, w / 2.0 - 168.0, y + 20.0, 20.0, if mine { ui().gold } else { ui().text }),
            None => label("open seat (a bot will play it)", w / 2.0 - 168.0, y + 20.0, 20.0, ui().muted),
        }
    }
    let y = h * 0.40 + seats.len() as f32 * 34.0 + 24.0;
    if app.net.state.seat.is_some() {
        if button("Start game", w / 2.0 - 90.0, y, 180.0, 44.0, true) {
            app.send(ClientMsg::Start);
        }
    } else {
        text_centered("Spectating: waiting for the game to start", w / 2.0, y + 24.0, 20.0, ui().muted);
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
    let Some(view) = &app.net.state.view else { return };
    let w = screen_width();
    draw_rectangle(0.0, 0.0, w, 44.0, ui().panel);
    label(&format!("Round {}  ({})", view.round_name, view.round_kind), 16.0, 29.0, 24.0, ui().text);
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
    text_centered(&phase, w / 2.0, 29.0, 24.0, ui().accent);
    if let Some(me) = app.me() {
        let xp_needed = app
            .net.state
            .content
            .as_ref()
            .and_then(|c| c.rules().levels.get(me.level as usize - 1))
            .map(|l| l.xp_to_next)
            .unwrap_or(0);
        let x = w - 520.0;
        label(&format!("Health {}", me.health.max(0)), x, 29.0, 22.0, if me.health > 30 { ui().good } else { ui().bad });
        label(&format!("Gold {}", me.gold), x + 140.0, 29.0, 22.0, ui().gold);
        label(&format!("Level {}  ({}/{} xp)", me.level, me.xp, xp_needed), x + 250.0, 29.0, 22.0, ui().text);
    } else {
        label("Spectating", w - 140.0, 29.0, 22.0, ui().muted);
    }
}

fn draw_traits(app: &App, layout: &Layout) {
    panel(0.0, 44.0, layout.left_w, screen_height() - 44.0, ui().panel);
    label("Traits", 14.0, 74.0, 22.0, ui().text);
    let (Some(player), Some(content)) = (app.shown_player(), &app.net.state.content) else { return };
    let units: Vec<BoardUnit> = player.board_units();
    let Ok(mut traits) = content.active_traits(&units) else { return };
    traits.sort_by_key(|t| (t.tier.is_none(), std::cmp::Reverse(t.count)));
    let mut y = 104.0;
    for t in traits {
        let Some(def) = content.trait_def(&t.id) else { continue };
        let steps: Vec<String> = def.breakpoints.iter().map(|b| b.count.to_string()).collect();
        let color = if t.tier.is_some() { ui().gold } else { ui().muted };
        label(&format!("{}  {}", t.count, def.name), 14.0, y, 20.0, color);
        label(&format!("({})", steps.join(" / ")), 150.0, y, 16.0, ui().muted);
        y += 26.0;
    }
    if !player.augments.is_empty() {
        y += 14.0;
        label("Augments", 14.0, y, 22.0, ui().text);
        y += 28.0;
        for a in &player.augments {
            label(&app.net.state.display_name(a), 14.0, y, 18.0, ui().accent);
            y += 22.0;
        }
    }
}

fn draw_scoreboard(app: &mut App, layout: &Layout) {
    let x = screen_width() - layout.right_w;
    panel(x, 44.0, layout.right_w, screen_height() - 44.0, ui().panel);
    label("Players", x + 14.0, 74.0, 22.0, ui().text);
    let Some(view) = &app.net.state.view else { return };
    let mut players: Vec<&Player> = view.players.iter().collect();
    players.sort_by_key(|p| (!p.alive, std::cmp::Reverse(p.health), p.placement));
    let mut y = 88.0;
    let mut clicked = None;
    let (mx, my) = mouse_position();
    for p in players {
        let me = app.net.state.seat == Some(p.id);
        let shown = app.shown_player().map(|s| s.id) == Some(p.id);
        let hover = mx >= x + 8.0 && mx <= x + layout.right_w - 8.0 && my >= y && my <= y + 34.0;
        panel(x + 8.0, y, layout.right_w - 16.0, 34.0, if shown || hover { ui().panel_light } else { ui().panel });
        let color = if !p.alive { ui().muted } else if me { ui().gold } else { ui().text };
        label(&p.name, x + 16.0, y + 16.0, 18.0, color);
        if p.alive {
            let frac = (p.health.max(0) as f32 / 100.0).min(1.0);
            draw_rectangle(x + 16.0, y + 22.0, 150.0, 6.0, ui().background);
            draw_rectangle(x + 16.0, y + 22.0, 150.0 * frac, 6.0, if p.health > 30 { ui().good } else { ui().bad });
            label(&format!("{}", p.health.max(0)), x + 174.0, y + 30.0, 16.0, ui().text);
            label(&format!("L{}", p.level), x + 206.0, y + 30.0, 16.0, ui().muted);
            if p.ready {
                label("ok", x + 174.0, y + 14.0, 14.0, ui().good);
            }
        } else if let Some(place) = p.placement {
            label(&format!("out, place {place}"), x + 130.0, y + 16.0, 16.0, ui().muted);
        }
        if hover && is_mouse_button_pressed(MouseButton::Left) {
            clicked = Some(p.id);
        }
        y += 38.0;
    }
    if let Some(id) = clicked {
        app.viewing = if Some(id) == app.net.state.seat { None } else { Some(id) };
    }

    y += 12.0;
    label("Log", x + 14.0, y + 14.0, 22.0, ui().text);
    y += 24.0;
    let room = ((screen_height() - y - 8.0) / 18.0) as usize;
    let skip = app.net.state.log.len().saturating_sub(room);
    for line in app.net.state.log.iter().skip(skip) {
        let mut line = line.clone();
        if theme::text_width(&line, 14.0) > layout.right_w - 24.0 {
            while theme::text_width(&line, 14.0) > layout.right_w - 36.0 && line.len() > 4 {
                line.pop();
            }
            line.push_str("...");
        }
        y += 18.0;
        label(&line, x + 14.0, y, 14.0, ui().muted);
    }
}

/// The hex grid itself. `own_half` highlights the rows a player may use.
/// Queues the board's tiles. `own_half` tints the near half, where the
/// viewer may place units; `highlight` marks the hex under a dragged unit.
fn board_tiles(ground: &mut Ground, layout: &Layout, own_half: bool, highlight: Option<(i32, i32)>) {
    for row in 0..layout.rows {
        for col in 0..layout.cols {
            let mine = own_half && row >= layout.rows / 2;
            let mut fill = if mine { ui().tile_own } else { ui().tile };
            if (col + row) % 2 == 0 {
                fill = Color::new(fill.r * 0.93, fill.g * 0.93, fill.b * 0.93, 1.0);
            }
            if highlight == Some((col, row)) {
                fill = ui().tile_highlight;
            }
            ground.hex_tile(layout.hex_world(col, row), 0.94, 0.22, fill);
        }
    }
}

fn unit_label(app: &App, unit: &OwnedUnit) -> String {
    app.net.state.display_name(&unit.unit)
}

/// Stars make a unit a little bigger.
fn star_scale(star: u8) -> f32 {
    1.0 + 0.14 * star.saturating_sub(1) as f32
}

const RING_LIFT: Vec3 = Vec3::new(0.0, 0.03, 0.0);

fn draw_game_over(app: &App) {
    let Some(view) = &app.net.state.view else { return };
    let ViewPhase::Over { winner } = view.phase else { return };
    if app.show.is_some() {
        return;
    }
    let (w, h) = (screen_width(), screen_height());
    draw_rectangle(w / 2.0 - 240.0, h / 2.0 - 200.0, 480.0, 400.0, Color::new(0.04, 0.05, 0.07, 0.96));
    let title = match winner {
        Some(id) if Some(id) == app.net.state.seat => "You win!".to_string(),
        Some(id) => format!("{} wins", app.net.state.player_name(id)),
        None => "Game over".to_string(),
    };
    text_centered(&title, w / 2.0, h / 2.0 - 150.0, 40.0, ui().gold);
    let mut players: Vec<&Player> = view.players.iter().collect();
    players.sort_by_key(|p| p.placement.unwrap_or(u32::MAX));
    for (i, p) in players.iter().enumerate() {
        let y = h / 2.0 - 100.0 + i as f32 * 30.0;
        let color = if Some(p.id) == app.net.state.seat { ui().gold } else { ui().text };
        label(&format!("{}.", p.placement.unwrap_or(0)), w / 2.0 - 180.0, y, 22.0, ui().muted);
        label(&p.name, w / 2.0 - 140.0, y, 22.0, color);
        label(&format!("level {}", p.level), w / 2.0 + 80.0, y, 18.0, ui().muted);
    }
}

/// Converts an axial hex to the `(q, r)` floats the layout works in.
#[allow(dead_code)]
fn axial(hex: Hex) -> (f32, f32) {
    (hex.q as f32, hex.r as f32)
}
