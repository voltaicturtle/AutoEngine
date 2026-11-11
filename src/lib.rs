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

pub mod bot;
pub mod combat;
pub mod content;
pub mod effect;
pub mod game;
pub mod hex;
pub mod rng;
pub mod round;
pub mod stats;
