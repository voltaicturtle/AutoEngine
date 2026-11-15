//! Resources: mana is one of them, and a game can define its own with
//! their own ways of being gained, spent and lost.

use autoengine::combat::{Combat, CombatConfig, EventKind, Team, UnitId};
use autoengine::content::{BoardUnit, ContentSet, ItemDef, TeamSetup, UnitDef};
use autoengine::effect::{
    Ability, Amount, Condition, Cost, Effect, Passive, ScaleBy, Selector, Trigger, UnitTemplate, Who,
};
use autoengine::hex::{Hex, HexBoard};
use autoengine::playback::Playback;
use autoengine::resource::{MANA, ResourceDef};
use autoengine::stats::{Stat, StatBlock};
use autoengine::tools::lint;

const MAX_RAGE: Stat = Stat::custom("max_rage");
const MAX_ENERGY: Stat = Stat::custom("max_energy");

/// A resource with no rules: only effects change it.
fn plain_rage() -> ResourceDef {
    ResourceDef::new("rage", "Rage", MAX_RAGE)
}

/// No crits, 100 attack damage, one attack per second (every 20 ticks).
fn hitter(name: &str) -> UnitTemplate {
    UnitTemplate::new(
        name,
        StatBlock::base().with(Stat::CritChance, 0).with(Stat::AttackDamage, 100).with(Stat::AttackSpeed, 1000),
    )
}

/// Lots of health, no resistances, does no damage.
fn dummy(name: &str) -> UnitTemplate {
    UnitTemplate::new(name, StatBlock::base().with(Stat::MaxHealth, 100_000).with(Stat::AttackDamage, 0))
}

fn duel(resources: Vec<ResourceDef>, a: UnitTemplate, b: UnitTemplate) -> (Combat, UnitId, UnitId) {
    let config = CombatConfig { resources, overtime_after_ms: 0, ..CombatConfig::default() };
    let mut c = Combat::with_config(HexBoard::new(7, 8), 1, config);
    let ia = c.spawn(a, Team(0), Hex::from_offset(3, 3)).unwrap();
    let ib = c.spawn(b, Team(1), Hex::from_offset(3, 4)).unwrap();
    (c, ia, ib)
}

fn steps(c: &mut Combat, n: u32) {
    for _ in 0..n {
        c.step();
    }
}

fn casts(c: &Combat) -> usize {
    c.events().iter().filter(|e| matches!(e.kind, EventKind::CastFinished { .. })).count()
}

#[test]
fn a_resource_follows_its_own_rules() {
    // Rage comes from dealing damage and leaks away every second. The
    // ability takes a fixed amount and leaves the rest.
    let rage = plain_rage().from_damage_dealt(100, None).per_second(-2);
    let berserker = hitter("Berserker")
        .with_stat(MAX_RAGE, 100)
        .with_ability(Ability::new("Rampage").cost(Cost::of("rage", 20)).effect(Effect::true_damage(500).to(Selector::CurrentTarget)));
    let (mut c, a, b) = duel(vec![rage], berserker, dummy("Dummy"));

    steps(&mut c, 20);
    assert_eq!(c.unit(a).resource("rage"), 10, "a tenth of the 100 damage dealt");
    steps(&mut c, 20);
    assert_eq!(c.unit(a).resource("rage"), 18, "two leaked away, ten more came in");
    steps(&mut c, 20);
    assert_eq!((c.unit(a).resource("rage"), casts(&c)), (26, 0));
    c.step();
    assert_eq!((c.unit(a).resource("rage"), casts(&c)), (56, 1), "the cast took 20, and its 500 damage gave back 50");
    assert_eq!(c.unit(b).health(), 100_000 - 300 - 500);

    // Mana is not part of this fight, so nobody has any.
    assert_eq!(c.unit(a).mana(), 0);
    assert!(c.unit(a).pool(MANA).is_none());
    // The dummy has no capacity for rage, so it does not have the resource.
    assert!(c.unit(b).pools().next().is_none());
}

