//! A whole game played by bots, driven by hand so every step is visible.
//!
//!     cargo run --example bot_game
//!
//! A game is a loop of rounds. In each one: `begin_round`, then players send
//! commands with `apply`, then `finish_round` settles it. Bots are ordinary
//! players that send the same commands a person's client would.

use autoengine::prelude::*;

fn main() {
    let content = autoengine::sample::content();
    let mut game = Game::new(content, 7, &["Ana", "Ben", "Cho", "Dee"]);
    let mut dice = Rng::new(7);

    while !game.is_over() {
        game.begin_round().expect("the schedule is valid");

        // A person's turn would be commands like these. They return an error
        // and change nothing when they are not allowed.
        let ana = PlayerId(0);
        if game.player(ana).placement.is_none() {
            let _ = game.apply(ana, Command::BuyUnit { slot: 0 });
        }

        // Let the bots do the rest of everyone's turn, including Ana's.
        for id in game.alive() {
            bot::play_turn(&mut game, id, &mut dice);
        }
        // Rounds that offer a choice wait for everyone to pick.
        for id in game.awaiting() {
            bot::make_pick(&mut game, id, &mut dice);
        }
        game.finish_round().expect("everyone has picked");

        // Everything that happened comes out as events.
        for event in game.drain_events() {
            match event {
                GameEvent::RoundStarted { name, kind, .. } => println!("round {name} ({kind})"),
                GameEvent::PlayerEliminated { player, placement } => {
                    println!("  {} is out in place {placement}", game.player(player).name)
                }
                GameEvent::GameOver { winner } => println!("{} wins", game.player(winner).name),
                _ => {}
            }
        }
    }
}
