//! Hosting a game inside your own program, with no network.
//!
//!     cargo run --example host_a_game
//!
//! A `Session` is a hosted game: seats, bots for the empty ones, round
//! timers and what each player is allowed to see. It does not know how
//! messages travel. The TCP server (`autoengine-server`) is a thin loop
//! around it, and this example is another: it plays the part of one
//! connected client itself.

use autoengine::prelude::*;
use autoengine::protocol::{ClientMsg, GameView, PROTOCOL_VERSION, ServerMsg, ViewPhase, ViewUpdate};

fn main() {
    let content = autoengine::sample::content();
    let config = SessionConfig { seats: 4, seed: 11, wait_for_playback: false, ..SessionConfig::default() };
    let mut session = Session::new(content, config);

    // Rust-side rules are added to a hosted game here.
    session.on_start(|game| println!("the game starts with {} players", game.players().len()));

    // Connections are just numbers the transport hands out. Time is passed
    // in as milliseconds, so the session never reads a clock itself.
    const ME: u64 = 1;
    let mut now = 0;
    session.handle(ME, ClientMsg::Join { name: "Ana".into(), version: PROTOCOL_VERSION, token: None }, now);
    session.handle(ME, ClientMsg::Start, now);

    let mut fights = 0;
    while !session.is_over() && now < 600_000 {
        now += 100;
        session.tick(now);
        // Messages for every connection queue up in the outbox.
        for (conn, message) in session.drain_outbox() {
            assert_eq!(conn, ME);
            match message {
                // The first `State` is the whole game as this player may see
                // it (their own shop, everyone's boards). After that come
                // `Update`s with only what changed; `ClientState::apply`
                // folds both. Here only the phase matters.
                ServerMsg::State(GameView { phase, .. }) | ServerMsg::Update(ViewUpdate { phase, .. }) => {
                    if let ViewPhase::Planning { .. } = phase {
                        // Skip the planning timer by readying up straight away.
                        // (Ana never buys a unit, so do not expect her to win.)
                        session.handle(ME, ClientMsg::Command(Command::Ready), now);
                    }
                }
                // Fights come as teams and a seed; `ClientState::apply`
                // fights them again to get the events.
                ServerMsg::CombatSeed { record, .. } => {
                    fights += 1;
                    if fights <= 5 {
                        println!("fight {fights}: {:?} wins in {} ms", record.winner, record.duration_ms);
                    }
                }
                _ => {}
            }
        }
    }
    let game = session.game().expect("the game started");
    match game.winner() {
        Some(winner) => println!("{} wins after {} rounds and {fights} fights shown to Ana", game.player(winner).name, game.round_index() + 1),
        None => println!("still going after {} rounds", game.round_index() + 1),
    }
}