#[test]
fn rage_can_come_from_being_hit_and_from_kills() {
    let rage = plain_rage().from_damage_taken(500, Some(30)).per_kill(40);
    let bruiser = dummy("Bruiser").with_stat(MAX_RAGE, 100);
    let killer = hitter("Killer").with_stat(MAX_RAGE, 100).with_stat(Stat::AttackDamage, 10_000);
    let victim = UnitTemplate::new("Victim", StatBlock::base().with(Stat::AttackDamage, 0));

    let (mut c, _, hit) = duel(vec![rage.clone()], hitter("Hitter"), bruiser);
    steps(&mut c, 20);
    assert_eq!(c.unit(hit).resource("rage"), 30, "half of 100 damage, capped at 30 a hit");

    let (mut c, k, _) = duel(vec![rage], killer, victim);
    steps(&mut c, 20);
    assert_eq!(c.unit(k).resource("rage"), 40);
}

#[test]
fn energy_refills_over_time_and_cooldowns_space_out_casts() {
    let energy = ResourceDef::new("energy", "Energy", MAX_ENERGY).starts_full().per_second(10);
    let duelist = dummy("Duelist").with_stat(MAX_ENERGY, 100).with_ability(
        Ability::new("Lunge").cost(Cost::of("energy", 40)).cooldown(3000).effect(Effect::true_damage(100).to(Selector::CurrentTarget)),
    );
    let (mut c, a, _) = duel(vec![energy], duelist, dummy("Dummy"));

    assert_eq!(c.unit(a).resource("energy"), 100, "starts full");
    c.step();
    assert_eq!((c.unit(a).resource("energy"), casts(&c)), (60, 1));
    assert_eq!(c.unit(a).cooldown_ms(), 3000);
    // Enough energy for another cast, but the cooldown holds it back.
    steps(&mut c, 59);
    assert_eq!((c.unit(a).resource("energy"), casts(&c)), (90, 1));
    c.step();
    assert_eq!((c.unit(a).resource("energy"), casts(&c)), (50, 2));
}

#[test]
fn a_free_ability_runs_on_its_cooldown_alone() {
    let pulse = dummy("Pulse").with_ability(
        Ability::new("Pulse").cost(Cost::Free).cooldown(1000).effect(Effect::true_damage(10).to(Selector::AllEnemies)),
    );
    let (mut c, _, b) = duel(vec![], pulse, dummy("Dummy"));
    steps(&mut c, 41);
    assert_eq!(casts(&c), 3, "at 0, 1 and 2 seconds");
    assert_eq!(c.unit(b).health(), 100_000 - 30);
}

#[test]
fn a_unit_can_carry_several_resources() {
    let both = hitter("Both")
        .with_stat(Stat::MaxMana, 20)
        .with_stat(MAX_RAGE, 50)
        .with_passive(Passive::new("Seethe", Trigger::OnAttack).effect(Effect::resource("rage", 5).to(Selector::Source)))
        .with_ability(Ability::new("Bolt").effect(Effect::true_damage(1000).to(Selector::CurrentTarget)));
    let (mut c, a, b) = duel(vec![ResourceDef::mana(), plain_rage()], both, dummy("Dummy"));

    steps(&mut c, 40);
    let unit = c.unit(a);
    let pools: Vec<(&str, i64, i64)> = unit.pools().map(|p| (p.def.id.as_str(), p.amount, p.max)).collect();
    assert_eq!(pools, [("mana", 20, 20), ("rage", 10, 50)]);
    // The ability costs a full bar of mana by default; rage is untouched.
    c.step();
    assert_eq!((c.unit(a).mana(), c.unit(a).resource("rage"), casts(&c)), (0, 10, 1));
    assert_eq!(c.unit(b).health(), 100_000 - 200 - 1000);
}

