//! Adding rules in Rust: a new round type and a rule that reacts to events.
//!
//!     cargo run --example custom_rules
//!
//! Scripts (see content/scripts/) do the same without rebuilding. Use Rust
//! when a rule needs more than the script functions offer.

use std::collections::BTreeMap;

use autoengine::content::{RoundDef, RoundKindDef, Schedule};
use autoengine::prelude::*;

/// A round with no fight: the player with the least health gets gold.
struct Charity {
    gold: i64,
}

impl RoundType for Charity {
    fn kind(&self) -> &str {
        "charity"
    }

    // `begin`, `awaiting`, `options` and `pick` have defaults. Only `finish`
    // is required.
    fn finish(&mut self, game: &mut Game) {
        let poorest = game.alive().into_iter().min_by_key(|id| game.player(*id).health);
        if let Some(id) = poorest {
            game.add_gold(id, self.gold);
        }
    }
}

/// A rule: every time a unit reaches two stars, its owner gets one gold.
struct UpgradeBounty;

impl GameHook for UpgradeBounty {
    fn on_event(&mut self, game: &mut Game, event: &GameEvent) {
        if let GameEvent::UnitUpgraded { player, star: 2, .. } = event {
            game.add_gold(*player, 1);
        }
    }
}

fn main() {
    // Use the sample content, with a charity round after every two fights.
    // In a real game this schedule would be in rounds.ron.
    let round = |name: &str, kind| RoundDef { name: name.into(), stage: 1, kind };
    let charity = RoundKindDef::Custom { kind: "charity".into(), params: BTreeMap::from([("gold".to_string(), 5)]) };
    let schedule = Schedule {
        rounds: vec![round("fight", RoundKindDef::Pvp), round("fight", RoundKindDef::Pvp), round("charity", charity)],
        repeat_from: 0,
    };
    let content = ContentSet::load_dir("content")
        .expect("run this from the repository root")
        .with_schedule(schedule)
        .expect("the schedule is valid");

    let mut game = Game::new(content, 3, &["Ana", "Ben", "Cho", "Dee"]);
    // A round type is registered as a function from the round's `params` to
    // a new round, so each round gets fresh state.
    game.register_round_type("charity", Box::new(|params| Box::new(Charity { gold: params["gold"] })));
    game.add_hook(Box::new(UpgradeBounty));

    let mut dice = Rng::new(3);
    for _ in 0..9 {
        if game.is_over() {
            break;
        }
        bot::play_round(&mut game, &mut dice).expect("the round types are registered");
        println!(
            "after {:<8} gold: {}",
            game.round_name(),
            game.players().iter().map(|p| format!("{} {}", p.name, p.gold)).collect::<Vec<_>>().join(", ")
        );
    }
}
