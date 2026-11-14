//! The sample game, written in Rust with the content builders.
//!
//! It is small and original: eight shop units, a summon and a neutral
//! monster, five traits, six items (three components and what they combine
//! into), seven augments, a round schedule with every built-in round kind and
//! a scripted one, a sample fight and a few content tests. A game built on
//! the engine defines the same things the same way, in its own crate.
//!
//! Model files, icons and scripts live in the repository's `content/`
//! folder; the content only names them.

use crate::content::{
    AugmentDef, BoardUnit, Bonus, ChoiceOffer, ContentSet, ItemDef, MatchSetup, RoundDef, RoundKindDef, Schedule,
    TeamSetup, TraitDef, UnitDef,
};
use crate::effect::{Ability, Amount, Anchor, DamageKind, Effect, Passive, ScaleBy, Selector, Trigger, Who};
use crate::stats::Stat::*;
use crate::tools::cli::GameSpec;
use crate::tools::scenario::{Expect, Scenario};

/// The folder that holds the sample game's models, icons and sounds.
pub const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/content");

/// The sample game as the command line sees it.
pub fn spec() -> GameSpec {
    GameSpec::new("sample", content).sample_match(sample_match).tests(tests).assets(ASSETS)
}

/// The whole sample game.
pub fn content() -> ContentSet {
    ContentSet::builder()
        .units(units())
        .trait_defs(traits())
        .items(items())
        .augments(augments())
        .schedule(schedule())
        .script("payday", include_str!("../content/scripts/payday.rhai"))
        .script("second_wind", include_str!("../content/scripts/second_wind.rhai"))
        .build()
        .expect("the sample content is valid")
}

/// Two boards to fight each other. Positions are `(column, row)` from each
/// player's own point of view: rows 4 to 7 are a player's half, with row 4
/// the front line.
pub fn sample_match() -> MatchSetup {
    MatchSetup {
        teams: vec![
            TeamSetup::new()
                .unit(BoardUnit::new("ironbark_warden", (3, 4)).star(2).item("barkplate"))
                .unit(BoardUnit::new("emberlash_duelist", (2, 4)).item("cinder_blade"))
                .unit(BoardUnit::new("tidewater_stormcaller", (1, 7)))
                .unit(BoardUnit::new("dawnpetal_mender", (4, 6)))
                .unit(BoardUnit::new("reed_stalker", (5, 7))),
            TeamSetup::new()
                .unit(BoardUnit::new("hollow_lantern_keeper", (3, 4)).star(2))
                .unit(BoardUnit::new("emberlash_duelist", (4, 4)))
                .unit(BoardUnit::new("ashen_hound", (1, 4)).star(2).item("whetstone").item("ember_core"))
                .unit(BoardUnit::new("tidewater_stormcaller", (5, 7)).item("grove_sigil"))
                .unit(BoardUnit::new("ironbark_warden", (2, 5)))
                // An augment-style modifier: the whole team heals from damage dealt.
                .bonus(Bonus::team().mods([Lifesteal.flat(80)])),
        ],
    }
}

/// Fights with an expected result, run by `autoengine test`. They pin down
/// what the content is supposed to do, so a later change that breaks it is
/// caught. Teams are numbered from 0.
pub fn tests() -> Vec<Scenario> {
    vec![
        Scenario::new(
            "two stars beat one star",
            [
                TeamSetup::new().unit(BoardUnit::new("ironbark_warden", (3, 4)).star(2)),
                TeamSetup::new().unit(BoardUnit::new("ironbark_warden", (3, 4))),
            ],
        )
        .runs(10)
        .expect([Expect::Winner(0), Expect::Survives(0, "ironbark_warden".into())]),
        Scenario::new(
            "a mender keeps its front line alive against a golem",
            [
                TeamSetup::new()
                    .unit(BoardUnit::new("ironbark_warden", (3, 4)))
                    .unit(BoardUnit::new("dawnpetal_mender", (3, 7))),
                TeamSetup::new().unit(BoardUnit::new("moss_golem", (3, 4)).star(2)),
            ],
        )
        .runs(5)
        .expect([
            Expect::Winner(0),
            Expect::Casts(0, "dawnpetal_mender".into()),
            Expect::Dies(1, "moss_golem".into()),
            Expect::LastsAtMost { seconds: 45 },
        ]),
        Scenario::new(
            "three cinder units out-fight three without the trait bonus",
            [
                TeamSetup::new()
                    .unit(BoardUnit::new("emberlash_duelist", (2, 4)).item("cinder_blade"))
                    .unit(BoardUnit::new("ashen_hound", (3, 4)))
                    .unit(BoardUnit::new("ashen_hound", (4, 4)).star(2)),
                TeamSetup::new()
                    .unit(BoardUnit::new("emberlash_duelist", (2, 4)))
                    .unit(BoardUnit::new("reed_stalker", (3, 4)))
                    .unit(BoardUnit::new("reed_stalker", (4, 4)).star(2)),
            ],
        )
        .runs(20)
        .expect([Expect::WinsAtLeast { team: 0, percent: 60 }]),
    ]
}

