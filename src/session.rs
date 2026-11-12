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
use crate::game::{Game, Opponent, PlayerId};
use crate::protocol::{ClientMsg, ContentInfo, GameView, PROTOCOL_VERSION, ServerMsg, ViewPhase, visible_to};
use crate::rng::Rng;

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
    phase: Phase,
    bot_rng: Rng,
    /// Source of seat tokens; seeded from the clock, never from the game seed.
    token_rng: Rng,
    events_sent: usize,
    outbox: Vec<(ConnId, ServerMsg)>,
}

impl Session {
    pub fn new(content: impl Into<Arc<ContentSet>>, config: SessionConfig) -> Self {
        assert!((1..=250).contains(&config.seats), "a session needs 1 to 250 seats");
        Session {
            content: content.into(),
            seats: vec![Seat::default(); config.seats],
            conns: BTreeMap::new(),
            game: None,
            phase: Phase::Lobby,
            bot_rng: Rng::new(config.seed ^ 0xB075),
            token_rng: Rng::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0)
                    ^ (std::process::id() as u64) << 32,
            ),
            events_sent: 0,
            outbox: Vec::new(),
            config,
        }
    }

    pub fn game(&self) -> Option<&Game> {
        self.game.as_ref()
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
        self.outbox.push((conn, message));
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
        match message {
            ClientMsg::Join { name, version, token } => {
                if !self.check_version(conn, version) || self.conns.contains_key(&conn) {
                    return;
                }
                let name = name.trim().chars().take(24).collect::<String>();
                if name.is_empty() {
                    return self.send(conn, ServerMsg::Error { message: "a name is required".into() });
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
                        if self.game.as_ref().unwrap().all_ready() {
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
                bot::play_turn(game, id, &mut self.bot_rng);
            }
            self.bot_picks();
            self.broadcast(now);
            if self.game.as_ref().unwrap().all_ready() {
                self.resolve(now);
            }
        }
    }

    /// Advances timers. Call regularly with a monotonic time in milliseconds.
    pub fn tick(&mut self, now: u64) {
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
        self.send_lobby();
        if self.game.is_some() {
            let view = self.view(seat, now);
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
        self.game = Some(Game::new(self.content.clone(), self.config.seed, &names));
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
                bot::play_turn(game, id, &mut self.bot_rng);
            }
        }
        self.phase = Phase::Planning { deadline: now + self.config.planning_ms };
        self.bot_picks();
        self.broadcast(now);
        if self.game.as_ref().unwrap().all_ready() {
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
                bot::make_pick(game, id, &mut self.bot_rng);
            }
        }
    }

    fn resolve(&mut self, now: u64) {
        let game = self.game.as_mut().unwrap();
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
        // Fights first, so a client has them before the state says to show them.
        for (conn, role) in self.conns.clone() {
            for record in &combats {
                let involved = match role {
                    Role::Spectator => true,
                    Role::Seat(i) => {
                        let me = PlayerId(i as u8);
                        record.home == me || record.away == Opponent::Player(me)
                    }
                };
                if involved {
                    self.send(conn, ServerMsg::Combat(record.clone()));
                }
            }
        }
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
            let view = self.view(viewer, now);
            self.send(conn, ServerMsg::State(view));
        }
    }

    fn view(&self, viewer: Option<PlayerId>, now: u64) -> GameView {
        let game = self.game.as_ref().expect("a started session has a game");
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
        }
    }

    /// Whether a seat is currently played by a bot.
    pub fn seat_is_bot(&self, seat: usize) -> bool {
        self.is_bot(seat)
    }
}
