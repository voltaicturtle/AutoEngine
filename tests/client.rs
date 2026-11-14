use std::sync::Arc;

use autoengine::client::ClientState;
use autoengine::combat::{EventKind, Team};
use autoengine::content::ContentSet;
use autoengine::game::{Command, PlayerId};
use autoengine::playback::{Flash, MOVE_ANIMATION_MS, Playback};
use autoengine::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg, ViewPhase, decode, encode};
use autoengine::session::{Session, SessionConfig};

fn content() -> Arc<ContentSet> {
    Arc::new(autoengine::sample::content())
}

/// Runs a two-seat session with one person, feeding everything the server
/// sends through the wire format into a client state.
fn play(rounds: usize) -> (Session, ClientState) {
    let config = SessionConfig { seats: 2, seed: 3, planning_ms: 30_000, wait_for_playback: false, after_combat_ms: 0 };
    let mut session = Session::new(content(), config);
    let mut client = ClientState::new();
    let pump = |session: &mut Session, client: &mut ClientState| {
        for (_, message) in session.drain_outbox() {
            client.apply(decode::<ServerMsg>(&encode(&message)).unwrap());
        }
    };
    session.handle(1, ClientMsg::Join { name: "Ana".into(), version: PROTOCOL_VERSION, token: None }, 0);
    session.handle(1, ClientMsg::Start, 0);
    pump(&mut session, &mut client);
    for now in 1..=rounds as u64 {
        // Buy whatever the first shop slot offers, then ready up.
        session.handle(1, ClientMsg::Command(Command::BuyUnit { slot: 0 }), now);
        session.handle(1, ClientMsg::Command(Command::Ready), now);
        session.tick(now);
        pump(&mut session, &mut client);
    }
    (session, client)
}

#[test]
fn client_state_mirrors_what_the_server_says() {
    let (session, mut client) = play(3);
    assert!(client.welcomed);
    assert_eq!(client.seat, Some(PlayerId(0)));
    assert_eq!(client.lobby, [Some("Ana".to_string()), Some("Bot 2".to_string())]);
    assert!(client.started);

    let game = session.game().unwrap();
    let view = client.view.as_ref().unwrap();
    assert_eq!(view.round_index, game.round_index());
    assert!(matches!(view.phase, ViewPhase::Planning { .. }));
    // The client's copy of our player matches the server's exactly.
    assert_eq!(client.me().unwrap(), game.player(PlayerId(0)));
    assert!(client.me().unwrap().units().count() >= 1, "the purchases went through");

    assert_eq!(client.combats.len(), 3, "one fight per round so far");
    assert!(client.log.iter().any(|l| l.starts_with("Round 1-1")));
    assert!(client.log.iter().any(|l| l.contains("the neutral camp")));
    assert_eq!(client.display_name("ashen_hound"), "Ashen Hound");
    assert_eq!(client.display_name("cinder_blade"), "Cinder Blade");
    assert_eq!(client.display_name("unknown_thing"), "unknown_thing");
    assert_eq!(client.player_name(PlayerId(1)), "Bot 2");

    // A refused command surfaces as an error the client can show.
    assert_eq!(client.error, None);
    client.apply(ServerMsg::Error { message: "nope".into() });
    assert_eq!(client.error.as_deref(), Some("nope"));
}

#[test]
fn playback_reproduces_the_fight_from_its_record() {
    // A real fight from a bot game, so there is plenty going on.
    let mut game = autoengine::game::Game::new(content(), 5, &["a", "b", "c", "d"]);
    let mut rng = autoengine::rng::Rng::new(6);
    let record = loop {
        autoengine::bot::play_round(&mut game, &mut rng).unwrap();
        if let Some(r) = game.combats().iter().find(|c| c.duration_ms > 5_000 && c.winner.is_some()) {
            break r.clone();
        }
    };

    let mut playback = Playback::new(&record);
    let spawned = record.events.iter().filter(|e| e.tick == 0 && matches!(e.kind, EventKind::Spawned { .. })).count();
    assert_eq!(playback.units().count(), spawned, "everyone is on the board at time zero");
    assert!(playback.units().all(|u| u.alive && u.health == u.max_health));
    assert!(!playback.is_finished());

    // Step through like a renderer would, checking invariants on the way.
    let mut saw_attack = false;
    let mut saw_damage = false;
    let mut t = 0;
    while !playback.is_finished() {
        t += 33;
        playback.advance_to(t);
        for u in playback.units() {
            assert!(u.health >= 0 && u.health <= u.max_health);
            assert!(u.shield >= 0);
            assert_eq!(u.alive, u.health > 0 || u.alive && u.max_health == 0);
        }
        for (age, flash) in playback.recent_flashes(100) {
            assert!(age <= 100);
            saw_attack |= matches!(flash, Flash::Attack { .. });
            saw_damage |= matches!(flash, Flash::Damage { .. });
        }
        playback.forget_flashes_older_than(1_000);
        assert!(t < record.duration_ms + 1_000, "playback should end with the fight");
    }
    assert!(saw_attack && saw_damage);
    assert!(t >= record.duration_ms);

    // The final picture agrees with the record: only the winner still stands.
    let winner = record.winner.unwrap();
    let standing: Vec<Team> = playback.units().filter(|u| u.alive).map(|u| u.team).collect();
    assert_eq!(standing.len() as u32, record.survivors);
    assert!(standing.iter().all(|team| *team == winner));
    assert_eq!(playback.winner(), Some(winner));
}

#[test]
fn moves_are_eased_between_hexes() {
    let (_, client) = play(1);
    let record = client.combats.front().unwrap();
    let Some(moved) = record.events.iter().find_map(|e| match &e.kind {
        EventKind::Moved { unit, from, to } => Some((e.tick * record.tick_ms, *unit, *from, *to)),
        _ => None,
    }) else {
        return; // nobody had to walk in this fight
    };
    let (at, unit, from, to) = moved;
    let mut playback = Playback::new(record);
    playback.advance_to(at);
    let u = playback.unit(unit).unwrap();
    assert_eq!(u.pos, to);
    assert_eq!(u.position_at(at), (from.q as f32, from.r as f32));
    assert_eq!(u.position_at(at + MOVE_ANIMATION_MS), (to.q as f32, to.r as f32));
    let (q, r) = u.position_at(at + MOVE_ANIMATION_MS / 2);
    assert!((q - (from.q + to.q) as f32 / 2.0).abs() < 0.01 && (r - (from.r + to.r) as f32 / 2.0).abs() < 0.01);
}