/// The round schedule. When it ends it loops from round 3-1, one stage
/// higher each time, so losses hurt more the longer the game goes on.
pub fn schedule() -> Schedule {
    let golem = |star: u8, pos| BoardUnit::new("moss_golem", pos).star(star);
    Schedule::new([
        RoundDef::new("1-1", 1, RoundKindDef::pve([golem(1, (3, 4))]).items(1)),
        RoundDef::new("1-2", 1, RoundKindDef::pve([golem(1, (2, 4)), golem(1, (4, 4))]).gold(1).items(1)),
        RoundDef::new("1-3", 1, RoundKindDef::pvp()),
        RoundDef::new("2-1", 2, RoundKindDef::choice(3, ChoiceOffer::Augments)),
        RoundDef::new("2-2", 2, RoundKindDef::pvp()),
        RoundDef::new("2-3", 2, RoundKindDef::pvp()),
        RoundDef::new("2-4", 2, RoundKindDef::draft(1).max_cost(2)),
        RoundDef::new("2-5", 2, RoundKindDef::pvp()),
        RoundDef::new(
            "2-6",
            2,
            RoundKindDef::pve([golem(2, (2, 4)), golem(2, (4, 4)), BoardUnit::new("grave_wisp", (3, 6)).star(2)])
                .gold(3)
                .items(1),
        ),
        RoundDef::new("3-1", 3, RoundKindDef::pvp()),
        RoundDef::new("3-2", 3, RoundKindDef::choice(3, ChoiceOffer::Augments)),
        RoundDef::new("3-3", 3, RoundKindDef::pvp()),
        RoundDef::new("3-4", 3, RoundKindDef::draft(1)),
        RoundDef::new("3-5", 3, RoundKindDef::pvp()),
        RoundDef::new("3-6", 3, RoundKindDef::choice(3, ChoiceOffer::Items)),
        RoundDef::new("3-7", 3, RoundKindDef::pvp()),
        // A round type written as a script: see content/scripts/payday.rhai.
        RoundDef::new("3-8", 3, RoundKindDef::custom("payday").param("gold", 2)),
    ])
    .repeat_from(9)
}

/// Traits. A breakpoint is active when at least `count` different units
/// with the trait are on the board; only the highest one reached applies.
pub fn traits() -> Vec<TraitDef> {
    vec![
        TraitDef::new("grove", "Grove")
            .description("Grove units are tougher.")
            .breakpoint(2, [Bonus::members().mods([Armor.flat(15), MagicResist.flat(15)])])
            .breakpoint(3, [Bonus::members().mods([Armor.flat(35), MagicResist.flat(35)])]),
        TraitDef::new("cinder", "Cinder")
            .description("Cinder units attack faster.")
            .breakpoint(2, [Bonus::members().mods([AttackSpeed.percent(200)])])
            .breakpoint(3, [Bonus::members().mods([AttackSpeed.percent(450)])]),
        TraitDef::new("tide", "Tide").description("Tide units steadily restore mana to themselves.").breakpoint(
            2,
            [Bonus::members().passive(
                Passive::new("Rising Tide", Trigger::Every { interval_ms: 2000 })
                    .effect(Effect::mana(10).to(Selector::Source)),
            )],
        ),
        TraitDef::new("hollow", "Hollow")
            .description("When a Hollow unit dies, the whole team hits harder.")
            .breakpoint(
                2,
                [Bonus::members().passive(
                    Passive::new("Hollow Vengeance", Trigger::OnDeath)
                        .effect(Effect::modify_stats([DamageAmp.flat(80)]).to(Selector::AllAllies)),
                )],
            ),
        TraitDef::new("bulwark", "Bulwark")
            .description("The whole team starts combat with a shield; Bulwark units get a bigger one.")
            .breakpoint(
                1,
                [
                    Bonus::team().passive(
                        Passive::new("Bulwark's Cover", Trigger::CombatStart)
                            .effect(Effect::shield_for(60, 8000).to(Selector::Source)),
                    ),
                    Bonus::members().passive(
                        Passive::new("Bulwark's Stand", Trigger::CombatStart)
                            .effect(Effect::shield_for(140, 8000).to(Selector::Source)),
                    ),
                ],
            ),
    ]
}

