use std::sync::Arc;

use autoengine::content::ContentSet;
use autoengine::game::{Game, Grant, Input, PlayerId};
use autoengine::matchlog::{MatchLog, MatchLogError};
use autoengine::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg};
use autoengine::rng::Rng;
use autoengine::session::{Session, SessionConfig};

fn content() -> Arc<ContentSet> {
    Arc::new(autoengine::sample::content())
}

fn played(rounds: usize) -> Game {
    let mut game = Game::new(content(), 9, &["Ann", "Bo", "Cy", "Di"]);
    let mut rng = Rng::new(4);
    for _ in 0..rounds {
        autoengine::bot::play_round(&mut game, &mut rng).unwrap();
    }
    game
}

#[test]
fn a_replayed_match_ends_in_the_same_state() {
    let mut game = played(6);
    game.grant(PlayerId(1), Grant::Gold(25)).unwrap();
    game.grant(PlayerId(2), Grant::Unit("ashen_hound".into())).unwrap();
    assert!(game.grant(PlayerId(2), Grant::Unit("nobody".into())).is_err());
    let log = MatchLog::of(&game);
    assert!(log.inputs.iter().any(|i| matches!(i, Input::Grant { .. })));
    assert_eq!(log.inputs.iter().filter(|i| matches!(i, Input::Grant { .. })).count(), 2, "refused grants are not kept");
    let again = log.replay(content(), |_| {}).unwrap();
    assert_eq!(again.checksum(), game.checksum());
    assert_eq!(again.players(), game.players());

    // Through a file too.
    let path = std::env::temp_dir().join(format!("autoengine-match-{}.json", std::process::id()));
    log.save(&path).unwrap();
    assert_eq!(MatchLog::load(&path).unwrap(), log);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn rounds_can_be_stepped_through() {
    let game = played(5);
    let mut health = Vec::new();
    MatchLog::of(&game)
        .rounds(content(), |_| {}, |g| health.push(g.players().iter().map(|p| p.health).sum::<i64>()))
        .unwrap();
    assert_eq!(health.len(), 5);
    assert!(health.windows(2).all(|w| w[1] <= w[0]), "health only goes down: {health:?}");
}

#[test]
fn a_changed_match_or_content_is_caught() {
    let game = played(4);
    let log = MatchLog::of(&game);

    let mut tampered = log.clone();
    tampered.inputs.retain(|i| !matches!(i, Input::Command { .. }));
    assert!(matches!(tampered.replay(content(), |_| {}), Err(MatchLogError::Diverged | MatchLogError::Refused { .. })));

    let mut schedule = autoengine::sample::schedule();
    schedule.rounds.swap(0, 1);
    let changed = autoengine::sample::content().with_schedule(schedule).unwrap();
    assert!(matches!(log.replay(changed, |_| {}), Err(MatchLogError::Content)));

    let mut future = log.clone();
    future.version = 99;
    assert!(matches!(future.replay(content(), |_| {}), Err(MatchLogError::Version { found: 99 })));
}

#[test]
fn a_session_resumes_where_it_stopped() {
    let dir = std::env::temp_dir().join(format!("autoengine-resume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = SessionConfig { seats: 3, seed: 5, planning_ms: 1_000, wait_for_playback: false, after_combat_ms: 0 };
    let mut first = Session::new(content(), config.clone());
    first.record_to(&dir);
    first.handle(1, ClientMsg::Join { name: "Ann".into(), version: PROTOCOL_VERSION, token: None }, 0);
    let token = first
        .drain_outbox()
        .into_iter()
        .find_map(|(_, m)| match m {
            ServerMsg::Welcome { token, .. } => token,
            _ => None,
        })
        .unwrap();
    first.handle(1, ClientMsg::Start, 0);
    for t in 1..=4 {
        first.tick(t * 1_000);
    }
    let before = first.game().unwrap().checksum();
    let round = first.game().unwrap().round_index();
    drop(first);

    let mut second = Session::new(content(), config);
    second.resume(Session::load_match(&dir).unwrap(), 10_000).unwrap();
    assert!(second.has_started());
    assert_eq!(second.game().unwrap().checksum(), before);
    assert_eq!(second.game().unwrap().round_index(), round);

    // A stranger cannot take Ann's seat; Ann can, with her token.
    second.handle(2, ClientMsg::Join { name: "Ann".into(), version: PROTOCOL_VERSION, token: None }, 10_000);
    let welcome = |s: &mut Session, conn| {
        s.drain_outbox().into_iter().find_map(|(c, m)| match m {
            ServerMsg::Welcome { seat, .. } if c == conn => Some(seat),
            _ => None,
        })
    };
    assert_eq!(welcome(&mut second, 2), Some(None));
    second.handle(3, ClientMsg::Join { name: "Ann".into(), version: PROTOCOL_VERSION, token: Some(token) }, 10_000);
    assert_eq!(welcome(&mut second, 3), Some(Some(PlayerId(0))));

    // And the game carries on.
    second.tick(12_000);
    second.tick(13_000);
    assert!(second.game().unwrap().round_index() > round);
    std::fs::remove_dir_all(dir).unwrap();
}
