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
pub const PROTOCOL_VERSION: u32 = 1;

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

/// Server to client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Sent once after joining or spectating.
    /// `token` proves ownership of the seat when reconnecting.
    Welcome { version: u32, seat: Option<PlayerId>, token: Option<String>, content: Box<ContentInfo> },
    /// Who is seated. `None` is an open seat.
    Lobby { seats: Vec<Option<String>>, started: bool },
    State(GameView),
    Events(Vec<GameEvent>),
    /// A fight the viewer should show.
    Combat(CombatRecord),
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
