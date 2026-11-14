//! A server and two players over real TCP, in one program.
//!
//!     cargo run --example network_game
//!
//! In a real game the server is `autoengine-server` on one machine and each
//! player runs a client somewhere else. The code is the same: `Server` on
//! one side, `GameClient` on the other.

use std::time::Duration;

use autoengine::net::{GameClient, Server};
use autoengine::prelude::*;
use autoengine::protocol::ViewPhase;

fn main() {
    // The server side: a session (the game) hosted on a socket. Port 0
    // picks any free port.
    let content = ContentSet::load_dir("content").expect("run this from the repository root");
    // No pause for watching fights, since nobody here is watching.
    let config = SessionConfig { seats: 4, planning_ms: 5_000, wait_for_playback: false, ..SessionConfig::default() };
    let session = Session::new(content, config);
    let server = Server::bind("127.0.0.1:0").expect("a free port").spawn(session);
    println!("server listening on {}", server.addr());

    // The client side. `join` returns at once and connects in the background.
    let addr = server.addr().to_string();
    let mut players = [GameClient::join(&addr, "Ana"), GameClient::join(&addr, "Ben")];

    // Everything a client knows is in `client.state`, kept up to date by
    // `update`, which never blocks. Call it every frame.
    let mut ready_for = [None, None];
    let mut started = false;
    loop {
        for (i, player) in players.iter_mut().enumerate() {
            player.update();
            let state = &player.state;
            if !state.welcomed {
                continue;
            }
            if !started {
                // Start once both are in; the two empty seats get bots.
                if state.lobby.iter().flatten().count() == 2 {
                    started = player.start();
                }
                continue;
            }
            let Some(view) = &state.view else { continue };
            if let ViewPhase::Over { winner } = view.phase {
                let name = winner.map(|w| view.players[w.0 as usize].name.clone());
                println!("game over: {} wins", name.unwrap_or_else(|| "nobody".into()));
                let stats = server.stats();
                println!("server sent {} messages, {} KB in all", stats.messages_out, stats.bytes_out / 1024);
                return;
            }
            let round = view.round_index;
            let me = state.seat.expect("seated");
            let picking = view.awaiting.contains(&me);
            if matches!(view.phase, ViewPhase::Planning { .. }) && (ready_for[i] != Some(round) || picking) {
                // A real client turns clicks into commands like these.
                if picking {
                    player.command(Command::Pick { option: 0 });
                } else {
                    player.command(Command::BuyUnit { slot: 0 });
                    player.command(Command::Ready);
                    ready_for[i] = Some(round);
                    if i == 0 && round % 5 == 0 {
                        let gold = player.state.me().map_or(0, |p| p.gold);
                        println!("round {round}: Ana has {gold} gold");
                    }
                }
            }
            // Fights arrive in `state.combats` with their full event log,
            // ready to draw. Here they are just counted and dropped.
            player.state.combats.clear();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
