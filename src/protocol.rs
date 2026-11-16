//! The messages a client and the server exchange.
//!
//! The server owns the game. A client only sends what its player wants to
//! do ([`ClientMsg`]) and draws what the server reports back
//! ([`ServerMsg`]). Messages are plain serde types; on the wire each one is
//! a single line of JSON, which [`encode`] and [`decode`] handle.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::content::{ActiveTrait, AugmentDef, ContentSet, GameRules, ItemDef, TraitDef, UnitDef};
use crate::game::{CombatRecord, Command, GameEvent, Player, PlayerId};

/// Bumped whenever the messages change incompatibly.
pub const PROTOCOL_VERSION: u32 = 3;

/// The longest chat message the server passes on, in characters.
pub const MAX_CHAT_CHARS: usize = 200;

/// Tidies a chat message: control characters become spaces, runs of
/// whitespace shrink to one, and it is cut to [`MAX_CHAT_CHARS`]. `None`
/// if nothing is left.
pub fn clean_chat(text: &str) -> Option<String> {
    let spaced: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let words: Vec<&str> = spaced.split_whitespace().collect();
    let joined = words.join(" ");
    let cut: String = joined.chars().take(MAX_CHAT_CHARS).collect();
    (!cut.is_empty()).then_some(cut)
}

/// Shortcuts for trying things out while building a game. They act on the
/// sender's own seat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DevCommand {
    Gold(i64),
    /// Some of a currency other than gold, by id.
    Currency(String, i64),
    Xp(u32),
    /// Add a unit by id, without paying or touching the shop.
    Unit(String),
    Item(String),
    Augment(String),
    /// Lose this much health.
    Hurt(i64),
    /// End the planning phase now.
    Skip,
}

impl DevCommand {
    /// Reads a console line such as `gold 50`, `unit ashen_hound` or `skip`.
    pub fn parse(line: &str) -> Result<DevCommand, String> {
        const HELP: &str = "commands: gold N, give CURRENCY N, xp N, unit ID, item ID, augment ID, hurt N, skip";
        let mut words = line.split_whitespace();
        let command = words.next().ok_or(HELP)?;
        let arg = words.next();
        let number = || arg.and_then(|a| a.parse::<i64>().ok()).ok_or(format!("{command} needs a number"));
        let id = || arg.map(str::to_string).ok_or(format!("{command} needs an id"));
        Ok(match command {
            "gold" => DevCommand::Gold(number()?),
            "give" => {
                let amount = words.next().and_then(|a| a.parse::<i64>().ok()).ok_or("give needs a currency and a number")?;
                DevCommand::Currency(id()?, amount)
            }
            "xp" => DevCommand::Xp(number()?.clamp(0, 10_000) as u32),
            "unit" => DevCommand::Unit(id()?),
            "item" => DevCommand::Item(id()?),
            "augment" => DevCommand::Augment(id()?),
            "hurt" => DevCommand::Hurt(number()?),
            "skip" => DevCommand::Skip,
            _ => return Err(HELP.to_string()),
        })
    }
}

/// Client to server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Take a seat. To take back a seat after a disconnect, join with the
    /// same name and the token from the earlier [`ServerMsg::Welcome`].
    Join {
        name: String,
        version: u32,
        #[serde(default)]
        token: Option<String>,
    },
    /// Watch without playing.
    Spectate { version: u32 },
    /// Start the game now; empty seats are filled with bots.
    Start,
    /// A game command for the sender's seat.
    Command(Command),
    /// A developer cheat, accepted only by servers started with `--dev`.
    Dev(DevCommand),
    /// How this connection wants things sent. May be sent at any time.
    Options {
        /// Send every fight with its whole event log instead of as a seed
        /// to fight again (see [`ServerMsg::CombatSeed`]). For clients that
        /// do not embed the engine.
        #[serde(default)]
        full_fights: bool,
    },
    /// Asks for the whole event log of a fight that arrived as a
    /// [`ServerMsg::CombatSeed`] and did not play out the same here.
    ResendCombat { id: u32 },
    /// Says something to everyone in the game (see [`clean_chat`]).
    Chat { text: String },
    /// On a server hosting many games: asks for [`ServerMsg::Rooms`].
    ListRooms,
    /// On a server hosting many games: opens a new game and enters it.
    /// `seats` falls back to the server's choice.
    CreateRoom {
        name: String,
        #[serde(default)]
        seats: Option<usize>,
    },
    /// On a server hosting many games: enters a game; `Join` or `Spectate`
    /// then takes a seat in it or watches it. Sending `Join` without
    /// entering a room first enters the first game with an open seat, or a
    /// new one.
    EnterRoom { room: u32 },
    /// Leaves the current game (a bot keeps a seat that was taken) and goes
    /// back to the list of games.
    LeaveRoom,
    /// On a server that keeps a ladder: asks for [`ServerMsg::Standings`].
    Standings,
    /// "Are you there?" The other end answers [`ServerMsg::Pong`].
    Ping,
    /// The answer to [`ServerMsg::Ping`]. The transport sends it by itself.
    Pong,
}

