use autoengine::content::{ContentSet, RoundDef, RoundKindDef, Schedule};
use autoengine::tools::{lint, simulate};

fn sample() -> ContentSet {
    ContentSet::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/content")).expect("sample content loads")
}

#[test]
fn the_sample_content_has_no_warnings() {
    let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/content"));
    let warnings = lint(&sample(), Some(dir));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn the_checker_points_out_likely_mistakes() {
    let custom = RoundDef {
        name: "odd".into(),
        stage: 1,
        kind: RoundKindDef::Custom { kind: "nobody_wrote_this".into(), params: Default::default() },
    };
    let content = sample().with_schedule(Schedule { rounds: vec![custom], repeat_from: 0 }).unwrap();
    // An assets folder with no models in it.
    let warnings = lint(&content, Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))));
    let text: Vec<String> = warnings.iter().map(|w| w.to_string()).collect();
    assert!(text.iter().any(|w| w.starts_with("round odd") && w.contains("nobody_wrote_this")), "{text:?}");
    assert!(text.iter().any(|w| w.contains("is missing")), "{text:?}");
    // Without an assets folder, models are not checked.
    assert!(lint(&content, None).iter().all(|w| !w.message.contains("model")));
}

#[test]
fn the_simulator_is_repeatable_and_its_numbers_add_up() {
    let content = std::sync::Arc::new(sample());
    let report = simulate(content.clone(), 3, 4, 11);
    assert_eq!(report, simulate(content.clone(), 3, 4, 11));
    assert_ne!(report, simulate(content.clone(), 3, 4, 12));
    assert_eq!(report.games, 3);
    assert_eq!(report.unfinished, 0);
    assert!(report.fights > 0 && report.rounds > report.games);
    for (unit, record) in &report.units {
        assert!(content.unit(unit).is_some());
        assert!(record.wins <= record.fights);
        assert!(record.win_percent() <= 100);
    }
    // Every decided fight has one winning side.
    let most = report.units.values().map(|r| r.fights).max().unwrap();
    assert!(most <= report.fights * 2);
    let text = report.to_text(&content);
    assert!(text.contains("3 games (3 finished)") && text.contains("win %"));
}
