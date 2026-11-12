//! Tools for people making a game. Each one is a plain function, so it can
//! run from the command line or from a game's own tests.
//!
//! | Module | Command | What it does |
//! | --- | --- | --- |
//! | [`lint`](lint()) | `autoengine check` | finds likely mistakes in content |
//! | [`simulate`] | `autoengine sim` | plays bot games and reports balance |
//! | [`scenario`] | `autoengine test` | runs fights written as data and checks what was expected of them |
//! | [`scaffold`] | `autoengine new` | writes a small starter game to build on |
//! | [`mod@bench`] | `autoengine bench` | measures how fast fights and games run |

pub mod bench;
mod lint;
pub mod scaffold;
pub mod scenario;
mod sim;

pub use lint::{Warning, lint};
pub use sim::{SimReport, UnitRecord, simulate};
