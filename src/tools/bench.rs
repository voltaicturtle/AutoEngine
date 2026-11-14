//! Speed measurements behind `autoengine bench`.
//!
//! The numbers depend on the machine and the build (use `--release`), so
//! they are for comparing before and after a change, not for tests.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::bot;
use crate::content::{ContentSet, MatchSetup};
use crate::game::Game;
use crate::rng::Rng;

/// How long a batch of work took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timing {
    /// Fights or games run.
    pub count: u32,
    pub elapsed: Duration,
    /// Simulated fight ticks or game rounds across the whole batch.
    pub steps: u64,
}

impl Timing {
    pub fn per_second(&self) -> f64 {
        self.count as f64 / self.elapsed.as_secs_f64().max(1e-9)
    }

    /// Average time for one fight or game, in microseconds.
    pub fn micros_each(&self) -> u64 {
        (self.elapsed.as_micros() / self.count.max(1) as u128) as u64
    }
}

/// Runs the same fight `count` times with different seeds.
pub fn fights(content: &ContentSet, setup: &MatchSetup, count: u32) -> Result<Timing, String> {
    let start = Instant::now();
    let mut steps = 0u64;
    for seed in 0..count {
        let mut combat = content.setup_combat(seed as u64 + 1, &setup.teams).map_err(|e| e.to_string())?;
        steps += combat.run().ticks as u64;
    }
    Ok(Timing { count, elapsed: start.elapsed(), steps })
}

/// Plays `count` whole games between bots.
pub fn games(content: impl Into<Arc<ContentSet>>, count: u32, players: usize) -> Timing {
    const NAMES: [&str; 8] = ["A", "B", "C", "D", "E", "F", "G", "H"];
    let content = content.into();
    let start = Instant::now();
    let mut steps = 0u64;
    for seed in 0..count {
        let mut game = Game::new(content.clone(), seed as u64 + 1, &NAMES[..players.clamp(2, 8)]);
        let mut rng = Rng::new(seed as u64 ^ 0xB07);
        while !game.is_over() && game.round_index() < 300 && bot::play_round(&mut game, &mut rng).is_ok() {
            steps += 1;
        }
    }
    Timing { count, elapsed: start.elapsed(), steps }
}
