use autoengine::hub::{Hub, HubConfig};
use autoengine::ladder::{Ladder, STARTING_RATING};
use autoengine::net::Host;
use autoengine::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg};
use autoengine::session::SessionConfig;

fn game(places: &[(&str, u32)]) -> Vec<(String, u32)> {
    places.iter().map(|(n, p)| (n.to_string(), *p)).collect()
}

#[test]
fn placements_move_ratings() {
    let mut ladder = Ladder::new();
    ladder.record(&game(&[("ann", 1), ("bo", 2), ("cy", 3), ("di", 4)]));
    let r = |l: &Ladder, n: &str| l.rating(n).rating;
    assert!(r(&ladder, "ann") > r(&ladder, "bo"));
    assert!(r(&ladder, "bo") > STARTING_RATING && r(&ladder, "cy") < STARTING_RATING);
    assert!(r(&ladder, "di") < r(&ladder, "cy"));
    // Close to zero-sum.
    let total: i32 = ["ann", "bo", "cy", "di"].iter().map(|n| r(&ladder, n) - STARTING_RATING).sum();
    assert!(total.abs() <= 2, "{total}");
    // A whole game moves a rating about as much as one match: at most 32.
    assert!(r(&ladder, "ann") - STARTING_RATING <= 32);
    let ann = ladder.rating("ann");
    assert_eq!((ann.games, ann.wins, ann.top_half), (1, 1, 1));
    assert_eq!(ladder.rating("cy").top_half, 0);

    // Beating a stronger player is worth more than beating a weaker one.
    for _ in 0..5 {
        ladder.record(&game(&[("ann", 1), ("cy", 2), ("di", 3)]));
    }
    let before = r(&ladder, "di");
    let mut upset = ladder.clone();
    upset.record(&game(&[("di", 1), ("ann", 2)]));
    let mut expected = ladder.clone();
    expected.record(&game(&[("di", 1), ("cy", 2)]));
    assert!(r(&upset, "di") - before > r(&expected, "di") - before);

    let rows = ladder.standings();
    assert_eq!(rows[0].name, "ann");
    assert_eq!(ladder.rating("ann").games, 6);
    assert_eq!(rows.last().unwrap().name, "di");
    ladder.record(&game(&[("solo", 1)]));
    assert_eq!(ladder.rating("solo").games, 0, "one player is not a game");

    let path = std::env::temp_dir().join(format!("autoengine-ladder-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    assert_eq!(Ladder::load(&path).unwrap(), Ladder::new(), "a missing file is an empty ladder");
    ladder.save(&path).unwrap();
    assert_eq!(Ladder::load(&path).unwrap(), ladder);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_hub_rates_the_people_in_finished_games() {
    let room = SessionConfig { seats: 3, seed: 2, planning_ms: 1_000, wait_for_playback: false, after_combat_ms: 0 };
    let mut hub = Hub::new(autoengine::sample::content(), HubConfig { room, ..HubConfig::default() });
    hub.keep_ladder(Ladder::new(), None);
    hub.handle(1, ClientMsg::Join { name: "Ann".into(), version: PROTOCOL_VERSION, token: None }, 0);
    hub.handle(2, ClientMsg::Join { name: "Bo".into(), version: PROTOCOL_VERSION, token: None }, 0);
    hub.handle(1, ClientMsg::Start, 0);
    let mut now = 0;
    while !hub.room(1).unwrap().is_over() {
        now += 1_000;
        hub.tick(now);
        hub.drain_outbox();
        assert!(now < 2_000_000, "the game should end");
    }
    hub.tick(now + 1);
    let ladder = hub.ladder().unwrap();
    let rated: Vec<String> = ladder.standings().into_iter().map(|s| s.name).collect();
    assert_eq!(rated.len(), 2, "the bot is not rated: {rated:?}");
    assert!(rated.contains(&"Ann".to_string()) && rated.contains(&"Bo".to_string()));
    // Only once per game.
    hub.tick(now + 2);
    assert_eq!(hub.ladder().unwrap().rating("Ann").games, 1);

    hub.handle(3, ClientMsg::Standings, now);
    let out = hub.drain_outbox();
    let rows = out.iter().find_map(|(c, m)| match m {
        ServerMsg::Standings(rows) if *c == 3 => Some(rows.clone()),
        _ => None,
    });
    assert_eq!(rows.unwrap().len(), 2);
}
