//! Many games on one server.
//!
//! A [`Hub`] holds any number of rooms, each a [`Session`] of its own, and
//! sends every connection's messages to the room it is in. People list the
//! rooms ([`ClientMsg::ListRooms`]), open one ([`ClientMsg::CreateRoom`]) or
//! enter one ([`ClientMsg::EnterRoom`]), then `Join` or `Spectate` as they
//! would on a server with one game. A client that only knows about single
//! games still works: its `Join` lands in the first room with an open seat,
//! or in a new room.
//!
//! Rooms that are finished, or that nobody is in, are closed after a while.
//!
//! ```no_run
//! use autoengine::hub::{Hub, HubConfig};
//! use autoengine::net::Server;
//!
//! let hub = Hub::new(autoengine::sample::content(), HubConfig::default());
//! let server = Server::bind("0.0.0.0:7777").unwrap().spawn(hub);
//! # let _ = server;
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::content::ContentSet;
use crate::ladder::Ladder;
use crate::net::Host;
use crate::protocol::{ClientMsg, RoomInfo, ServerMsg};
use crate::session::{ConnId, Session, SessionConfig};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubConfig {
    /// Settings for each new room. `seats` is the default when a room is
    /// opened without saying, and each room gets its own seed from `seed`.
    pub room: SessionConfig,
    /// Rooms open at once.
    pub max_rooms: usize,
    /// The most seats a room may ask for.
    pub max_seats: usize,
    /// Whether people may open rooms themselves. When false, rooms only
    /// open for `Join`s that find no open seat.
    pub allow_create: bool,
    /// How long a finished room, or one nobody is connected to, stays open.
    pub close_after_ms: u64,
}

impl Default for HubConfig {
    fn default() -> Self {
        HubConfig {
            room: SessionConfig::default(),
            max_rooms: 32,
            max_seats: 8,
            allow_create: true,
            close_after_ms: 120_000,
        }
    }
}

struct Room {
    name: String,
    session: Session,
    /// When the room was last seen with nobody in it or finished, if it is
    /// that way now.
    idle_since: Option<u64>,
}

/// Something done to every new room; see [`Hub::on_new_room`].
type RoomSetup = Box<dyn FnMut(u32, &mut Session) + Send>;

/// Many rooms, each a [`Session`]. Host it with [`crate::net::Server`].
pub struct Hub {
    content: Arc<ContentSet>,
    config: HubConfig,
    rooms: BTreeMap<u32, Room>,
    next_room: u32,
    /// The room each known connection is in, if any.
    conns: BTreeMap<ConnId, Option<u32>>,
    setup: Option<RoomSetup>,
    record_dir: Option<PathBuf>,
    outbox: Vec<(ConnId, ServerMsg)>,
    /// Ratings, and the file they are saved to after each game.
    ladder: Option<(Ladder, Option<PathBuf>)>,
    /// Rooms whose results the ladder already has.
    rated: std::collections::BTreeSet<u32>,
}

/// Rows of the ladder sent to a client that asks.
const STANDINGS_SHOWN: usize = 50;

impl Hub {
    pub fn new(content: impl Into<Arc<ContentSet>>, config: HubConfig) -> Hub {
        Hub {
            content: content.into(),
            config,
            rooms: BTreeMap::new(),
            next_room: 1,
            conns: BTreeMap::new(),
            setup: None,
            record_dir: None,
            outbox: Vec::new(),
            ladder: None,
            rated: Default::default(),
        }
    }

    /// Runs `setup` on every room as it opens, with the room's id: the
    /// place to call [`Session::on_start`], [`Session::allow_dev`],
    /// [`Session::set_bot_difficulty`] and so on.
    pub fn on_new_room(&mut self, setup: impl FnMut(u32, &mut Session) + Send + 'static) {
        self.setup = Some(Box::new(setup));
    }

    /// Records every room in its own folder, `room-<id>`, under `dir` (see
    /// [`Session::record_to`]).
    pub fn record_to(&mut self, dir: impl Into<PathBuf>) {
        self.record_dir = Some(dir.into());
    }

