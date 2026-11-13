//! The messages a client and the server exchange.
//!
//! The server owns the game. A client only sends what its player wants to
//! do ([`ClientMsg`]) and draws what the server reports back
//! ([`ServerMsg`]). Messages are plain serde types; on the wire each one is
//! a single line of JSON, which [`encode`] and [`decode`] handle.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::content::{AugmentDef, ContentSet, GameRules, ItemDef, TraitDef, UnitDef};
use crate::game::{CombatRecord, Command, GameEvent, Player, PlayerId};

/// Bumped whenever the messages change incompatibly.
pub const PROTOCOL_VERSION: u32 = 2;

/// Shortcuts for trying things out while building a game. They act on the
/// sender's own seat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DevCommand {
    Gold(i64),
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
        const HELP: &str = "commands: gold N, xp N, unit ID, item ID, augment ID, hurt N, skip";
        let mut words = line.split_whitespace();
        let command = words.next().ok_or(HELP)?;
        let arg = words.next();
        let number = || arg.and_then(|a| a.parse::<i64>().ok()).ok_or(format!("{command} needs a number"));
        let id = || arg.map(str::to_string).ok_or(format!("{command} needs an id"));
        Ok(match command {
            "gold" => DevCommand::Gold(number()?),
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
}

impl ContentInfo {
    pub fn from_content(content: &ContentSet) -> Self {
        ContentInfo {
            rules: content.rules().clone(),
            units: content.units().cloned().collect(),
            traits: content.traits().cloned().collect(),
            items: content.items().cloned().collect(),
            augments: content.augments().cloned().collect(),
        }
    }

    /// Rebuilds a content set for lookups on the client.
    pub fn into_content(self) -> Result<ContentSet, crate::content::ContentError> {
        ContentSet::new(self.units, self.traits, self.items, self.rules)?.with_augments(self.augments)
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
        let same_phase = std::mem::discriminant(&self.phase) == std::mem::discriminant(&newer.phase)
            && !matches!(newer.phase, ViewPhase::Over { .. });
        let unchanged = players.is_empty()
            && same_phase
            && self.round_index == newer.round_index
            && self.round_name == newer.round_name
            && self.round_kind == newer.round_kind
            && self.stage == newer.stage
            && self.awaiting == newer.awaiting
            && self.options == newer.options;
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
        for player in update.players {
            let seat = player.id.0 as usize;
            match self.players.get_mut(seat) {
                Some(slot) => *slot = player,
                None => self.players.push(player),
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
    CombatSeed { id: u32, events: u32, record: CombatRecord },
    /// "Are you there?" The other end answers [`ClientMsg::Pong`].
    Ping,
    /// The answer to [`ClientMsg::Ping`].
    Pong,
    /// The last message from this client was refused.
    Error { message: String },
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

/// Encodes a message as one line of JSON, without the newline.
pub fn encode<T: Serialize>(message: &T) -> String {
    serde_json::to_string(message).expect("protocol messages always serialize")
}

/// Decodes one line of JSON.
pub fn decode<T: DeserializeOwned>(line: &str) -> Result<T, String> {
    serde_json::from_str(line).map_err(|e| e.to_string())
}
