//! The game: its units, traits, items, augments, rounds and content tests,
//! all in Rust. `cargo run -- check` looks for mistakes after a change and
//! `cargo test` runs the content tests.
//!
//! Stats you leave out use the engine's defaults. Speeds and percentages are
//! whole numbers: AttackSpeed 700 means 0.7 attacks per second, and
//! `.percent(250)` is +25%.

use autoengine::content::{
    AugmentDef, BoardUnit, Bonus, ChoiceOffer, ContentSet, ItemDef, MatchSetup, RoundDef, RoundKindDef, Schedule,
    TeamSetup, TraitDef, UnitDef,
};
use autoengine::effect::{Ability, Amount, Effect, Selector};
use autoengine::stats::Stat::*;
use autoengine::tools::cli::GameSpec;
use autoengine::tools::scenario::{Expect, Scenario};

/// Everything the command line needs to know about the game.
pub fn spec() -> GameSpec {
    GameSpec::new("mygame", content).sample_match(sample_match).tests(tests).assets("assets")
}

/// The whole game. `build()` checks every reference and reports mistakes.
pub fn content() -> ContentSet {
    ContentSet::builder()
        .units(units())
        .trait_defs(traits())
        .items(items())
        .augments(augments())
        .schedule(schedule())
        // A round type written as a script; see scripts/bonus_gold.rhai.
        .script("bonus_gold", include_str!("../scripts/bonus_gold.rhai"))
        .build()
        .expect("the content is valid")
}

/// Units. Each one players can buy, or one that only appears in neutral
/// fights (`summon_only`).
pub fn units() -> Vec<UnitDef> {
    vec![
        // A front-line unit: lots of health, short range. Its ability is cast
        // when mana is full: a list of effects, each with a target.
        UnitDef::new("shield_bearer", "Shield Bearer")
            .cost(1)
            .traits(["guard"])
            .stats([(MaxHealth, 800), (AttackDamage, 40), (Armor, 35), (MagicResist, 20), (AttackSpeed, 600), (MaxMana, 80)])
            .ability(Ability::new("Brace").cast_time(300).effect(Effect::shield_for(250, 4000).to(Selector::Source))),
        // A ranged damage dealer.
        UnitDef::new("longbow", "Longbow")
            .cost(1)
            .traits(["ranger"])
            .stats([(MaxHealth, 480), (AttackDamage, 55), (Range, 4), (AttackSpeed, 800), (MaxMana, 40)])
            .ability(
                Ability::new("Aimed Shot")
                    .cast_time(400)
                    .effect(Effect::physical(Amount::power(220)).to(Selector::CurrentTarget)),
            ),
        // A healer that belongs to both traits.
        UnitDef::new("field_medic", "Field Medic")
            .cost(2)
            .traits(["guard", "ranger"])
            .stats([(MaxHealth, 550), (AttackDamage, 35), (Range, 3), (MaxMana, 70)])
            .ability(
                Ability::new("Patch Up")
                    .cast_time(400)
                    .effect(Effect::heal(Amount::power(200)).to(Selector::LowestHealthAllies(1))),
            ),
        // Not sold in the shop: only used by the neutral round.
        UnitDef::new("training_dummy", "Training Dummy")
            .summon_only()
            .stats([(MaxHealth, 500), (AttackDamage, 25), (AttackSpeed, 500)]),
    ]
}

/// Traits. A breakpoint is active when at least `count` different units with
/// the trait are on the board. Only the highest one reached applies.
pub fn traits() -> Vec<TraitDef> {
    vec![
        TraitDef::new("guard", "Guard")
            .description("Guard units take less damage.")
            .breakpoint(2, [Bonus::members().mods([Armor.flat(20), MagicResist.flat(20)])]),
        TraitDef::new("ranger", "Ranger")
            .description("Ranger units attack faster.")
            .breakpoint(2, [Bonus::members().mods([AttackSpeed.percent(250)])]),
    ]
}

/// Items. An item with a recipe is made by putting its two components on the
/// same unit.
pub fn items() -> Vec<ItemDef> {
    vec![
        ItemDef::new("iron_ingot", "Iron Ingot").mods([Armor.flat(20)]),
        ItemDef::new("sharp_flint", "Sharp Flint").mods([AttackDamage.flat(15)]),
        ItemDef::new("spiked_plate", "Spiked Plate")
            .recipe("iron_ingot", "sharp_flint")
            .mods([Armor.flat(40), AttackDamage.flat(25)]),
    ]
}

/// Augments: lasting bonuses a player picks in a choice round.
pub fn augments() -> Vec<AugmentDef> {
    vec![
        AugmentDef::new("deep_pockets", "Deep Pockets").description("Gain 10 gold now.").gold(10),
        AugmentDef::new("drilled", "Drilled")
            .description("Your team gains 15 armor.")
            .bonus(Bonus::team().mods([Armor.flat(15)])),
        AugmentDef::new("keen_eyes", "Keen Eyes")
            .description("Your team gains 10 attack damage.")
            .bonus(Bonus::team().mods([AttackDamage.flat(10)])),
    ]
}

/// The order of rounds. When the list ends it loops from `repeat_from`, one
/// stage higher each time.
pub fn schedule() -> Schedule {
    Schedule::new([
        RoundDef::new("1-1", 1, RoundKindDef::pve([BoardUnit::new("training_dummy", (3, 4))]).items(1)),
        RoundDef::new("1-2", 1, RoundKindDef::choice(2, ChoiceOffer::Augments)),
        RoundDef::new("1-3", 1, RoundKindDef::pvp()),
        RoundDef::new("1-4", 1, RoundKindDef::pvp()),
        RoundDef::new("1-5", 1, RoundKindDef::custom("bonus_gold").param("gold", 3)),
        RoundDef::new("1-6", 1, RoundKindDef::pvp()),
    ])
    .repeat_from(2)
}

/// A fight for `cargo run -- fight`, which prints everything that happens.
/// Positions are (column, row) from each team's own side: row 4 is the front
/// line and row 7 the back.
pub fn sample_match() -> MatchSetup {
    MatchSetup {
        teams: vec![
            TeamSetup::new().unit(BoardUnit::new("shield_bearer", (3, 4))).unit(BoardUnit::new("longbow", (2, 7))),
            TeamSetup::new().unit(BoardUnit::new("shield_bearer", (3, 4))).unit(BoardUnit::new("field_medic", (4, 6))),
        ],
    }
}

/// Fights with an expected result, run by `cargo test` and `cargo run --
/// test`. Use them to pin down what your content is supposed to do, so a
/// later change that breaks it is caught. Each is fought `runs` times with
/// different dice.
pub fn tests() -> Vec<Scenario> {
    vec![
        Scenario::new(
            "two guards beat one",
            [
                TeamSetup::new().unit(BoardUnit::new("shield_bearer", (3, 4))).unit(BoardUnit::new("field_medic", (3, 6))),
                TeamSetup::new().unit(BoardUnit::new("shield_bearer", (3, 4))),
            ],
        )
        .runs(5)
        .expect([Expect::Winner(0), Expect::Survives(0, "shield_bearer".into()), Expect::LastsAtMost { seconds: 40 }]),
        Scenario::new(
            "a longbow gets its shot off",
            [
                TeamSetup::new().unit(BoardUnit::new("longbow", (3, 7))),
                TeamSetup::new().unit(BoardUnit::new("training_dummy", (3, 4))),
            ],
        )
        .expect([Expect::Casts(0, "longbow".into()), Expect::Dies(1, "training_dummy".into())]),
    ]
}
