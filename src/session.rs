//! A hosted game: seats, connections and timing around a [`Game`].
//!
//! [`Session`] is the server's brain and knows nothing about sockets. The
//! transport tells it when a connection sends a message or drops, and calls
//! [`Session::tick`] with the current time; the session answers by queueing
//! messages for specific connections, collected with
//! [`Session::drain_outbox`]. That keeps the whole server logic testable
//! without any networking.
//!
//! Empty seats are played by bots, and so is the seat of anyone who
//! disconnects, until they join again under the same name.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::bot;
use crate::content::ContentSet;
use crate::game::{CombatRecord, Game, Grant, Opponent, PlayerId};
use crate::matchlog::{MatchLog, MatchLogError};
use crate::replay::ReplayFile;
use crate::protocol::DevCommand;
use crate::protocol::{
    ClientMsg, ContentInfo, GameView, PROTOCOL_VERSION, PlayerStatus, ServerMsg, ViewPhase, events_checksum, visible_to,
};
use crate::rng::Rng;

/// Whether a viewer is sent a fight: players see their own, spectators all.
fn may_see(role: Role, record: &CombatRecord) -> bool {
    match role {
        Role::Spectator => true,
        Role::Seat(i) => {
            let me = PlayerId(i as u8);
            record.home == me || record.away == Opponent::Player(me)
        }
    }
}

/// What [`Session::record_to`] saves: the match, and the seat tokens so
/// its players can take their seats back after [`Session::resume`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedMatch {
    pub log: MatchLog,
    pub tokens: Vec<Option<String>>,
}

/// 64 bits the outside world cannot guess, for seat tokens: the standard
/// library seeds every `RandomState` from the operating system, so hashing
/// with a fresh one yields OS randomness. The clock and process id are mixed
/// in as well.
fn os_entropy() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    hasher.write_u128(nanos);
    hasher.write_u32(std::process::id());
    hasher.finish()
}

/// Something to run on a game when it starts.
type Setup = Box<dyn FnMut(&mut Game) + Send>;

