//! AutoEngine: a data-driven engine for autochess-style games.
//!
//! The crate is a headless simulation library: it does no rendering. A
//! server runs it and clients only draw what it reports. Everything is
//! deterministic (integer math and a seeded [`rng::Rng`]) and everything that
//! happens is reported as events, so fights can be replayed and streamed.
//!
//! # Quick start
//!
//! Take a game's content, set up two teams and let them fight:
//!
//! ```
//! use autoengine::prelude::*;
//!
//! let content = autoengine::sample::content();
//! let unit = |id: &str, pos| BoardUnit { unit: id.into(), star: 1, items: vec![], pos };
//! let home = TeamSetup { units: vec![unit("ironbark_warden", (3, 4))], bonuses: vec![] };
//! let away = TeamSetup { units: vec![unit("ashen_hound", (3, 4))], bonuses: vec![] };
//!
//! let mut combat = content.setup_combat(7, &[home, away]).unwrap();
//! let outcome = combat.run();
//! println!("winner: {:?} after {} ms", outcome.winner, outcome.duration_ms);
//! ```
//!
//! The `examples/` folder has longer ones: a whole game played by bots, a
//! round type and a rule written in Rust, a game hosted in-process, and a
//! server with two players over TCP.
//!
//! # Layers
//!
//! The modules form layers. Each one only uses the layers above it, so a
//! project can take the fight simulation alone, or everything up to the
//! lobby, without the rest.
//!
//! | Layer | Modules | What it gives you |
//! | --- | --- | --- |
//! | Foundations | [`rng`], [`hex`], [`stats`] | seeded dice, board geometry and pathfinding, integer stats |
//! | Fights | [`effect`], [`combat`] | abilities built from effect blocks, and the tick-based fight simulation |
//! | Content | [`content`], [`sample`] | units, traits, items, augments and rounds, defined in Rust; a complete sample game |
//! | Game | [`game`], [`round`], [`bot`], `script` | the lobby: shop, economy, round loop, pluggable round types, scripts, computer players |
//! | Hosting | [`protocol`], [`session`], [`net`], [`replay`] | messages, a hosted game with seats and timers, TCP, saved fights |
//! | Display | [`client`], [`playback`] | what a front end needs to show a game, with no rendering in it |
//! | Tools | [`tools`] | the command line, content checker, content tests, balance simulator, starter game, benchmarks |
//!
//! # Extending
//!
//! | To add | Use |
//! | --- | --- |
//! | units, traits, items, augments, rounds | [`content::ContentSet::builder`] (see [`sample`]) |
//! | an effect or condition the data model cannot express | [`content::GameBuilder::custom_effect`] and [`content::GameBuilder::custom_condition`] |
//! | a round type or rule without rebuilding | a script (see `script`, or run `autoengine api`) |
//! | a round type in Rust | [`round::RoundType`] and [`game::Game::register_round_type`] |
//! | a rule that reacts to events in Rust | [`game::GameHook`] and [`game::Game::add_hook`] |
//! | either of those on a hosted game | [`session::Session::on_start`] |
//! | a networked game | [`net::Server`] and [`net::GameClient`] |
//! | another transport than TCP | drive [`session::Session`] yourself: `handle`, `tick`, `drain_outbox` |
//! | another front end | [`client::ClientState`] and [`playback::Playback`] |
//!
//! # Cargo features
//!
//! * `scripting` (on by default): the `script` module and its Rhai
//!   interpreter. Without it the engine has no scripting dependency.

pub mod bot;
pub mod client;
pub mod combat;
pub mod content;
pub mod effect;
pub mod game;
pub mod hex;
pub mod net;
pub mod playback;
pub mod protocol;
pub mod replay;
pub mod rng;
pub mod round;
pub mod sample;
#[cfg(feature = "scripting")]
pub mod script;
pub mod session;
pub mod stats;
pub mod tools;

/// The types most programs built on the engine need, in one import:
/// `use autoengine::prelude::*;`.
pub mod prelude {
    pub use crate::bot;
    pub use crate::combat::{Combat, CombatConfig, Event, EventKind, Outcome, Team, UnitId};
    pub use crate::content::{BoardUnit, ContentError, ContentSet, MatchSetup, TeamSetup};
    pub use crate::game::{Command, CommandError, Game, GameEvent, GameHook, Opponent, Player, PlayerId, Slot};
    pub use crate::hex::Hex;
    pub use crate::rng::Rng;
    pub use crate::round::RoundType;
    pub use crate::net::{GameClient, Server, ServerConfig};
    pub use crate::session::{Session, SessionConfig};
    pub use crate::stats::Stat;
}