    /// Picks up every unfinished game recorded under `dir` (by
    /// [`Hub::record_to`], in `room-<id>` folders), each in a room with
    /// its old number, and keeps recording there. Players rejoin with
    /// [`ClientMsg::EnterRoom`] and their names and tokens; a client that
    /// reconnects does this by itself. Returns how many games were resumed.
    /// Call before anyone connects, after [`Hub::on_new_room`].
    pub fn resume(&mut self, dir: &std::path::Path, now: u64) -> Result<usize, String> {
        let mut resumed = 0;
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        };
        let mut found: Vec<(u32, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let id = name.strip_prefix("room-")?.parse().ok()?;
                Some((id, e.path()))
            })
            .collect();
        found.sort();
        for (id, room_dir) in found {
            let Ok(saved) = Session::load_match(&room_dir) else { continue };
            let mut config = self.config.room.clone();
            config.seats = saved.log.names.len();
            let mut session = Session::new(self.content.clone(), config);
            if let Some(setup) = &mut self.setup {
                setup(id, &mut session);
            }
            session.resume(saved, now).map_err(|e| format!("{}: {e}", room_dir.display()))?;
            if session.is_over() {
                continue;
            }
            session.record_to(&room_dir);
            self.rooms.insert(id, Room { name: format!("Game {id}"), session, idle_since: None });
            self.next_room = self.next_room.max(id + 1);
            resumed += 1;
        }
        self.record_dir = Some(dir.to_path_buf());
        Ok(resumed)
    }

    /// Rates the people in every game that finishes from now on (see
    /// [`crate::ladder`]), saving the ladder to `file` after each one if
    /// given. Players can ask for the standings with
    /// [`ClientMsg::Standings`].
    pub fn keep_ladder(&mut self, ladder: Ladder, file: Option<PathBuf>) {
        self.ladder = Some((ladder, file));
    }

    pub fn ladder(&self) -> Option<&Ladder> {
        self.ladder.as_ref().map(|(l, _)| l)
    }

    /// The rooms as the list of rooms shows them.
    pub fn rooms(&self) -> Vec<RoomInfo> {
        self.rooms
            .iter()
            .map(|(id, room)| RoomInfo {
                id: *id,
                name: room.name.clone(),
                seats: room.session.seat_names().len(),
                players: room.session.players_connected(),
                connected: room.session.connections(),
                started: room.session.has_started(),
                over: room.session.is_over(),
            })
            .collect()
    }

    /// A room's session.
    pub fn room(&self, id: u32) -> Option<&Session> {
        self.rooms.get(&id).map(|r| &r.session)
    }

    /// Removes a player by name from whichever rooms they are seated in
    /// (see [`Session::kick`]). Returns how many seats they lost.
    pub fn kick(&mut self, name: &str, now: u64) -> usize {
        self.rooms.values_mut().filter_map(|r| r.session.kick(name, now).then_some(())).count()
    }

    /// A room's session, to change, such as to pause it.
    pub fn room_mut(&mut self, id: u32) -> Option<&mut Session> {
        self.rooms.get_mut(&id).map(|r| &mut r.session)
    }

    /// Sends a chat message from the server to every room.
    pub fn announce(&mut self, text: &str) {
        for room in self.rooms.values_mut() {
            room.session.announce(text);
        }
    }

    fn error(&mut self, conn: ConnId, message: &str) {
        self.outbox.push((conn, ServerMsg::Error { message: message.to_string() }));
    }

    /// Opens a room, or explains why not.
    fn open(&mut self, name: &str, seats: Option<usize>) -> Result<u32, String> {
        if self.rooms.len() >= self.config.max_rooms {
            return Err("the server has as many games as it can hold; try again later".into());
        }
        let id = self.next_room;
        self.next_room += 1;
        let mut config = self.config.room.clone();
        config.seats = seats.unwrap_or(config.seats).clamp(1, self.config.max_seats.max(1));
        config.seed = config.seed.wrapping_add(id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut session = Session::new(self.content.clone(), config);
        if let Some(dir) = &self.record_dir {
            session.record_to(dir.join(format!("room-{id}")));
        }
        if let Some(setup) = &mut self.setup {
            setup(id, &mut session);
        }
        let name = crate::protocol::clean_chat(name).unwrap_or_else(|| format!("Game {id}"));
        let name: String = name.chars().take(40).collect();
        self.rooms.insert(id, Room { name, session, idle_since: None });
        Ok(id)
    }

    fn enter(&mut self, conn: ConnId, room: u32, now: u64) {
        self.leave(conn, now);
        self.conns.insert(conn, Some(room));
        self.outbox.push((conn, ServerMsg::InRoom { room: Some(room) }));
    }

    fn leave(&mut self, conn: ConnId, now: u64) {
        if let Some(Some(room)) = self.conns.insert(conn, None)
            && let Some(room) = self.rooms.get_mut(&room)
        {
            room.session.disconnect(conn, now);
        }
    }

    /// The first room someone joining now would get a seat in.
    fn open_seat(&self) -> Option<u32> {
        self.rooms.iter().find(|(_, r)| r.session.has_open_seat()).map(|(id, _)| *id)
    }
}