/// Identifies a connection to the transport.
pub type ConnId = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionConfig {
    /// Number of players in the game, people and bots together.
    pub seats: usize,
    pub seed: u64,
    /// How long players get to shop and arrange each round.
    pub planning_ms: u64,
    /// Pause after a round resolves so clients can play back its fights:
    /// the longest fight plus `after_combat_ms`. When false, the next round
    /// starts on the next tick.
    pub wait_for_playback: bool,
    pub after_combat_ms: u64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig { seats: 8, seed: 1, planning_ms: 30_000, wait_for_playback: true, after_combat_ms: 2_000 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Lobby,
    Planning { deadline: u64 },
    Showing { until: u64 },
    Over,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Seat(usize),
    Spectator,
}

#[derive(Clone, Debug, Default)]
struct Seat {
    name: Option<String>,
    conn: Option<ConnId>,
    /// Secret handed to the seat's player, needed to reclaim it.
    token: Option<String>,
}

/// One hosted game and everyone connected to it.
pub struct Session {
    content: Arc<ContentSet>,
    config: SessionConfig,
    seats: Vec<Seat>,
    conns: BTreeMap<ConnId, Role>,
    game: Option<Game>,
    /// Where to save every fight as a replay file, if anywhere.
    record_dir: Option<std::path::PathBuf>,
    dev: bool,
    /// Run on the game when it starts; see [`Session::on_start`].
    setup: Option<Setup>,
    phase: Phase,
    bot_rng: Rng,
    /// Source of seat tokens; seeded from the clock, never from the game seed.
    token_rng: Rng,
    events_sent: usize,
    /// The view each connection was last sent, so only changes go out.
    views: BTreeMap<ConnId, GameView>,
    /// Connections that asked for fights with their whole event log.
    full_fights: std::collections::BTreeSet<ConnId>,
    /// The fights of the round last resolved, kept for
    /// [`ClientMsg::ResendCombat`], and that round's number.
    shown: (u32, Vec<CombatRecord>),
    outbox: Vec<(ConnId, ServerMsg)>,
    /// Whether chat is passed on; see [`Session::allow_chat`].
    chat: bool,
    /// How far behind spectators see the game; see [`Session::set_spectator_delay`].
    spectator_delay_ms: u64,
    /// Game messages held back for spectators: when to send, to whom, what.
    delayed: std::collections::VecDeque<(u64, ConnId, ServerMsg)>,
    /// The time last given by the transport.
    now: u64,
    /// Connections to close, for [`Session::take_closed`].
    closing: Vec<ConnId>,
    /// When the host paused the game, if it is paused.
    paused_at: Option<u64>,
    /// Names kicked from this game, which may not take a seat again.
    kicked: std::collections::BTreeSet<String>,
    bot_difficulty: bot::Difficulty,
    /// The last few chat messages, for people who join later.
    chat_history: std::collections::VecDeque<ServerMsg>,
    /// When each connection last chatted, recent ones only, for the limit.
    chat_times: BTreeMap<ConnId, std::collections::VecDeque<u64>>,
    /// Rewrites or blocks chat lines; see [`Session::set_chat_filter`].
    chat_filter: Option<ChatFilter>,
}

/// Chat messages kept for people who join later.
const CHAT_HISTORY: usize = 20;

/// At most this many chat lines from one connection...
const CHAT_BURST: usize = 5;
/// ...in this many milliseconds.
const CHAT_WINDOW_MS: u64 = 10_000;

/// See [`Session::set_chat_filter`].
type ChatFilter = Box<dyn Fn(&str) -> Option<String> + Send>;

impl Session {
    /// How well the bots in this game play, including those keeping the
    /// seat of someone who left. Normal by default.
    pub fn set_bot_difficulty(&mut self, difficulty: bot::Difficulty) {
        self.bot_difficulty = difficulty;
    }

    /// Removes whoever holds a seat, by name: they are told why and
    /// disconnected, a bot plays the seat from then on, and they cannot
    /// take it back. Before the game starts the seat opens again instead.
    /// Returns whether anyone had that name.
    pub fn kick(&mut self, name: &str, now: u64) -> bool {
        let Some(seat) = self.seats.iter().position(|s| s.name.as_deref() == Some(name)) else { return false };
        self.kicked.insert(name.to_string());
        if let Some(conn) = self.seats[seat].conn {
            self.send(conn, ServerMsg::Error { message: "you were removed from the game by the host".into() });
            self.closing.push(conn);
            self.disconnect(conn, now);
        }
        if self.has_started() {
            self.seats[seat].token = None;
        } else {
            self.seats[seat] = Seat::default();
            self.send_lobby();
        }
        true
    }

    /// Stops the clock: no round ends and no fight is cut short until
    /// [`Session::unpause`]. Players can still shop and arrange. Returns
    /// whether it paused (not before the start, after the end, or twice).
    pub fn pause(&mut self, now: u64) -> bool {
        if self.paused_at.is_some() || !matches!(self.phase, Phase::Planning { .. } | Phase::Showing { .. }) {
            return false;
        }
        self.now = self.now.max(now);
        self.paused_at = Some(now);
        self.announce("the host has paused the game");
        self.broadcast(now);
        true
    }

    /// Starts the clock again where it stopped.
    pub fn unpause(&mut self, now: u64) -> bool {
        let Some(at) = self.paused_at.take() else { return false };
        self.now = self.now.max(now);
        let gap = now.saturating_sub(at);
        match &mut self.phase {
            Phase::Planning { deadline } => *deadline += gap,
            Phase::Showing { until } => *until += gap,
            _ => {}
        }
        self.announce("the game continues");
        self.broadcast(now);
        if matches!(self.phase, Phase::Planning { .. }) && self.game.as_ref().is_some_and(Game::all_ready) {
            self.resolve(now);
        }
        true
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Connections the session wants closed; see [`crate::net::Host::take_closed`].
    pub fn take_closed(&mut self) -> Vec<ConnId> {
        std::mem::take(&mut self.closing)
    }

    /// Shows spectators the game this many milliseconds late, so nobody
    /// watching can tell a player what their opponents are doing. Chat and
    /// the lobby are not delayed. Off (0) by default.
    pub fn set_spectator_delay(&mut self, ms: u64) {
        self.spectator_delay_ms = ms;
    }

    /// Turns chat on or off; it is on by default.
    pub fn allow_chat(&mut self, allow: bool) {
        self.chat = allow;
    }

    /// Passes every chat line through `filter` before anyone sees it: it
    /// returns the line to show (changed or not), or `None` to drop it.
    /// For word filters and the like. Lines from the server itself are not
    /// filtered.
    pub fn set_chat_filter(&mut self, filter: impl Fn(&str) -> Option<String> + Send + 'static) {
        self.chat_filter = Some(Box::new(filter));
    }

    /// Sends a chat message to everyone from the server itself, such as an
    /// announcement from whoever runs it.
    pub fn announce(&mut self, text: &str) {
        if let Some(text) = crate::protocol::clean_chat(text) {
            self.post_chat(ServerMsg::Chat { from: "server".into(), seat: None, text });
        }
    }

    fn post_chat(&mut self, message: ServerMsg) {
        for conn in self.conns.keys().copied().collect::<Vec<_>>() {
            self.send(conn, message.clone());
        }
        self.chat_history.push_back(message);
        if self.chat_history.len() > CHAT_HISTORY {
            self.chat_history.pop_front();
        }
    }

    /// Lets players use [`DevCommand`] cheats. For development only.
    pub fn allow_dev(&mut self, allow: bool) {
        self.dev = allow;
    }

    fn dev_command(&mut self, conn: ConnId, command: DevCommand, now: u64) {
        let error = |s: &mut Self, message: &str| s.send(conn, ServerMsg::Error { message: message.to_string() });
        if !self.dev {
            return error(self, "developer commands are off; start the server with --dev");
        }
        let Some(seat) = self.seat_of(conn) else { return error(self, "spectators cannot act") };
        if !matches!(self.phase, Phase::Planning { .. }) {
            return error(self, "no round is in progress");
        }
        let id = PlayerId(seat as u8);
        let game = self.game.as_mut().expect("a started session has a game");
        let grant = match command {
            DevCommand::Gold(n) => Grant::Gold(n),
            DevCommand::Currency(currency, amount) => Grant::Currency { currency, amount },
            DevCommand::Xp(n) => Grant::Xp(n),
            DevCommand::Unit(unit) => Grant::Unit(unit),
            DevCommand::Item(item) => Grant::Item(item),
            DevCommand::Augment(augment) => Grant::Augment(augment),
            DevCommand::Hurt(n) => Grant::Damage(n),
            DevCommand::Skip => return self.resolve(now),
        };
        if game.grant(id, grant).is_err() {
            return error(self, "there is nothing with that id");
        }
        self.broadcast(now);
    }

    /// Runs `setup` on the game when it starts, before the first round.
    /// This is where a game written in Rust adds its own round types
    /// ([`Game::register_round_type`]) and hooks ([`Game::add_hook`]) to a
    /// hosted game.
    pub fn on_start(&mut self, setup: impl FnMut(&mut Game) + Send + 'static) {
        self.setup = Some(Box::new(setup));
    }

    /// Saves every fight from now on as a replay file in `dir`, and the
    /// whole match so far as `match.json` after every change of round, so
    /// a server that stops can [`Session::resume`] from it.
    pub fn record_to(&mut self, dir: impl Into<std::path::PathBuf>) {
        self.record_dir = Some(dir.into());
    }

    /// Where [`Session::record_to`] keeps the match in `dir`.
    pub fn match_file(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("match.json")
    }

    fn save_match(&self) {
        let (Some(dir), Some(game)) = (&self.record_dir, &self.game) else { return };
        let saved = SavedMatch {
            log: MatchLog::of(game),
            tokens: self.seats.iter().map(|s| s.token.clone()).collect(),
        };
        let path = Self::match_file(dir);
        let result = std::fs::create_dir_all(dir).and_then(|_| {
            let json = serde_json::to_string(&saved).map_err(std::io::Error::other)?;
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, json)?;
            std::fs::rename(tmp, &path)
        });
        if let Err(e) = result {
            eprintln!("could not save the match to {}: {e}", path.display());
        }
    }

    /// Loads what [`Session::record_to`] saved in `dir`.
    pub fn load_match(dir: &std::path::Path) -> Result<SavedMatch, String> {
        let path = Self::match_file(dir);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Picks up a saved match instead of waiting in the lobby: the game is
    /// replayed to where it was (running the [`Session::on_start`] setup
    /// first), and its players can join again under their names with the
    /// tokens they were given. Until they do, their seats wait; seats that
    /// were bots stay bots. Call before anyone connects.
    pub fn resume(&mut self, saved: SavedMatch, now: u64) -> Result<(), MatchLogError> {
        assert!(!self.has_started(), "resume a session before it starts");
        let mut setup = self.setup.take();
        let game = saved.log.replay(self.content.clone(), |g| {
            if let Some(setup) = &mut setup {
                setup(g);
            }
        });
        self.setup = setup;
        let game = game?;
        self.seats = saved
            .log
            .names
            .iter()
            .enumerate()
            .map(|(i, name)| Seat {
                name: Some(name.clone()),
                conn: None,
                token: saved.tokens.get(i).cloned().flatten(),
            })
            .collect();
        self.config.seats = self.seats.len();
        self.phase = if game.is_over() {
            Phase::Over
        } else if game.in_round() {
            Phase::Planning { deadline: now + self.planning_ms(&game) }
        } else {
            Phase::Showing { until: now }
        };
        self.events_sent = game.events().len();
        self.game = Some(game);
        Ok(())
    }

    pub fn new(content: impl Into<Arc<ContentSet>>, config: SessionConfig) -> Self {
        assert!((1..=250).contains(&config.seats), "a session needs 1 to 250 seats");
        Session {
            content: content.into(),
            seats: vec![Seat::default(); config.seats],
            conns: BTreeMap::new(),
            game: None,
            record_dir: None,
            dev: false,
            setup: None,
            phase: Phase::Lobby,
            bot_rng: Rng::new(config.seed ^ 0xB075),
            token_rng: Rng::new(os_entropy()),
            events_sent: 0,
            views: BTreeMap::new(),
            full_fights: Default::default(),
            shown: (0, Vec::new()),
            outbox: Vec::new(),
            chat: true,
            spectator_delay_ms: 0,
            delayed: Default::default(),
            now: 0,
            closing: Vec::new(),
            paused_at: None,
            kicked: Default::default(),
            bot_difficulty: bot::Difficulty::Normal,
            chat_history: Default::default(),
            chat_times: BTreeMap::new(),
            chat_filter: None,
            config,
        }
    }

    pub fn game(&self) -> Option<&Game> {
        self.game.as_ref()
    }

    /// Names in each seat; `None` for open seats.
    pub fn seat_names(&self) -> Vec<Option<String>> {
        self.seats.iter().map(|s| s.name.clone()).collect()
    }

    /// Seats held by a connected person.
    pub fn players_connected(&self) -> usize {
        self.seats.iter().filter(|s| s.conn.is_some()).count()
    }

    /// Connections, players and spectators together.
    pub fn connections(&self) -> usize {
        self.conns.len()
    }

    /// Final places of the people (not bots) in a finished game, by name.
    /// Empty until the game is over.
    pub fn human_results(&self) -> Vec<(String, u32)> {
        let Some(game) = self.game.as_ref().filter(|g| g.is_over()) else { return Vec::new() };
        game.players()
            .iter()
            .filter(|p| self.seats[p.id.0 as usize].token.is_some())
            .filter_map(|p| Some((p.name.clone(), p.placement?)))
            .collect()
    }

    /// Whether someone joining now would get a seat.
    pub fn has_open_seat(&self) -> bool {
        !self.has_started() && self.seats.iter().any(|s| s.name.is_none())
    }

    pub fn is_over(&self) -> bool {
        self.phase == Phase::Over
    }

    pub fn has_started(&self) -> bool {
        self.phase != Phase::Lobby
    }

    /// Messages waiting to be delivered, in order.
    pub fn drain_outbox(&mut self) -> Vec<(ConnId, ServerMsg)> {
        std::mem::take(&mut self.outbox)
    }

    fn send(&mut self, conn: ConnId, message: ServerMsg) {
        let game_news = matches!(
            message,
            ServerMsg::State(_)
                | ServerMsg::Update(_)
                | ServerMsg::Events(_)
                | ServerMsg::Combat(_)
                | ServerMsg::CombatSeed { .. }
        );
        if self.spectator_delay_ms > 0 && game_news && self.conns.get(&conn) == Some(&Role::Spectator) {
            self.delayed.push_back((self.now + self.spectator_delay_ms, conn, message));
        } else {
            self.outbox.push((conn, message));
        }
    }

    /// How long players get to plan the current round: the content's time
    /// for its kind, or the server's.
    fn planning_ms(&self, game: &Game) -> u64 {
        let by_kind = &game.content().rules().planning_ms_by_kind;
        by_kind.get(game.round_kind()).copied().unwrap_or(self.config.planning_ms)
    }

    /// Lets go of held-back spectator messages that are due.
    fn release_delayed(&mut self) {
        while let Some((at, _, _)) = self.delayed.front() {
            if *at > self.now {
                break;
            }
            let (_, conn, message) = self.delayed.pop_front().unwrap();
            if self.conns.contains_key(&conn) {
                self.outbox.push((conn, message));
            }
        }
    }

    fn seat_of(&self, conn: ConnId) -> Option<usize> {
        match self.conns.get(&conn) {
            Some(Role::Seat(i)) => Some(*i),
            _ => None,
        }
    }

    fn is_bot(&self, seat: usize) -> bool {
        self.seats[seat].conn.is_none()
    }

    // ----- input from the transport ----------------------------------------

    /// Handles a message from a connection.
    pub fn handle(&mut self, conn: ConnId, message: ClientMsg, now: u64) {
        self.now = self.now.max(now);
        match message {
            ClientMsg::Join { name, version, token } => {
                if !self.check_version(conn, version) {
                    return;
                }
                if self.conns.contains_key(&conn) {
                    return self.send(conn, ServerMsg::Error { message: "this connection has already joined".into() });
                }
                let name = name.trim().chars().take(24).collect::<String>();
                if name.is_empty() {
                    return self.send(conn, ServerMsg::Error { message: "a name is required".into() });
                }
                if self.kicked.contains(&name) {
                    return self.send(conn, ServerMsg::Error { message: "you were removed from this game".into() });
                }
                // The seat this player left (name and token must match),
                // else an open seat (only before the game starts), else watch.
                let returning = self.seats.iter().position(|s| {
                    s.conn.is_none() && s.name.as_deref() == Some(name.as_str()) && s.token.is_some() && s.token == token
                });
                let open = if self.has_started() { None } else { self.seats.iter().position(|s| s.name.is_none()) };
                let role = match (returning, open) {
                    (Some(i), _) => {
                        self.seats[i].conn = Some(conn);
                        Role::Seat(i)
                    }
                    (None, Some(i)) => {
                        let token = format!("{:016x}{:016x}", self.token_rng.next_u64(), self.token_rng.next_u64());
                        self.seats[i] = Seat { name: Some(name), conn: Some(conn), token: Some(token) };
                        Role::Seat(i)
                    }
                    (None, None) => Role::Spectator,
                };
                self.welcome(conn, role, now);
                if !self.has_started() && self.seats.iter().all(|s| s.conn.is_some()) {
                    self.start(now);
                }
            }
            ClientMsg::Spectate { version } => {
                if self.check_version(conn, version) && !self.conns.contains_key(&conn) {
                    self.welcome(conn, Role::Spectator, now);
                }
            }
            ClientMsg::Start => {
                if self.has_started() {
                    self.send(conn, ServerMsg::Error { message: "the game has already started".into() });
                } else if self.seat_of(conn).is_none() {
                    self.send(conn, ServerMsg::Error { message: "only seated players can start the game".into() });
                } else {
                    self.start(now);
                }
            }
            ClientMsg::Dev(command) => self.dev_command(conn, command, now),
            ClientMsg::Options { full_fights } => {
                if full_fights {
                    self.full_fights.insert(conn);
                } else {
                    self.full_fights.remove(&conn);
                }
            }
            ClientMsg::ResendCombat { id } => {
                let (round, index) = (id >> 8, (id & 0xff) as usize);
                let role = self.conns.get(&conn).copied();
                let visible = self.shown.1.get(index).filter(|record| {
                    round == self.shown.0 && role.is_some_and(|role| may_see(role, record))
                });
                match visible {
                    Some(record) => {
                        let record = record.clone();
                        self.send(conn, ServerMsg::Combat(record));
                    }
                    None => self.send(conn, ServerMsg::Error { message: "that fight is no longer kept".into() }),
                }
            }
            ClientMsg::Chat { text } => {
                let Some(role) = self.conns.get(&conn).copied() else {
                    return self.send(conn, ServerMsg::Error { message: "join or spectate before chatting".into() });
                };
                if !self.chat {
                    return self.send(conn, ServerMsg::Error { message: "chat is turned off in this game".into() });
                }
                let Some(text) = crate::protocol::clean_chat(&text) else { return };
                let times = self.chat_times.entry(conn).or_default();
                times.retain(|t| now.saturating_sub(*t) < CHAT_WINDOW_MS);
                if times.len() >= CHAT_BURST {
                    return self.send(conn, ServerMsg::Error { message: "you are sending chat too fast; wait a moment".into() });
                }
                times.push_back(now);
                let text = match &self.chat_filter {
                    Some(filter) => match filter(&text).and_then(|t| crate::protocol::clean_chat(&t)) {
                        Some(text) => text,
                        None => return,
                    },
                    None => text,
                };
                let (from, seat) = match role {
                    Role::Seat(i) => (self.seats[i].name.clone().unwrap_or_default(), Some(PlayerId(i as u8))),
                    Role::Spectator => ("spectator".to_string(), None),
                };
                self.post_chat(ServerMsg::Chat { from, seat, text });
            }
            // The transport answers pings; nothing here depends on them.
            ClientMsg::ListRooms
            | ClientMsg::CreateRoom { .. }
            | ClientMsg::EnterRoom { .. }
            | ClientMsg::LeaveRoom
            | ClientMsg::Standings => {
                self.send(conn, ServerMsg::Error { message: "this server hosts a single game".into() });
            }
            ClientMsg::Ping => self.send(conn, ServerMsg::Pong),
            ClientMsg::Pong => {}
            ClientMsg::Command(command) => {
                let Some(seat) = self.seat_of(conn) else {
                    return self.send(conn, ServerMsg::Error { message: "spectators cannot act".into() });
                };
                if !matches!(self.phase, Phase::Planning { .. }) {
                    return self.send(conn, ServerMsg::Error { message: "no round is in progress".into() });
                }
                let game = self.game.as_mut().expect("a started session has a game");
                match game.apply(PlayerId(seat as u8), command) {
                    Ok(()) => {
                        self.bot_picks();
                        self.broadcast(now);
                        if self.paused_at.is_none() && self.game.as_ref().unwrap().all_ready() {
                            self.resolve(now);
                        }
                    }
                    Err(e) => self.send(conn, ServerMsg::Error { message: e.to_string() }),
                }
            }
        }
    }

    /// Call when a connection closes.
    pub fn disconnect(&mut self, conn: ConnId, now: u64) {
        self.now = self.now.max(now);
        self.delayed.retain(|(_, c, _)| *c != conn);
        self.chat_times.remove(&conn);
        self.views.remove(&conn);
        self.full_fights.remove(&conn);
        let Some(role) = self.conns.remove(&conn) else { return };
        let Role::Seat(seat) = role else { return };
        if !self.has_started() {
            self.seats[seat] = Seat::default();
            self.send_lobby();
            return;
        }
        // A bot keeps the seat warm until the player returns.
        self.seats[seat].conn = None;
        self.send_lobby();
        if matches!(self.phase, Phase::Planning { .. }) {
            let game = self.game.as_mut().unwrap();
            let id = PlayerId(seat as u8);
            if game.player(id).alive {
                bot::play_turn_as(game, id, &mut self.bot_rng, self.bot_difficulty);
            }
            self.bot_picks();
            self.broadcast(now);
            if self.paused_at.is_none() && self.game.as_ref().unwrap().all_ready() {
                self.resolve(now);
            }
        }
    }

    /// Advances timers. Call regularly with a monotonic time in milliseconds.
    pub fn tick(&mut self, now: u64) {
        self.now = self.now.max(now);
        self.release_delayed();
        if self.paused_at.is_some() {
            return;
        }
        match self.phase {
            Phase::Planning { deadline } if now >= deadline => self.resolve(now),
            Phase::Showing { until } if now >= until => self.next_round(now),
            _ => {}
        }
    }

    // ----- internals --------------------------------------------------------

    fn check_version(&mut self, conn: ConnId, version: u32) -> bool {
        if version != PROTOCOL_VERSION {
            let message = format!("protocol version {version} is not supported; the server speaks {PROTOCOL_VERSION}");
            self.send(conn, ServerMsg::Error { message });
            return false;
        }
        true
    }

    fn welcome(&mut self, conn: ConnId, role: Role, now: u64) {
        self.conns.insert(conn, role);
        let seat = match role {
            Role::Seat(i) => Some(PlayerId(i as u8)),
            Role::Spectator => None,
        };
        let token = match role {
            Role::Seat(i) => self.seats[i].token.clone(),
            Role::Spectator => None,
        };
        let content = Box::new(ContentInfo::from_content(&self.content));
        self.send(conn, ServerMsg::Welcome { version: PROTOCOL_VERSION, seat, token, content });
        for message in self.chat_history.clone() {
            self.send(conn, message);
        }
        self.send_lobby();
        if self.game.is_some() {
            let view = self.view(seat, now);
            self.views.insert(conn, view.clone());
            self.send(conn, ServerMsg::State(view));
        }
    }

    fn send_lobby(&mut self) {
        let seats: Vec<Option<String>> = self.seats.iter().map(|s| s.name.clone()).collect();
        let started = self.has_started();
        for conn in self.conns.keys().copied().collect::<Vec<_>>() {
            self.send(conn, ServerMsg::Lobby { seats: seats.clone(), started });
        }
    }

    fn start(&mut self, now: u64) {
        for (i, seat) in self.seats.iter_mut().enumerate() {
            if seat.name.is_none() {
                seat.name = Some(format!("Bot {}", i + 1));
            }
        }
        let names: Vec<&str> = self.seats.iter().map(|s| s.name.as_deref().unwrap()).collect();
        let mut game = Game::new(self.content.clone(), self.config.seed, &names);
        if let Some(setup) = &mut self.setup {
            setup(&mut game);
        }
        self.game = Some(game);
        self.phase = Phase::Showing { until: now };
        self.send_lobby();
        self.next_round(now);
    }

    fn next_round(&mut self, now: u64) {
        let game = self.game.as_mut().expect("a started session has a game");
        if let Err(e) = game.begin_round() {
            self.phase = Phase::Over;
            for conn in self.conns.keys().copied().collect::<Vec<_>>() {
                self.send(conn, ServerMsg::Error { message: format!("the game cannot continue: {e}") });
            }
            return;
        }
        for id in game.alive() {
            if self.seats[id.0 as usize].conn.is_none() {
                bot::play_turn_as(game, id, &mut self.bot_rng, self.bot_difficulty);
            }
        }
        let planning = self.planning_ms(self.game.as_ref().unwrap());
        self.phase = Phase::Planning { deadline: now + planning };
        self.bot_picks();
        self.save_match();
        self.broadcast(now);
        if self.paused_at.is_none() && self.game.as_ref().unwrap().all_ready() {
            self.resolve(now);
        }
    }

    /// Lets bots make any picks the round is waiting on them for.
    fn bot_picks(&mut self) {
        let game = self.game.as_mut().unwrap();
        for _ in 0..self.seats.len() * 2 {
            let bots: Vec<PlayerId> =
                game.awaiting().into_iter().filter(|id| self.seats[id.0 as usize].conn.is_none()).collect();
            if bots.is_empty() {
                break;
            }
            for id in bots {
                bot::make_pick_as(game, id, &mut self.bot_rng, self.bot_difficulty);
            }
        }
    }

    fn resolve(&mut self, now: u64) {
        let game = self.game.as_mut().unwrap();
        let round_played = game.round_index();
        game.finish_round().expect("a round is in progress");
        let combats = game.combats().to_vec();
        let longest = combats.iter().map(|c| c.duration_ms as u64).max();
        self.phase = if game.is_over() {
            Phase::Over
        } else {
            let wait = match longest {
                Some(ms) if self.config.wait_for_playback => ms + self.config.after_combat_ms,
                _ => 0,
            };
            Phase::Showing { until: now + wait }
        };
        if let Some(dir) = &self.record_dir {
            let game = self.game.as_ref().unwrap();
            let content = ContentInfo::from_content(game.content());
            for record in &combats {
                let away = match &record.away {
                    Opponent::Player(p) => game.player(*p).name.clone(),
                    Opponent::Ghost(p) => format!("{} (ghost)", game.player(*p).name),
                    Opponent::Neutral => "the neutral camp".to_string(),
                };
                let title = format!("{} vs {}", game.player(record.home).name, away);
                let path = dir.join(crate::replay::file_name(round_played, &title));
                let saved = std::fs::create_dir_all(dir)
                    .and_then(|_| ReplayFile::new(title, content.clone(), record.clone()).save(&path));
                if let Err(e) = saved {
                    eprintln!("could not save replay {}: {e}", path.display());
                }
            }
        }
        // Fights first, so a client has them before the state says to show them.
        // A fight goes out as its teams and seed unless the client asked for
        // the whole log: fights are deterministic, so the client fights it
        // again. That is a few hundred bytes instead of a few hundred
        // thousand.
        let round = round_played as u32 & 0x00ff_ffff;
        let seeds: Vec<ServerMsg> = combats
            .iter()
            .take(256)
            .enumerate()
            .map(|(index, record)| ServerMsg::CombatSeed {
                id: round << 8 | index as u32,
                events: record.events.len() as u32,
                checksum: events_checksum(&record.events),
                record: CombatRecord { events: Vec::new(), ..record.clone() },
            })
            .collect();
        for (conn, role) in self.conns.clone() {
            for (index, record) in combats.iter().enumerate() {
                if !may_see(role, record) {
                    continue;
                }
                match seeds.get(index).filter(|_| !self.full_fights.contains(&conn)) {
                    Some(seed) => self.send(conn, seed.clone()),
                    None => self.send(conn, ServerMsg::Combat(record.clone())),
                }
            }
        }
        self.shown = (round, combats);
        self.save_match();
        self.broadcast(now);
    }

    /// Sends everyone the events they have not seen and a fresh snapshot.
    fn broadcast(&mut self, now: u64) {
        let game = self.game.as_ref().unwrap();
        let new_events = game.events()[self.events_sent..].to_vec();
        self.events_sent = game.events().len();
        for (conn, role) in self.conns.clone() {
            let viewer = match role {
                Role::Seat(i) => Some(PlayerId(i as u8)),
                Role::Spectator => None,
            };
            let events: Vec<_> = new_events.iter().filter(|e| visible_to(e, viewer)).cloned().collect();
            if !events.is_empty() {
                self.send(conn, ServerMsg::Events(events));
            }
            // Only what changed since the view this connection has.
            let view = self.view(viewer, now);
            match self.views.get(&conn) {
                Some(last) => {
                    if let Some(update) = last.diff(&view) {
                        self.send(conn, ServerMsg::Update(update));
                    }
                }
                None => self.send(conn, ServerMsg::State(view.clone())),
            }
            self.views.insert(conn, view);
        }
    }

    fn view(&self, viewer: Option<PlayerId>, now: u64) -> GameView {
        let game = self.game.as_ref().expect("a started session has a game");
        // While paused, time stands still at the moment of pausing.
        let now = self.paused_at.unwrap_or(now);
        let phase = match self.phase {
            Phase::Lobby => ViewPhase::Lobby,
            Phase::Planning { deadline } => ViewPhase::Planning { remaining_ms: deadline.saturating_sub(now) },
            Phase::Showing { until } => ViewPhase::Showing { remaining_ms: until.saturating_sub(now) },
            Phase::Over => ViewPhase::Over { winner: game.winner() },
        };
        let players = game
            .players()
            .iter()
            .cloned()
            .map(|mut p| {
                if Some(p.id) != viewer {
                    p.shop.clear();
                }
                p
            })
            .collect();
        GameView {
            phase,
            round_index: game.round_index(),
            round_name: game.round_name().to_string(),
            round_kind: game.round_kind().to_string(),
            stage: game.stage(),
            you: viewer,
            players,
            awaiting: game.awaiting(),
            options: viewer.map(|id| game.options(id)).unwrap_or_default(),
            status: game.players().iter().map(|p| status(game, p.id)).collect(),
            paused: self.paused_at.is_some(),
            pool: game.pool().clone(),
        }
    }

    /// Whether a seat is currently played by a bot.
    pub fn seat_is_bot(&self, seat: usize) -> bool {
        self.is_bot(seat)
    }
}

/// The server's figures for one player (see [`PlayerStatus`]).
fn status(game: &Game, id: PlayerId) -> PlayerStatus {
    let levels = &game.content().rules().levels;
    let level = game.player(id).level as usize;
    PlayerStatus {
        player: id,
        board_limit: game.board_limit(id),
        xp_to_next: levels.get(level.saturating_sub(1)).filter(|_| level < levels.len()).map(|l| l.xp_to_next),
        // Content is checked when it is loaded, so this cannot fail for a
        // board the game itself built.
        traits: game.content().active_traits(&game.team_setup(id).units).unwrap_or_default(),
        rerolls_left: game.rerolls_left(id),
    }
}
