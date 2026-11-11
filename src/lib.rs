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

pub mod combat;
pub mod effect;
pub mod hex;
pub mod rng;
pub mod stats;