impl Host for Hub {
    fn handle(&mut self, conn: ConnId, message: ClientMsg, now: u64) {
        let room = self.conns.get(&conn).copied().flatten();
        match message {
            ClientMsg::ListRooms => {
                let rooms = self.rooms();
                self.conns.entry(conn).or_insert(None);
                self.outbox.push((conn, ServerMsg::Rooms(rooms)));
            }
            ClientMsg::CreateRoom { name, seats } => {
                if !self.config.allow_create {
                    return self.error(conn, "this server opens games by itself; join to be given a seat");
                }
                match self.open(&name, seats) {
                    Ok(id) => self.enter(conn, id, now),
                    Err(e) => self.error(conn, &e),
                }
            }
            ClientMsg::EnterRoom { room } => {
                if self.rooms.contains_key(&room) {
                    self.enter(conn, room, now);
                } else {
                    self.error(conn, "there is no game with that number");
                }
            }
            ClientMsg::LeaveRoom => {
                self.leave(conn, now);
                self.outbox.push((conn, ServerMsg::InRoom { room: None }));
            }
            ClientMsg::Standings => match &self.ladder {
                Some((ladder, _)) => {
                    let mut rows = ladder.standings();
                    rows.truncate(STANDINGS_SHOWN);
                    self.outbox.push((conn, ServerMsg::Standings(rows)));
                }
                None => self.error(conn, "this server keeps no ladder"),
            },
            ClientMsg::Ping => self.outbox.push((conn, ServerMsg::Pong)),
            ClientMsg::Pong => {}
            message => {
                let room = match (room, &message) {
                    (Some(room), _) => room,
                    // Quick play: a seat in the first game that has one.
                    (None, ClientMsg::Join { .. }) => {
                        let found = match self.open_seat() {
                            Some(id) => Ok(id),
                            None => self.open("", None),
                        };
                        match found {
                            Ok(id) => {
                                self.enter(conn, id, now);
                                id
                            }
                            Err(e) => return self.error(conn, &e),
                        }
                    }
                    (None, _) => return self.error(conn, "enter a game first (see ListRooms)"),
                };
                if let Some(r) = self.rooms.get_mut(&room) {
                    r.session.handle(conn, message, now);
                }
            }
        }
    }

    fn disconnect(&mut self, conn: ConnId, now: u64) {
        self.leave(conn, now);
        self.conns.remove(&conn);
    }

    fn tick(&mut self, now: u64) {
        let mut closing = Vec::new();
        for (id, room) in &mut self.rooms {
            room.session.tick(now);
            if room.session.is_over()
                && let Some((ladder, file)) = &mut self.ladder
                && self.rated.insert(*id)
            {
                ladder.record(&room.session.human_results());
                if let Some(file) = file
                    && let Err(e) = ladder.save(file)
                {
                    eprintln!("could not save the ladder to {}: {e}", file.display());
                }
            }
            let idle = room.session.is_over() || room.session.connections() == 0;
            match (idle, room.idle_since) {
                (false, _) => room.idle_since = None,
                (true, None) => room.idle_since = Some(now),
                (true, Some(since)) if now.saturating_sub(since) >= self.config.close_after_ms => closing.push(*id),
                (true, Some(_)) => {}
            }
        }
        for id in closing {
            self.rooms.remove(&id);
            for (conn, room) in &mut self.conns {
                if *room == Some(id) {
                    *room = None;
                    self.outbox.push((*conn, ServerMsg::InRoom { room: None }));
                }
            }
        }
    }

    fn drain_outbox(&mut self) -> Vec<(ConnId, ServerMsg)> {
        let mut out = std::mem::take(&mut self.outbox);
        for room in self.rooms.values_mut() {
            out.extend(room.session.drain_outbox());
        }
        out
    }

    fn take_closed(&mut self) -> Vec<ConnId> {
        let closed: Vec<ConnId> = self.rooms.values_mut().flat_map(|r| r.session.take_closed()).collect();
        for conn in &closed {
            self.conns.remove(conn);
        }
        closed
    }
}