#[test]
fn effects_conditions_and_amounts_can_read_resources() {
    // Starts with 60 rage; while at least half full, attacks deal the
    // current rage as extra true damage and then burn 20 of it.
    let on_hit = Passive::new("Burn", Trigger::OnAttack)
        .when(Condition::ResourceAtLeast { who: Who::Source, resource: "rage".into(), permille: 500 })
        .effect(Effect::true_damage(Amount::default().plus(Who::Source, ScaleBy::Resource("rage".into()), 1000)).to(Selector::EventOther))
        .effect(Effect::resource("rage", -20).to(Selector::Source));
    let unit = hitter("Furnace")
        .with_stat(MAX_RAGE, 100)
        .with_passive(Passive::new("Stoke", Trigger::CombatStart).effect(Effect::resource("rage", 60).to(Selector::Source)))
        .with_passive(on_hit);
    let (mut c, a, b) = duel(vec![plain_rage()], unit, dummy("Dummy"));

    steps(&mut c, 20);
    assert_eq!((c.unit(a).resource("rage"), c.unit(b).health()), (40, 100_000 - 100 - 60));
    // 40 is under half of 100, so the passive stays quiet from here on.
    steps(&mut c, 20);
    assert_eq!((c.unit(a).resource("rage"), c.unit(b).health()), (40, 100_000 - 100 - 60 - 100));
    // Giving a resource to a unit without it does nothing.
    assert_eq!(c.unit(b).resource("rage"), 0);
}

#[test]
fn capacity_is_a_stat_so_anything_that_changes_stats_changes_it() {
    let shrink = Passive::new("Shrink", Trigger::Every { interval_ms: 1000 })
        .max_procs(1)
        .effect(Effect::stat_status([MAX_RAGE.flat(-70)], 1000).to(Selector::Source));
    let unit = dummy("Vessel").with_stat(MAX_RAGE, 100).with_passive(shrink);
    let (mut c, a, _) = duel(vec![plain_rage().starts_full()], unit, dummy("Dummy"));

    steps(&mut c, 19);
    assert_eq!(c.unit(a).pool("rage").map(|p| (p.amount, p.max)), Some((100, 100)));
    steps(&mut c, 1);
    assert_eq!(c.unit(a).pool("rage").map(|p| (p.amount, p.max)), Some((30, 30)), "what no longer fits is lost");
    steps(&mut c, 25);
    assert_eq!(c.unit(a).pool("rage").map(|p| (p.amount, p.max)), Some((30, 100)), "and does not come back");

    // Every change is reported, with the capacity, so a bar can be drawn.
    let seen: Vec<(i64, i64)> = c
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ResourceChanged { unit, resource, amount, max } if *unit == a && resource == "rage" => Some((*amount, *max)),
            _ => None,
        })
        .collect();
    assert_eq!(seen, [(100, 100), (30, 30), (30, 100)]);

    let mut playback = Playback::from_events(c.events().to_vec(), c.config().tick_ms);
    playback.advance_to(1000);
    let bar = playback.unit(a).unwrap().resource("rage").unwrap();
    assert_eq!((bar.amount, bar.max), (30, 30));
    assert!(playback.unit(UnitId(1)).unwrap().resources.is_empty());
}

#[test]
fn mana_does_not_build_during_a_cast_unless_the_game_says_so() {
    let fight = |mana: ResourceDef| {
        let caster = hitter("Caster")
            .with_stat(Stat::MaxMana, 10)
            .with_stat(Stat::StartingMana, 10)
            .with_passive(Passive::new("Trickle", Trigger::Every { interval_ms: 100 }).effect(Effect::mana(1).to(Selector::Source)))
            .with_ability(Ability::new("Channel").cast_time(500).effect(Effect::true_damage(1).to(Selector::CurrentTarget)));
        let (mut c, a, _) = duel(vec![mana], caster, dummy("Dummy"));
        steps(&mut c, 6);
        assert!(c.unit(a).is_casting());
        c.unit(a).mana()
    };
    assert_eq!(fight(ResourceDef::mana()), 0);
    assert!(fight(ResourceDef::mana().changes_while_casting()) > 0);
}

fn x() -> UnitDef {
    UnitDef::new("x", "X")
}

fn error(result: Result<ContentSet, autoengine::content::ContentError>) -> String {
    result.err().map(|e| e.to_string()).unwrap_or_default()
}