/// Units. Stats not listed use the engine defaults (see `StatBlock::base`).
pub fn units() -> Vec<UnitDef> {
    vec![
        UnitDef::new("ironbark_warden", "Ironbark Warden")
            .model("models/ironbark_warden.obj", 110)
            .cost(2)
            .traits(["grove", "bulwark"])
            .stats([
                (MaxHealth, 900),
                (AttackDamage, 45),
                (Armor, 40),
                (MagicResist, 30),
                (AttackSpeed, 600),
                (MaxMana, 80),
                (StartingMana, 30),
            ])
            .ability(
                Ability::new("Rootwall")
                    .cast_time(300)
                    .effect(
                        Effect::shield_for(Amount::power(250).plus(Who::Source, ScaleBy::Stat(MaxHealth), 100), 4000)
                            .to(Selector::Source),
                    )
                    .effect(Effect::stun(1000).to(Selector::EnemiesWithin { anchor: Anchor::Source, radius: 1 })),
            )
            .passive(
                Passive::new("Bramble Hide", Trigger::OnDamaged).effect(Effect::magic(12).to(Selector::EventOther)),
            ),
        UnitDef::new("emberlash_duelist", "Emberlash Duelist")
            .model("models/emberlash_duelist.gltf", 100)
            .cost(3)
            .traits(["cinder"])
            .stats([
                (MaxHealth, 650),
                (AttackDamage, 65),
                (Armor, 25),
                (MagicResist, 20),
                (AttackSpeed, 850),
                (MaxMana, 60),
                (Lifesteal, 100),
            ])
            .ability(
                Ability::new("Searing Flurry")
                    .cast_time(250)
                    .effect(
                        Effect::physical(Amount::power(80).plus(Who::Source, ScaleBy::Stat(AttackDamage), 1500))
                            .with_crit()
                            .to(Selector::CurrentTarget),
                    )
                    .effect(Effect::stat_status([AttackSpeed.percent(400)], 3000).to(Selector::Source)),
            )
            .passive(
                Passive::new("Last Spark", Trigger::HealthBelow { permille: 400 })
                    .max_procs(1)
                    .effect(Effect::modify_stats([AttackDamage.percent(300)]).to(Selector::Source)),
            ),
        UnitDef::new("tidewater_stormcaller", "Tidewater Stormcaller")
            .model("models/tidewater_stormcaller.obj", 100)
            .cost(2)
            .traits(["tide"])
            .stats([
                (MaxHealth, 500),
                (AttackDamage, 40),
                (Armor, 15),
                (MagicResist, 15),
                (Range, 4),
                (MaxMana, 70),
                (StartingMana, 20),
            ])
            .ability({
                let splash = Selector::EnemiesWithin { anchor: Anchor::CurrentTarget, radius: 1 };
                Ability::new("Breaking Wave")
                    .cast_time(500)
                    .effect(Effect::magic(Amount::power(180)).to(splash.clone()))
                    .effect(Effect::damage_over_time(Amount::power(30), DamageKind::Magic, 3000).to(splash))
            }),
        UnitDef::new("dawnpetal_mender", "Dawnpetal Mender")
            .model("models/dawnpetal_mender.obj", 100)
            .cost(1)
            .traits(["grove"])
            .stats([(MaxHealth, 520), (AttackDamage, 35), (Armor, 15), (MagicResist, 25), (Range, 3), (MaxMana, 60)])
            .ability(
                Ability::new("Morning Dew").cast_time(400).effect(
                    Effect::heal(Amount::power(150).plus(Who::Target, ScaleBy::MissingHealth, 150))
                        .to(Selector::LowestHealthAllies(2)),
                ),
            ),
        UnitDef::new("hollow_lantern_keeper", "Hollow Lantern Keeper")
            .model("models/hollow_lantern_keeper.obj", 100)
            .cost(2)
            .traits(["hollow", "bulwark"])
            .stats([(MaxHealth, 700), (AttackDamage, 50), (Armor, 30), (MagicResist, 30)])
            .passive(
                Passive::new("Lantern's Release", Trigger::OnDeath)
                    .effect(Effect::summon("grave_wisp", 2).to(Selector::Source)),
            ),
        UnitDef::new("ashen_hound", "Ashen Hound")
            .model("models/ashen_hound.obj", 80)
            .cost(1)
            .traits(["cinder", "hollow"])
            .stats([
                (MaxHealth, 550),
                (AttackDamage, 50),
                (Armor, 20),
                (MagicResist, 20),
                (AttackSpeed, 800),
                (MaxMana, 50),
                (MoveSpeed, 2400),
            ])
            .ability(
                Ability::new("Cinder Bite")
                    .effect(
                        Effect::physical(Amount::flat(0).plus(Who::Source, ScaleBy::Stat(AttackDamage), 2000))
                            .with_crit()
                            .to(Selector::CurrentTarget),
                    )
                    .effect(Effect::stat_status([Armor.percent(-400)], 4000).to(Selector::CurrentTarget)),
            ),
        UnitDef::new("reed_stalker", "Reed Stalker")
            .model("models/reed_stalker.gltf", 100)
            .cost(1)
            .traits(["tide", "grove"])
            .stats([
                (MaxHealth, 480),
                (AttackDamage, 55),
                (Armor, 15),
                (MagicResist, 15),
                (AttackSpeed, 750),
                (Range, 4),
                (MaxMana, 90),
            ])
            .ability(
                Ability::new("Piercing Reed").cast_time(350).effect(
                    Effect::physical(Amount::power(120).plus(Who::Target, ScaleBy::CurrentHealth, 80))
                        .to(Selector::FarthestEnemies(1)),
                ),
            ),
        UnitDef::new("grave_wisp", "Grave Wisp")
            .model("models/grave_wisp.ply", 80)
            .summon_only()
            .traits(["hollow"])
            .stats([(MaxHealth, 200), (AttackDamage, 30), (AttackSpeed, 900)]),
        UnitDef::new("moss_golem", "Moss Golem")
            .model("models/moss_golem.obj", 125)
            .summon_only()
            .stats([(MaxHealth, 450), (AttackDamage, 35), (Armor, 10), (MagicResist, 10), (AttackSpeed, 550)]),
    ]
}

