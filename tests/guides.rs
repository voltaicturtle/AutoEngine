//! Every Rhai example in `docs/guides` compiles and runs in a real game
//! without errors. (The Rust examples are doctests: see `src/lib.rs`.)
#![cfg(feature = "scripting")]

use autoengine::content::{RoundDef, RoundKindDef, Schedule};
use autoengine::prelude::*;

fn rhai_examples() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/guides");
    let mut found = Vec::new();
    let mut paths: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    paths.sort();
    for path in paths.into_iter().filter(|p| p.extension().is_some_and(|e| e == "md")) {
        let text = std::fs::read_to_string(&path).unwrap();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let mut rest = text.as_str();
        while let Some(start) = rest.find("```rhai\n") {
            let body = &rest[start + 8..];
            let end = body.find("```").expect("closed code block");
            found.push((format!("{name}_{}", found.len()), body[..end].to_string()));
            rest = &body[end..];
        }
    }
    found
}

#[test]
fn rhai_examples_compile_and_run() {
    let examples = rhai_examples();
    assert!(examples.len() >= 7, "found only {} examples", examples.len());
    for (name, source) in examples {
        let is_round = source.contains("fn finish(");
        let mut rounds = vec![RoundDef::new("fight", 1, RoundKindDef::pvp())];
        if is_round {
            rounds.push(RoundDef::new("Bounty", 1, RoundKindDef::custom(name.as_str()).param("gold", 2)));
        }
        let mut scripts = autoengine::sample::content().scripts().clone();
        scripts.insert(name.clone(), source);
        let content = autoengine::sample::content()
            .with_schedule(Schedule::new(rounds))
            .and_then(|c| c.with_scripts(scripts))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut game = Game::new(content, 5, &["Ana", "Ben", "Cho"]);
        let mut rng = Rng::new(5);
        for _ in 0..8 {
            if game.is_over() {
                break;
            }
            bot::play_round(&mut game, &mut rng).unwrap();
        }
        for event in game.events() {
            if let GameEvent::Script { script, message } = event {
                assert!(!message.starts_with("error"), "{script}: {message}");
            }
        }
    }
}