#[test]
fn games_define_resources_and_mistakes_are_caught() {
    let ability = |cost: Cost| Ability::new("Strike").cost(cost).effect(Effect::physical(10).to(Selector::CurrentTarget));
    let energy = || ResourceDef::new("energy", "Energy", MAX_ENERGY).per_second(10);

    let game = ContentSet::builder()
        .resource(energy())
        .resource(ResourceDef::mana().from_damage_taken(0, None))
        .unit(x().stat(MAX_ENERGY, 100).ability(ability(Cost::of("energy", 40))))
        .item(ItemDef::new("cell", "Cell").mods([MAX_ENERGY.flat(50)]))
        .build()
        .unwrap();
    let ids: Vec<&str> = game.resources().iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["mana", "energy"], "replacing mana keeps its place");
    assert_eq!(game.resource("mana").unwrap().from_damage_taken.permille, 0);

    // Items change capacity like any other stat.
    let team = TeamSetup::new().unit(BoardUnit::new("x", (3, 0)).item("cell"));
    let placed = game.build_team(&team).unwrap();
    assert_eq!(placed[0].template.stats.get(MAX_ENERGY), 150);

    let unknown = error(ContentSet::builder().resource(energy()).unit(x().ability(ability(Cost::full("enrgy")))).build());
    assert!(unknown.contains("unknown resource `enrgy` (did you mean `energy`?)"), "{unknown}");
    let no_mana = error(ContentSet::builder().without_resource("mana").unit(x().ability(ability(Cost::default()))).build());
    assert!(no_mana.contains("unknown resource `mana`"), "{no_mana}");
    let gives = x().passive(Passive::new("", Trigger::OnKill).effect(Effect::resource("fury", 5).to(Selector::Source)));
    assert!(error(ContentSet::builder().unit(gives).build()).contains("unknown resource `fury`"));
    let reads = Effect::heal(Amount::default().plus(Who::Source, ScaleBy::Resource("fury".into()), 1000));
    let reads = x().ability(Ability::new("Mend").effect(reads.to(Selector::Source)));
    assert!(error(ContentSet::builder().unit(reads).build()).contains("unknown resource `fury`"));
    let asks = Condition::ResourceAtLeast { who: Who::Source, resource: "fury".into(), permille: 1 };
    let asks = x().passive(Passive::new("", Trigger::OnKill).when(asks));
    assert!(error(ContentSet::builder().unit(asks).build()).contains("unknown resource `fury`"));

    assert!(error(ContentSet::builder().unit(x().ability(ability(Cost::Free))).build()).contains("no cooldown"));
    assert!(error(ContentSet::builder().unit(x().ability(ability(Cost::of("mana", 0)))).build()).contains("Cost::Free"));
    let twice = error(ContentSet::builder().rules(Default::default()).resource(energy()).build());
    assert!(twice.is_empty(), "adding a resource once is fine: {twice}");
}

#[test]
fn lint_points_out_abilities_that_can_never_be_cast() {
    let ability = |cost: Cost| Ability::new("Strike").cost(cost).effect(Effect::physical(10).to(Selector::CurrentTarget));
    let game = ContentSet::builder()
        .resource(ResourceDef::new("energy", "Energy", MAX_ENERGY))
        .unit(UnitDef::new("no_mana", "No Mana").ability(ability(Cost::default())))
        .unit(UnitDef::new("too_small", "Too Small").stat(MAX_ENERGY, 30).ability(ability(Cost::of("energy", 40))))
        .unit(UnitDef::new("fine", "Fine").stat(Stat::MaxMana, 50).ability(ability(Cost::default())))
        .build()
        .unwrap();
    let warnings: Vec<String> = lint(&game, None).iter().map(|w| w.to_string()).filter(|w| w.contains("never cast")).collect();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("no_mana") && w.contains("0 mana (the MaxMana stat)")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("too_small") && w.contains("30 energy (the max_energy stat)")), "{warnings:?}");
}

#[test]
fn the_sample_game_plays_the_same_with_mana_as_a_resource() {
    // The built-in mana is described with the same data any other resource
    // uses, so this pins down that the description matches the old rules.
    let mana = ResourceDef::mana();
    assert_eq!((mana.id.as_str(), mana.max), ("mana", Stat::MaxMana));
    assert_eq!(mana.start, Amount::stat(Stat::StartingMana));
    assert_eq!(mana.per_attack, Amount::stat(Stat::ManaPerAttack));
    assert_eq!((mana.from_damage_taken.permille, mana.from_damage_taken.cap), (30, Some(40)));
    assert_eq!(CombatConfig::default().resources, vec![mana]);
}
