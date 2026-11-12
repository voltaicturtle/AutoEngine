use std::collections::BTreeMap;
use std::sync::Arc;

use autoengine::content::{ContentSet, RoundDef, RoundKindDef, Schedule};
use autoengine::game::{Game, GameEvent, GameHook, PlayerId};
use autoengine::script::ScriptHost;

const P0: PlayerId = PlayerId(0);
const P1: PlayerId = PlayerId(1);

fn sample() -> ContentSet {
    ContentSet::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/content")).expect("sample content loads")
}

fn scripts(list: &[(&str, &str)]) -> BTreeMap<String, String> {
    list.iter().map(|(name, source)| (name.to_string(), source.to_string())).collect()
}

/// The sample units with the given scripts and a schedule of custom rounds.
fn content(list: &[(&str, &str)], rounds: &[(&str, i64)]) -> Arc<ContentSet> {
    let rounds = rounds
        .iter()
        .map(|(kind, n)| RoundDef {
            name: kind.to_string(),
            stage: 1,
            kind: RoundKindDef::Custom { kind: kind.to_string(), params: BTreeMap::from([("n".to_string(), *n)]) },
        })
        .collect();
    let set = sample().with_scripts(scripts(list)).unwrap().with_schedule(Schedule { rounds, repeat_from: 0 }).unwrap();
    Arc::new(set)
}

fn logs(game: &Game) -> Vec<String> {
    game.events()
        .iter()
        .filter_map(|e| match e {
            GameEvent::Script { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_sample_scripts_load_with_the_content() {
    let content = sample();
    assert!(content.scripts().contains_key("payday"));
    let host = ScriptHost::compile(content.scripts()).unwrap();
    assert_eq!(host.round_kinds(), vec!["payday"]);
}

#[test]
fn a_script_is_a_round_type_with_params() {
    let item = sample().items().next().unwrap().id.clone();
    let source = r#"
        fn begin(game) { game.log(`round ${game.round()} of stage ${game.stage()}`); }
        fn finish(game) {
            for p in game.players() {
                game.give_gold(p, game.param("n") + p);
                // The script sees its own change at once.
                if game.gold(p) < game.param("n") { game.log("gold did not update"); }
            }
            game.give_xp(0, 2);
            game.give_item(1, "ITEM");
            game.give_unit(1, "ashen_hound");
            game.damage(0, 7);
        }
    "#
    .replace("ITEM", &item);
    let mut g = Game::new(content(&[("bonus", &source)], &[("bonus", 10)]), 1, &["a", "b"]);
    g.begin_round().unwrap();
    assert!(g.events().iter().any(|e| matches!(e, GameEvent::RoundStarted { kind, .. } if kind == "bonus")));
    assert_eq!(logs(&g), vec!["round 1 of stage 1"]);
    let (gold0, gold1, health0) = (g.player(P0).gold, g.player(P1).gold, g.player(P0).health);
    g.finish_round().unwrap();
    assert_eq!(g.player(P0).gold, gold0 + 10);
    assert_eq!(g.player(P1).gold, gold1 + 11);
    assert_eq!(g.player(P0).health, health0 - 7);
    assert_eq!(g.player(P0).level, 2);
    assert_eq!(g.player(P1).items, vec![item]);
    assert_eq!(g.player(P1).units().filter(|u| u.unit == "ashen_hound").count(), 1);
    assert_eq!(logs(&g).len(), 1, "no errors and no stale reads: {:?}", logs(&g));
}

#[test]
fn a_script_can_listen_to_events_and_remember_things() {
    let rule = r#"
        fn on_event(game, event) {
            if event.kind == "PlayerDamaged" {
                game.set("hits", game.get("hits") + 1);
                game.log(`${game.name(event.player)} took ${event.amount}, hit number ${game.get("hits")}`);
            }
        }
    "#;
    let hurt = r#"fn finish(game) { game.damage(1, game.param("n")); }"#;
    let mut g = Game::new(content(&[("rule", rule), ("hurt", hurt)], &[("hurt", 3)]), 1, &["a", "b"]);
    for _ in 0..2 {
        g.begin_round().unwrap();
        g.finish_round().unwrap();
    }
    assert_eq!(logs(&g), vec!["b took 3, hit number 1", "b took 3, hit number 2"]);
}

#[test]
fn script_mistakes_are_reported_and_do_not_stop_the_game() {
    let source = r#"
        fn finish(game) {
            game.give_gold(0, 5);
            game.give_item(0, "no_such_item");
            game.give_gold(99, 5);
            game.no_such_function();
        }
    "#;
    let endless = "fn finish(game) { loop { } }";
    let mut g = Game::new(content(&[("oops", source), ("endless", endless)], &[("oops", 0), ("endless", 0)]), 1, &["a", "b"]);
    g.begin_round().unwrap();
    let gold = g.player(P0).gold;
    g.finish_round().unwrap();
    // What worked before the mistake still counts.
    assert_eq!(g.player(P0).gold, gold + 5);
    let errors = logs(&g);
    assert_eq!(errors.len(), 3, "{errors:?}");
    assert!(errors.iter().any(|e| e.contains("no_such_item")));
    assert!(errors.iter().any(|e| e.contains("no player 99")));
    assert!(errors.iter().any(|e| e.contains("no_such_function")));
    // A script that never ends is cut off.
    g.begin_round().unwrap();
    g.finish_round().unwrap();
    assert!(logs(&g).len() > 3);
    assert!(!g.is_over());
}

#[test]
fn scripts_that_do_not_compile_are_rejected_with_the_content() {
    let broken = sample().with_scripts(scripts(&[("broken", "fn finish(game) { let = ; }")]));
    assert!(broken.unwrap_err().to_string().contains("scripts/broken.rhai"));
    let idle = sample().with_scripts(scripts(&[("idle", "fn helper() { 1 }")]));
    assert!(idle.unwrap_err().to_string().contains("never run"));
}

#[test]
fn scripted_games_are_reproducible() {
    let source = r#"
        fn finish(game) {
            for p in game.players() { game.give_gold(p, game.random(50)); }
            game.fight_all();
        }
    "#;
    let play = |seed| {
        let mut g = Game::new(content(&[("lottery", source)], &[("lottery", 0)]), seed, &["a", "b", "c"]);
        for _ in 0..3 {
            g.begin_round().unwrap();
            g.finish_round().unwrap();
        }
        (g.players().iter().map(|p| (p.gold, p.health)).collect::<Vec<_>>(), g.drain_events())
    };
    assert_eq!(play(5), play(5));
    assert_ne!(play(5).0, play(6).0);
}

/// Hooks are also available to Rust code.
struct Counter(Arc<std::sync::Mutex<Vec<String>>>);

impl GameHook for Counter {
    fn on_event(&mut self, game: &mut Game, event: &GameEvent) {
        if let GameEvent::RoundStarted { name, .. } = event {
            self.0.lock().unwrap().push(name.clone());
            // A hook may change the game; the events it causes are delivered too.
            game.add_gold(P0, 1);
        }
        if let GameEvent::GoldChanged { player, .. } = event
            && *player == P0
        {
            self.0.lock().unwrap().push("gold".into());
        }
    }
}

#[test]
fn rust_hooks_see_every_event_including_their_own() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut g = Game::new(sample(), 1, &["a", "b"]);
    g.add_hook(Box::new(Counter(seen.clone())));
    let gold = g.player(P0).gold;
    g.begin_round().unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen.first().map(String::as_str), Some("1-1"));
    assert!(seen.iter().any(|s| s == "gold"));
    assert!(g.player(P0).gold > gold);
}