/// The definitions a client needs to label and draw things.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentInfo {
    pub rules: GameRules,
    pub units: Vec<UnitDef>,
    pub traits: Vec<TraitDef>,
    pub items: Vec<ItemDef>,
    pub augments: Vec<AugmentDef>,
    /// Names and descriptions in other languages.
    #[serde(default)]
    pub translations: crate::content::Translations,
}

impl ContentInfo {
    pub fn from_content(content: &ContentSet) -> Self {
        ContentInfo {
            rules: content.rules().clone(),
            units: content.units().cloned().collect(),
            traits: content.traits().cloned().collect(),
            items: content.items().cloned().collect(),
            augments: content.augments().cloned().collect(),
            translations: content.translations().clone(),
        }
    }

    /// Rebuilds a content set for lookups on the client.
    pub fn into_content(self) -> Result<ContentSet, crate::content::ContentError> {
        ContentSet::new(self.units, self.traits, self.items, self.rules)?
            .with_augments(self.augments)?
            .with_translations(self.translations)
    }
}

/// Where the game is in its cycle, with time left where it applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewPhase {
    /// Waiting for players before the game starts.
    Lobby,
    /// Players are shopping and arranging their boards.
    Planning { remaining_ms: u64 },
    /// The round has resolved and clients are showing its fights.
    Showing { remaining_ms: u64 },
    Over { winner: Option<PlayerId> },
}

/// Figures about a player that follow from the rules. The server works them
/// out and sends them, so clients show exactly what the server will use and
/// never compute game values themselves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerStatus {
    pub player: PlayerId,
    /// Units the player may have on the board at their level.
    pub board_limit: usize,
    /// Experience needed for the next level, or `None` at the top level.
    pub xp_to_next: Option<u32>,
    /// Every trait on the player's board, with the breakpoint reached.
    pub traits: Vec<ActiveTrait>,
    /// Times the player may still reroll what the round offers them.
    #[serde(default)]
    pub rerolls_left: u32,
}

/// A snapshot of the game as one client is allowed to see it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameView {
    pub phase: ViewPhase,
    pub round_index: usize,
    pub round_name: String,
    pub round_kind: String,
    pub stage: u32,
    /// The viewer's seat; `None` for spectators.
    pub you: Option<PlayerId>,
    /// Every player. Other players' shops are hidden.
    pub players: Vec<Player>,
    /// Players the round is waiting on for a pick.
    pub awaiting: Vec<PlayerId>,
    /// What the round offers the viewer; `None` entries are taken.
    pub options: Vec<Option<String>>,
    /// One per player, in seat order.
    #[serde(default)]
    pub status: Vec<PlayerStatus>,
    /// The host has stopped the clock; `remaining_ms` stands still.
    #[serde(default)]
    pub paused: bool,
    /// Copies of each shop unit left in the shared pool (not counting those
    /// sitting in shops).
    #[serde(default)]
    pub pool: std::collections::BTreeMap<String, u32>,
    /// The [tiles](crate::tile) this round's fights start with, as the
    /// viewer's side sees its own half, so a board can be laid out for them.
    #[serde(default)]
    pub round_tiles: Vec<crate::tile::TilePlacement>,
}

impl GameView {
    /// The server's figures for one player.
    pub fn status_of(&self, player: PlayerId) -> Option<&PlayerStatus> {
        self.status.iter().find(|s| s.player == player)
    }
}

/// What changed in a [`GameView`] since the last one the client was sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewUpdate {
    pub phase: ViewPhase,
    pub round_index: usize,
    pub round_name: String,
    pub round_kind: String,
    pub stage: u32,
    /// Only the players that changed, each in full.
    pub players: Vec<Player>,
    pub awaiting: Vec<PlayerId>,
    pub options: Vec<Option<String>>,
    /// Only the statuses that changed.
    #[serde(default)]
    pub status: Vec<PlayerStatus>,
    #[serde(default)]
    pub paused: bool,
    /// The pool, only when it changed.
    #[serde(default)]
    pub pool: Option<std::collections::BTreeMap<String, u32>>,
    /// The round's tiles, only when they changed.
    #[serde(default)]
    pub round_tiles: Option<Vec<crate::tile::TilePlacement>>,
}

