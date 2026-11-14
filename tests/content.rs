use autoengine::combat::EventKind;
use autoengine::content::{
    BoardUnit, Bonus, ContentError, ContentSet, GameBuilder, ItemDef, MatchSetup, Scope, TeamSetup, TraitDef, UnitDef,
};
use autoengine::effect::{Ability, Effect, Passive, Selector, Trigger};
use autoengine::stats::{Stat, StatMod};

fn sample() -> ContentSet {
    autoengine::sample::content()
}

fn sample_match() -> MatchSetup {
    autoengine::sample::sample_match()
}

fn unit(id: &str, pos: (i32, i32)) -> BoardUnit {
    BoardUnit { unit: id.into(), star: 1, items: vec![], pos }
}

/// A game with one trait, `a`, and nothing else.
fn with_trait_a() -> GameBuilder {
    ContentSet::builder().trait_def(
        TraitDef::new("a", "A")
            .breakpoint(2, [Bonus::members().mods([Stat::Armor.flat(10)])])
            .breakpoint(3, [Bonus::members().mods([Stat::Armor.flat(30)]), Bonus::team().mods([Stat::MagicResist.flat(5)])]),
    )
}

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
            ..Bonus::default()
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
fn sample_match_runs_deterministically() {
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
fn summons_and_trait_passives_work() {
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
            let path = std::path::Path::new(autoengine::sample::ASSETS).join(&model.file);
            assert!(path.is_file(), "{} names a missing model: {}", unit.id, model.file);
            assert!(model.scale_percent > 0);
            with_model += 1;
        }
    }
    assert!(with_model > 0);
}

#[test]
fn a_mistyped_id_gets_a_suggestion() {
    let err = sample().setup_combat(1, &[TeamSetup { units: vec![unit("ironbark_wardn", (3, 4))], bonuses: vec![] }]);
    let message = err.err().expect("the unit does not exist").to_string();
    assert!(message.contains("unknown unit `ironbark_wardn` (did you mean `ironbark_warden`?)"), "{message}");
    // Nothing is suggested when nothing is close.
    let err = sample().setup_combat(1, &[TeamSetup { units: vec![unit("zzz", (3, 4))], bonuses: vec![] }]);
    assert!(!err.err().unwrap().to_string().contains("did you mean"));
}

#[test]
fn build_team_records_unit_ids() {
    let c = sample();
    let team = TeamSetup::new()
        .unit(unit("ironbark_warden", (3, 4)).star(2).item("barkplate"))
        .unit(unit("tidewater_stormcaller", (1, 7)))
        .bonus(Bonus::with_trait("tide").mods([Stat::AbilityPower.flat(25)]));
    let built = c.build_team(&team).unwrap();
    assert_eq!(built[1].template.stats.get(Stat::AbilityPower), 125);
    assert_eq!((built[0].template.id.as_str(), built[1].template.id.as_str()), ("ironbark_warden", "tidewater_stormcaller"));
}

