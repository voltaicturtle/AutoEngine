#![cfg(feature = "scripting")]
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

mod new_tools {
    use autoengine::content::ContentSet;
    use autoengine::tools::scenario::{Expect, ScenarioFile, run_all, run_scenario};
    use autoengine::tools::{bench, lint, scaffold, simulate};

    fn sample_dir() -> &'static str {
        concat!(env!("CARGO_MANIFEST_DIR"), "/content")
    }

    fn scenarios(dir: &std::path::Path) -> ScenarioFile {
        let file = dir.join("tests.ron");
        ScenarioFile::parse("tests.ron", &std::fs::read_to_string(file).unwrap()).unwrap()
    }

    #[test]
    fn the_sample_content_passes_its_own_tests() {
        let content = ContentSet::load_dir(sample_dir()).unwrap();
        let file = scenarios(std::path::Path::new(sample_dir()));
        assert!(!file.tests.is_empty());
        for result in run_all(&content, &file) {
            assert!(result.passed(), "{}: {:?}", result.name, result.failures);
        }
    }

    #[test]
    fn a_broken_expectation_says_what_went_wrong() {
        let content = ContentSet::load_dir(sample_dir()).unwrap();
        let mut scenario = scenarios(std::path::Path::new(sample_dir())).tests.remove(0);
        // The two-star unit wins, so each of these is wrong in its own way.
        scenario.expect = vec![
            Expect::Winner(1),
            Expect::Draw,
            Expect::Dies(0, "ironbark_warden".into()),
            Expect::Survives(1, "ironbark_warden".into()),
            Expect::LastsAtMost { seconds: 0 },
            Expect::WinsAtLeast { team: 1, percent: 50 },
            Expect::Casts(0, "no_such_unit".into()),
            Expect::Casts(1, "moss_golem".into()),
        ];
        let result = run_scenario(&content, &scenario);
        assert!(!result.passed());
        assert_eq!(result.failures.len(), scenario.expect.len(), "{:?}", result.failures);
        assert!(result.failures[0].contains("expected team 1 to win, but team 0 won"), "{:?}", result.failures);
        assert!(result.failures[0].contains("seed"), "failures name the seed so the fight can be watched");
        assert!(result.failures[6].contains("no unit with the id"));
        assert!(result.failures[7].contains("has no moss_golem"));

        // A fight that cannot be set up fails instead of panicking.
        scenario.teams[0].units[0].unit = "no_such_unit".into();
        assert!(run_scenario(&content, &scenario).failures[0].contains("could not be set up"));
    }

    #[test]
    fn the_starter_game_is_a_working_game() {
        let dir = std::env::temp_dir().join(format!("autoengine-starter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let written = scaffold::write_starter(&dir).unwrap();
        assert_eq!(written.len(), scaffold::STARTER.len());
        // It never overwrites anything.
        assert!(scaffold::write_starter(&dir).unwrap_err().contains("not empty"));

        let content = ContentSet::load_dir(&dir).expect("the starter loads");
        let warnings = lint(&content, Some(&dir));
        assert!(warnings.is_empty(), "{warnings:?}");
        for result in run_all(&content, &scenarios(&dir)) {
            assert!(result.passed(), "{}: {:?}", result.name, result.failures);
        }
        // Bots can play it to the end, scripted round included.
        let report = simulate(content, 2, 4, 5);
        assert_eq!(report.unfinished, 0);
        assert!(report.fights > 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_benchmarks_run() {
        let content = ContentSet::load_dir(sample_dir()).unwrap();
        let text = std::fs::read_to_string(format!("{}/sample_match.ron", sample_dir())).unwrap();
        let setup = autoengine::content::parse_ron("sample_match.ron", &text).unwrap();
        let fights = bench::fights(&content, &setup, 3).unwrap();
        assert_eq!(fights.count, 3);
        assert!(fights.steps > 0 && fights.per_second() > 0.0);
        let games = bench::games(content, 1, 4);
        assert!(games.steps > 0);
    }
}
