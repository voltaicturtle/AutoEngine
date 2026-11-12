use autoengine::combat::EventKind;
use autoengine::content::{
    BoardUnit, Bonus, ContentError, ContentSet, MatchSetup, Scope, TeamSetup, parse_ron,
};
use autoengine::effect::Trigger;
use autoengine::stats::{Stat, StatMod};

fn sample() -> ContentSet {
    ContentSet::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/content")).expect("sample content loads")
}

fn sample_match() -> MatchSetup {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/content/sample_match.ron");
    parse_ron(path, &std::fs::read_to_string(path).unwrap()).expect("sample match parses")
}

fn unit(id: &str, pos: (i32, i32)) -> BoardUnit {
    BoardUnit { unit: id.into(), star: 1, items: vec![], pos }
}

const TRAITS: &str = r#"[
    (id: "a", name: "A", breakpoints: [
        (count: 2, bonuses: [(mods: [(stat: Armor, flat: 10)])]),
        (count: 3, bonuses: [(mods: [(stat: Armor, flat: 30)]), (scope: Team, mods: [(stat: MagicResist, flat: 5)])]),
    ]),
]"#;

fn expect_invalid(result: Result<ContentSet, ContentError>, needle: &str) {
    match result {
        Err(ContentError::Invalid(m)) => assert!(m.contains(needle), "`{m}` should mention `{needle}`"),
        other => panic!("expected invalid content mentioning `{needle}`, got {other:?}"),
    }
}

#[test]
fn sample_content_loads_and_is_indexed() {
    let c = sample();
    assert_eq!(c.units().count(), 9);
    assert_eq!(c.traits().count(), 5);
    assert_eq!(c.items().count(), 6);
    assert_eq!(c.board().len(), 56);
    let warden = c.unit("ironbark_warden").unwrap();
    assert_eq!(warden.cost, 2);
    assert!(warden.in_shop);
    assert!(!c.unit("grave_wisp").unwrap().in_shop);
    assert_eq!(c.combine_items("ember_core", "whetstone").unwrap().id, "cinder_blade");
    assert_eq!(c.combine_items("oak_charm", "oak_charm").unwrap().id, "barkplate");
    assert!(c.combine_items("ember_core", "ember_core").is_none());
}

#[test]
fn traits_count_distinct_units_and_item_grants() {
    let c = sample();
    let mut board = vec![
        unit("dawnpetal_mender", (0, 4)),
        unit("dawnpetal_mender", (1, 4)), // a second copy does not count again
        unit("tidewater_stormcaller", (2, 4)),
    ];
    let traits = c.active_traits(&board).unwrap();
    let grove = traits.iter().find(|t| t.id == "grove").unwrap();
    assert_eq!((grove.count, grove.tier, grove.next_at), (1, None, Some(2)));

    board[2].items.push("grove_sigil".into());
    let traits = c.active_traits(&board).unwrap();
    let grove = traits.iter().find(|t| t.id == "grove").unwrap();
    assert_eq!((grove.count, grove.tier, grove.next_at), (2, Some(0), Some(3)));

    board.push(unit("reed_stalker", (3, 4)));
    let traits = c.active_traits(&board).unwrap();
    let grove = traits.iter().find(|t| t.id == "grove").unwrap();
    assert_eq!((grove.count, grove.tier, grove.next_at), (3, Some(1), None));
    let tide = traits.iter().find(|t| t.id == "tide").unwrap();
    assert_eq!((tide.count, tide.tier), (2, Some(0)));
}

