//! AutoEngine: a data-driven engine for autochess-style games.
//!
//! The crate is a headless simulation library: it does no rendering and no
//! I/O. A server runs it and clients only draw what it reports. Everything is
//! deterministic (integer math and a seeded [`rng::Rng`]) and everything that
//! happens is reported as events, so fights can be replayed and streamed.
//!
//! * [`hex`]: board geometry and pathfinding.
//! * [`stats`]: integer unit stats and modifiers.
//! * [`effect`]: the data model for abilities and passives.
//! * [`combat`]: the tick-based fight simulation.
//! * [`content`]: units, traits, items, augments and the round schedule,
//!   loaded from data files.
//! * [`game`]: the lobby: players, shop, economy and the round loop.
//! * [`round`]: pluggable round types.
//! * [`bot`]: simple computer players.
//! * [`protocol`]: the messages between a client and the server.
//! * [`session`]: a hosted game with seats, bots and timers, independent of
//!   any transport.
//! * [`net`]: the TCP transport for both ends.
//! * [`client`] and [`playback`]: the display-side model a client builds
//!   from server messages, with no rendering in it.

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
pub mod script;
pub mod session;
pub mod stats;
pub mod tools;