/// Items: three components and the items they combine into.
pub fn items() -> Vec<ItemDef> {
    vec![
        ItemDef::new("whetstone", "Whetstone").mods([AttackDamage.flat(15)]),
        ItemDef::new("oak_charm", "Oak Charm").mods([MaxHealth.flat(150)]),
        ItemDef::new("ember_core", "Ember Core").mods([AttackSpeed.percent(100)]),
        ItemDef::new("barkplate", "Barkplate")
            .recipe("oak_charm", "oak_charm")
            .mods([MaxHealth.flat(350), Armor.flat(20)])
            .passive(
                Passive::new("Barkplate Splinters", Trigger::OnDamaged)
                    .effect(Effect::magic(20).to(Selector::EventOther)),
            ),
        ItemDef::new("cinder_blade", "Cinder Blade")
            .recipe("whetstone", "ember_core")
            // The third Cinder unit has to be made with this blade.
            .grants_trait("cinder")
            .mods([AttackDamage.flat(25), AttackSpeed.percent(150)])
            .passive(
                Passive::new("Cinder Blade Burn", Trigger::OnAttack)
                    .effect(Effect::damage_over_time(20, DamageKind::True, 2000).to(Selector::EventOther)),
            ),
        ItemDef::new("grove_sigil", "Grove Sigil")
            .recipe("oak_charm", "whetstone")
            .mods([MaxHealth.flat(150), AttackDamage.flat(15)])
            .grants_trait("grove"),
    ]
}

/// Augments: lasting, team-wide modifiers a player picks in a choice round.
pub fn augments() -> Vec<AugmentDef> {
    let at_start = |name: &str, shield| {
        Bonus::team().passive(
            Passive::new(name, Trigger::CombatStart).effect(Effect::shield_for(shield, 6000).to(Selector::Source)),
        )
    };
    vec![
        AugmentDef::new("war_chest", "War Chest").description("Gain 12 gold now.").gold(12),
        AugmentDef::new("iron_discipline", "Iron Discipline")
            .description("Your team gains 15 armor and magic resist.")
            .bonus(Bonus::team().mods([Armor.flat(15), MagicResist.flat(15)])),
        AugmentDef::new("blood_pact", "Blood Pact")
            .description("Your team heals for 10% of the damage it deals.")
            .bonus(Bonus::team().mods([Lifesteal.flat(100)])),
        AugmentDef::new("quickening", "Quickening")
            .description("Your team attacks 12% faster.")
            .bonus(Bonus::team().mods([AttackSpeed.percent(120)])),
        AugmentDef::new("tidal_surge", "Tidal Surge")
            .description("Tide units gain 40 ability power.")
            .bonus(Bonus::with_trait("tide").mods([AbilityPower.flat(40)])),
        AugmentDef::new("second_breath", "Second Breath")
            .description("The first time each of your units drops below 30% health, it heals 20% of its maximum.")
            .bonus(Bonus::team().passive(
                Passive::new("Second Breath", Trigger::HealthBelow { permille: 300 }).effect(
                    Effect::heal(Amount::flat(0).plus(Who::Source, ScaleBy::Stat(MaxHealth), 200)).to(Selector::Source),
                ),
            )),
        AugmentDef::new("opening_gambit", "Opening Gambit")
            .description("Your team starts each fight with a 150 shield for 6 seconds.")
            .bonus(at_start("Opening Gambit", 150)),
    ]
}