#[test]
fn build_team_folds_in_stars_items_traits_and_team_bonuses() {
    let c = sample();
    let team = TeamSetup {
        units: vec![
            BoardUnit { unit: "ironbark_warden".into(), star: 2, items: vec!["barkplate".into()], pos: (3, 4) },
            unit("dawnpetal_mender", (4, 6)),
            unit("tidewater_stormcaller", (1, 7)),
        ],
        bonuses: vec![Bonus {
            scope: Scope::WithTrait("tide".into()),
            mods: vec![StatMod::flat(Stat::AbilityPower, 25)],
            passives: vec![],
        }],
    };
    let built = c.build_team(&team).unwrap();
    let (warden, mender, caller) = (&built[0].template, &built[1].template, &built[2].template);

    // 900 health at two stars (+80%) is 1620, plus 350 from the item.
    assert_eq!(warden.stats.get(Stat::MaxHealth), 1970);
    // 40 armor + 20 (item) + 15 (Grove at 2 members).
    assert_eq!(warden.stats.get(Stat::Armor), 75);
    assert_eq!(warden.stats.get(Stat::AttackDamage), 67);
    assert_eq!(warden.stats.get(Stat::AbilityPower), 150);
    let names: Vec<&str> = warden.passives.iter().map(|p| p.name.as_str()).collect();
    // Own passive, item passive, then both Bulwark shields (team-wide and member).
    assert_eq!(names, ["Bramble Hide", "Barkplate Splinters", "Bulwark's Cover", "Bulwark's Stand"]);

    assert_eq!(mender.stats.get(Stat::Armor), 30);
    let names: Vec<&str> = mender.passives.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Bulwark's Cover"], "non-members only get the team-wide part");

    // Not a Grove unit, so no armor bonus; the team bonus targets Tide.
    assert_eq!(caller.stats.get(Stat::Armor), 15);
    assert_eq!(caller.stats.get(Stat::AbilityPower), 125);
    assert_eq!(mender.stats.get(Stat::AbilityPower), 100);
    assert_eq!(caller.tags, ["tide"]);
    assert_eq!(built[0].pos.to_offset(), (3, 4));
}

#[test]
fn only_the_highest_breakpoint_applies() {
    let units = r#"[
        (id: "x", name: "X", traits: ["a"]),
        (id: "y", name: "Y", traits: ["a"]),
        (id: "z", name: "Z", traits: ["a"]),
        (id: "w", name: "W"),
    ]"#;
    let c = ContentSet::from_ron(units, TRAITS, None, None).unwrap();
    let armor = |ids: &[&str]| -> Vec<(i64, i64)> {
        let units = ids.iter().enumerate().map(|(i, id)| unit(id, (i as i32, 4))).collect();
        c.build_team(&TeamSetup { units, bonuses: vec![] })
            .unwrap()
            .iter()
            .map(|p| (p.template.stats.get(Stat::Armor), p.template.stats.get(Stat::MagicResist)))
            .collect()
    };
    assert_eq!(armor(&["x", "w"]), [(0, 0), (0, 0)]);
    assert_eq!(armor(&["x", "y", "w"]), [(10, 0), (10, 0), (0, 0)]);
    assert_eq!(armor(&["x", "y", "z", "w"]), [(30, 5), (30, 5), (30, 5), (0, 5)]);
}

#[test]
fn invalid_content_is_rejected_with_a_clear_message() {
    let ok_units = r#"[(id: "x", name: "X", traits: ["a"])]"#;
    assert!(ContentSet::from_ron(ok_units, TRAITS, None, None).is_ok());

    expect_invalid(
        ContentSet::from_ron(r#"[(id: "x", name: "X", traits: ["nope"])]"#, TRAITS, None, None),
        "unknown trait `nope`",
    );
    expect_invalid(
        ContentSet::from_ron(r#"[(id: "x", name: "X"), (id: "x", name: "Again")]"#, TRAITS, None, None),
        "duplicate unit id `x`",
    );
    expect_invalid(
        ContentSet::from_ron(
            r#"[(id: "x", name: "X", passives: [(trigger: OnDeath, effects: [
                (target: Source, effect: Summon(unit: "ghost", count: 1)),
            ])])]"#,
            TRAITS,
            None,
            None,
        ),
        "summons unknown unit `ghost`",
    );
    expect_invalid(
        ContentSet::from_ron(
            ok_units,
            r#"[(id: "a", name: "A", breakpoints: [(count: 3, bonuses: []), (count: 2, bonuses: [])])]"#,
            None,
            None,
        ),
        "increasing",
    );
    expect_invalid(
        ContentSet::from_ron(
            ok_units,
            TRAITS,
            Some(r#"[(id: "i", name: "I", recipe: ("i", "missing"))]"#),
            None,
        ),
        "unknown item `missing`",
    );
    expect_invalid(
        ContentSet::from_ron(ok_units, TRAITS, None, Some("(star_levels: [])")),
        "star level",
    );

    match ContentSet::from_ron("[(id: 5)]", TRAITS, None, None) {
        Err(ContentError::Parse { file, .. }) => assert_eq!(file, "units.ron"),
        other => panic!("expected a parse error, got {other:?}"),
    }
}