#[test]
fn only_the_highest_breakpoint_applies() {
    let c = with_trait_a()
        .units(["x", "y", "z"].map(|id| UnitDef::new(id, id.to_uppercase()).traits(["a"])))
        .unit(UnitDef::new("w", "W"))
        .build()
        .unwrap();
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
fn invalid_built_content_is_rejected_with_a_clear_message() {
    let x = || UnitDef::new("x", "X").traits(["a"]);
    assert!(with_trait_a().unit(x()).build().is_ok());

    expect_invalid(with_trait_a().unit(UnitDef::new("x", "X").traits(["nope"])).build(), "unknown trait `nope`");
    expect_invalid(with_trait_a().unit(x()).unit(UnitDef::new("x", "Again")).build(), "duplicate unit id `x`");
    expect_invalid(with_trait_a().unit(UnitDef::new("", "Nameless")).build(), "empty id");
    let summoner = x().passive(Passive::new("", Trigger::OnDeath).effect(Effect::summon("ghost", 1).to(Selector::Source)));
    expect_invalid(with_trait_a().unit(summoner).build(), "summons unknown unit `ghost`");
    expect_invalid(
        ContentSet::builder().unit(x()).trait_def(TraitDef::new("a", "A").breakpoint(3, []).breakpoint(2, [])).build(),
        "increasing",
    );
    expect_invalid(ContentSet::builder().trait_def(TraitDef::new("a", "A")).build(), "no breakpoints");
    expect_invalid(with_trait_a().unit(x()).item(ItemDef::new("i", "I").recipe("i", "missing")).build(), "unknown item `missing`");
    expect_invalid(
        with_trait_a().item(ItemDef::new("i", "I").grants_trait("nope")).build(),
        "item `i` refers to unknown trait `nope`",
    );
    let empty_status = Ability::new("Hex").effect(Effect::stat_status([], 1000).to(Selector::Source));
    expect_invalid(with_trait_a().unit(x().ability(empty_status)).build(), "no modifiers");
    expect_invalid(with_trait_a().unit(x()).star_levels(Vec::<Vec<_>>::new()).build(), "star level");
    expect_invalid(with_trait_a().board(0, 8).build(), "at least one column");
    let custom = Ability::new("Odd").effect(Effect::custom("unregistered").to(Selector::Source));
    expect_invalid(with_trait_a().unit(x().ability(custom)).build(), "unregistered custom effect `unregistered`");
}

#[test]
fn definitions_can_be_added_in_any_order() {
    // Units and items name traits and units that are only added afterwards.
    let call = Passive::new("Call", Trigger::CombatStart).effect(Effect::summon("pet", 1).to(Selector::Source));
    let c = ContentSet::builder()
        .item(ItemDef::new("sigil", "Sigil").grants_trait("a"))
        .unit(UnitDef::new("caller", "Caller").passive(call))
        .unit(UnitDef::new("pet", "Pet").summon_only())
        .trait_def(TraitDef::new("a", "A").breakpoint(1, [Bonus::members().mods([Stat::Armor.flat(5)])]))
        .build()
        .unwrap();
    assert!(!c.unit("pet").unwrap().in_shop);
    let built = c.build_team(&TeamSetup::new().unit(unit("caller", (0, 4)).item("sigil"))).unwrap();
    assert_eq!(built[0].template.stats.get(Stat::Armor), 5);
    assert_eq!(built[0].template.tags, ["a"]);
}

#[test]
fn custom_hooks_run_in_fights_set_up_from_content() {
    let c = ContentSet::builder()
        .custom_effect("execute", |ctx| {
            let target = ctx.target();
            let health = ctx.combat().unit(target).health();
            ctx.apply(target, &Effect::true_damage(health));
        })
        .unit(
            UnitDef::new("reaper", "Reaper")
                .stats([(Stat::MaxMana, 10), (Stat::StartingMana, 10), (Stat::Range, 7)])
                .ability(Ability::new("Execute").effect(Effect::custom("execute").to(Selector::CurrentTarget))),
        )
        .unit(UnitDef::new("dummy", "Dummy").stat(Stat::MaxHealth, 99_999))
        .build()
        .unwrap();
    let mut combat = c
        .setup_combat(1, &[TeamSetup::new().unit(unit("reaper", (3, 4))), TeamSetup::new().unit(unit("dummy", (3, 4)))])
        .unwrap();
    let outcome = combat.run();
    assert_eq!(outcome.winner.map(|t| t.0), Some(0), "the hook killed the dummy outright");
    assert!(outcome.duration_ms < 5_000);
}

/// The example in docs/guide.md ("Conditions, stacks and movement") builds
/// and fights.
#[test]
fn the_guide_example_for_conditions_and_stacks_works() {
    use autoengine::effect::{Condition, Who};
    let brawler = UnitDef::new("frenzied_brawler", "Frenzied Brawler")
        .cost(2)
        .passive(
            Passive::new("Frenzy", Trigger::OnAttack).effect(
                Effect::add_stacks("frenzy", 1).capped_at(10).per_stack([Stat::AttackSpeed.percent(50)]).to(Selector::Source),
            ),
        )
        .passive(
            Passive::new("Finisher", Trigger::OnAttack)
                .when(Condition::StacksAtLeast { who: Who::Source, stack: "frenzy".into(), count: 10 })
                .effect(Effect::knockback(1).to(Selector::EventOther))
                .effect(Effect::stun(500).to(Selector::EventOther).when(Condition::Stunned(Who::Target).not()))
                .effect(Effect::clear_stacks("frenzy").to(Selector::Source)),
        );
    let c = ContentSet::builder().unit(brawler).build().expect("the guide example is valid");
    let team = || TeamSetup::new().unit(unit("frenzied_brawler", (3, 4)));
    let mut combat = c.setup_combat(3, &[team(), team()]).unwrap();
    combat.run();
    let stacked = combat
        .events()
        .iter()
        .any(|e| matches!(&e.kind, EventKind::StacksChanged { stack, .. } if stack == "frenzy"));
    assert!(stacked);
}