impl GameView {
    /// What a client holding `self` needs to arrive at `newer`, or `None`
    /// if nothing but the countdown differs.
    pub fn diff(&self, newer: &GameView) -> Option<ViewUpdate> {
        let players: Vec<Player> = newer
            .players
            .iter()
            .filter(|p| self.players.get(p.id.0 as usize) != Some(p))
            .cloned()
            .collect();
        let status: Vec<PlayerStatus> =
            newer.status.iter().filter(|s| self.status_of(s.player) != Some(s)).cloned().collect();
        let same_phase = std::mem::discriminant(&self.phase) == std::mem::discriminant(&newer.phase)
            && !matches!(newer.phase, ViewPhase::Over { .. });
        let unchanged = players.is_empty()
            && status.is_empty()
            && same_phase
            && self.round_index == newer.round_index
            && self.round_name == newer.round_name
            && self.round_kind == newer.round_kind
            && self.stage == newer.stage
            && self.awaiting == newer.awaiting
            && self.options == newer.options
            && self.paused == newer.paused
            && self.pool == newer.pool
            && self.round_tiles == newer.round_tiles;
        if unchanged {
            return None;
        }
        Some(ViewUpdate {
            phase: newer.phase,
            round_index: newer.round_index,
            round_name: newer.round_name.clone(),
            round_kind: newer.round_kind.clone(),
            stage: newer.stage,
            players,
            awaiting: newer.awaiting.clone(),
            options: newer.options.clone(),
            status,
            paused: newer.paused,
            pool: (self.pool != newer.pool).then(|| newer.pool.clone()),
            round_tiles: (self.round_tiles != newer.round_tiles).then(|| newer.round_tiles.clone()),
        })
    }

    /// Applies an update made by [`GameView::diff`].
    pub fn patch(&mut self, update: ViewUpdate) {
        self.phase = update.phase;
        self.round_index = update.round_index;
        self.round_name = update.round_name;
        self.round_kind = update.round_kind;
        self.stage = update.stage;
        self.awaiting = update.awaiting;
        self.options = update.options;
        self.paused = update.paused;
        if let Some(pool) = update.pool {
            self.pool = pool;
        }
        if let Some(tiles) = update.round_tiles {
            self.round_tiles = tiles;
        }
        for player in update.players {
            let seat = player.id.0 as usize;
            match self.players.get_mut(seat) {
                Some(slot) => *slot = player,
                None => self.players.push(player),
            }
        }
        for status in update.status {
            match self.status.iter_mut().find(|s| s.player == status.player) {
                Some(slot) => *slot = status,
                None => self.status.push(status),
            }
        }
    }
}

/// Server to client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Sent once after joining or spectating.
    /// `token` proves ownership of the seat when reconnecting.
    Welcome { version: u32, seat: Option<PlayerId>, token: Option<String>, content: Box<ContentInfo> },
    /// Who is seated. `None` is an open seat.
    Lobby { seats: Vec<Option<String>>, started: bool },
    /// The whole game as this client may see it. Sent after joining.
    State(GameView),
    /// What changed since the last `State` or `Update`. Sent instead of a
    /// whole `State` whenever the client already has one.
    Update(ViewUpdate),
    Events(Vec<GameEvent>),
    /// A fight the viewer should show, with its whole event log.
    Combat(CombatRecord),
    /// A fight the viewer should show, without its event log: `record` has
    /// the teams and the seed, and fighting it again with the content from
    /// `Welcome` gives the same `events` events. A client that cannot do
    /// that, or gets a different fight, asks with
    /// [`ClientMsg::ResendCombat`] (or opts out with [`ClientMsg::Options`]).
    /// `checksum` is [`events_checksum`] of the server's events: a client
    /// shows its own result only if every event matches.
    CombatSeed { id: u32, events: u32, checksum: u64, record: CombatRecord },
    /// "Are you there?" The other end answers [`ClientMsg::Pong`].
    Ping,
    /// The answer to [`ClientMsg::Ping`].
    Pong,
    /// The last message from this client was refused.
    Error { message: String },
    /// Someone said something. `seat` is `None` for spectators and for the
    /// server itself.
    Chat { from: String, seat: Option<PlayerId>, text: String },
    /// The games on a server hosting many.
    Rooms(Vec<RoomInfo>),
    /// The connection is now in this game, or in none.
    InRoom { room: Option<u32> },
    /// The ladder's best players, best first.
    Standings(Vec<crate::ladder::Standing>),
}

/// One game on a server hosting many, as the list of games shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomInfo {
    pub id: u32,
    pub name: String,
    pub seats: usize,
    /// Seats taken by people.
    pub players: usize,
    /// Everyone connected: players and spectators.
    pub connected: usize,
    pub started: bool,
    pub over: bool,
}

/// Whether an event may be shown to a viewer. Shops and private offers are
/// only visible to their owner.
pub fn visible_to(event: &GameEvent, viewer: Option<PlayerId>) -> bool {
    match event {
        GameEvent::ShopRolled { player, .. } => Some(*player) == viewer,
        GameEvent::ChoiceOffered { player: Some(player), .. } => Some(*player) == viewer,
        _ => true,
    }
}

/// A fingerprint of a fight's events (FNV-1a over their JSON), so a client
/// that fought a fight again can check it got exactly the server's events.
pub fn events_checksum(events: &[crate::combat::Event]) -> u64 {
    crate::rng::fnv1a(encode(&events).as_bytes())
}

/// Encodes a message as one line of JSON, without the newline.
pub fn encode<T: Serialize>(message: &T) -> String {
    serde_json::to_string(message).expect("protocol messages always serialize")
}

/// Decodes one line of JSON.
pub fn decode<T: DeserializeOwned>(line: &str) -> Result<T, String> {
    serde_json::from_str(line).map_err(|e| e.to_string())
}