#[test]
fn bad_boards_are_rejected() {
    let c = sample();
    let build = |u: BoardUnit| c.build_team(&TeamSetup { units: vec![u], bonuses: vec![] }).map(|_| ());
    let err = |r: Result<(), ContentError>| r.unwrap_err().to_string();

    assert!(err(build(unit("nobody", (0, 4)))).contains("unknown unit `nobody`"));
    assert!(err(build(BoardUnit { star: 4, ..unit("ashen_hound", (0, 4)) })).contains("star level 4"));
    assert!(err(build(BoardUnit { star: 0, ..unit("ashen_hound", (0, 4)) })).contains("star level 0"));
    assert!(err(build(BoardUnit { items: vec!["nothing".into()], ..unit("ashen_hound", (0, 4)) }))
        .contains("unknown item `nothing`"));
    assert!(err(build(BoardUnit { items: vec!["whetstone".into(); 4], ..unit("ashen_hound", (0, 4)) }))
        .contains("limit is 3"));

    // Two units on the same hex, and a unit off the board.
    let clash = TeamSetup { units: vec![unit("ashen_hound", (0, 4)), unit("reed_stalker", (0, 4))], bonuses: vec![] };
    assert!(c.setup_combat(1, &[clash]).is_err());
    let off = TeamSetup { units: vec![unit("ashen_hound", (9, 9))], bonuses: vec![] };
    assert!(c.setup_combat(1, &[off]).is_err());
}

#[test]
fn sample_match_runs_deterministically_from_data() {
    let c = sample();
    let setup = sample_match();
    let run = |seed| {
        let mut combat = c.setup_combat(seed, &setup.teams).unwrap();
        combat.run();
        (combat.outcome().unwrap().clone(), combat.events().to_vec())
    };
    let (outcome, events) = run(11);
    assert_eq!((outcome.clone(), events.clone()), run(11));
    assert!(outcome.winner.is_some());

    // The second team is mirrored onto the far side of the board.
    let first_spawns: Vec<(i32, i32)> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Spawned { pos, .. } => Some(pos.to_offset()),
            _ => None,
        })
        .take(10)
        .collect();
    assert_eq!(first_spawns[0], (3, 4));
    assert_eq!(first_spawns[5], (3, 3));
}

#[test]
fn summons_and_trait_passives_work_from_data() {
    let c = sample();
    let keeper = TeamSetup {
        units: vec![unit("hollow_lantern_keeper", (3, 4)), unit("ashen_hound", (2, 4))],
        bonuses: vec![],
    };
    let enemy = TeamSetup {
        units: vec![
            BoardUnit { star: 3, ..unit("ironbark_warden", (3, 4)) },
            BoardUnit { star: 3, ..unit("emberlash_duelist", (2, 4)) },
        ],
        bonuses: vec![],
    };
    // Hollow is active at 2, so both units carry the death passive.
    let built = c.build_team(&keeper).unwrap();
    assert!(built.iter().all(|p| p.template.passives.iter().any(|x| x.name == "Hollow Vengeance" && x.trigger == Trigger::OnDeath)));

    let mut combat = c.setup_combat(3, &[keeper, enemy]).unwrap();
    combat.run();
    let wisps = combat.units().iter().filter(|u| u.name() == "Grave Wisp").count();
    assert_eq!(wisps, 2, "the keeper's death summons two wisps from the content set");
    assert!(combat.units().iter().filter(|u| u.name() == "Grave Wisp").all(|u| u.team().0 == 0));
}

#[test]
fn sample_units_name_model_files_that_exist() {
    let content = sample();
    let mut with_model = 0;
    for unit in content.units() {
        if let Some(model) = &unit.model {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("content").join(&model.file);
            assert!(path.is_file(), "{} names a missing model: {}", unit.id, model.file);
            assert!(model.scale_percent > 0);
            with_model += 1;
        }
    }
    assert!(with_model > 0);
}
