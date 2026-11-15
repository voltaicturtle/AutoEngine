//! Plays a game between bots, saves it as a match log, then plays the log
//! back round by round and checks it ends exactly where the game did.
//!
//! cargo run --example replay_a_match

use std::sync::Arc;

use autoengine::bot::{Difficulty, play_round_with};
use autoengine::matchlog::MatchLog;
use autoengine::prelude::*;

fn main() {
    let content = Arc::new(autoengine::sample::content());
    let mut game = Game::new(content.clone(), 42, &["Ann", "Bo", "Cy", "Di"]);
    let mut rng = Rng::new(7);
    // Two hard bots against two easy ones.
    let level = |id: PlayerId| if id.0 < 2 { Difficulty::Hard } else { Difficulty::Easy };
    while !game.is_over() {
        play_round_with(&mut game, &mut rng, level).unwrap();
    }

    let path = std::env::temp_dir().join("autoengine-example-match.json");
    MatchLog::of(&game).save(&path).unwrap();
    println!("saved {} inputs to {}", game.inputs().len(), path.display());

    let log = MatchLog::load(&path).unwrap();
    let mut round = 0;
    log.rounds(content.clone(), |_| {}, |g| {
        round += 1;
        let health: Vec<String> = g.players().iter().map(|p| format!("{} {}", p.name, p.health.max(0))).collect();
        println!("after round {round:>2}: {}", health.join(", "));
    })
    .unwrap();

    let again = log.replay(content, |_| {}).unwrap();
    assert_eq!(again.checksum(), game.checksum());
    let winner = again.player(again.winner().unwrap());
    println!("{} wins again: the replay matches the game", winner.name);
}
