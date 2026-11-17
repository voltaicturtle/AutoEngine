//! Ability shapes and timing, and the effects that change who a unit is.

use autoengine::combat::{Combat, CombatConfig, EventKind, Team, UnitId};
use autoengine::effect::{Ability, Cost, Effect, Passive, Selector, Trigger, UnitTemplate};
use autoengine::hex::{Hex, HexBoard};
use autoengine::resource::ResourceDef;
use autoengine::stats::{Stat, StatBlock};

fn at(col: i32, row: i32) -> Hex {
    Hex::from_offset(col, row)
}

/// Lots of health, no resistances, does no damage, never moves.
fn dummy(name: &str) -> UnitTemplate {
    UnitTemplate::new(name, StatBlock::base().with(Stat::MaxHealth, 100_000).with(Stat::AttackDamage, 0).with(Stat::MoveSpeed, 0))
}

fn fight() -> Combat {
    Combat::with_config(HexBoard::new(7, 8), 1, CombatConfig { overtime_after_ms: 0, ..CombatConfig::default() })
}

fn steps(c: &mut Combat, n: u32) {
    for _ in 0..n {
        c.step();
    }
}

fn lost(c: &Combat, id: UnitId) -> i64 {
    c.unit(id).stat(Stat::MaxHealth) - c.unit(id).health()
}

fn spawn(c: &mut Combat, name: &str, team: u8, pos: Hex) -> UnitId {
    c.spawn(dummy(name), Team(team), pos).unwrap()
}

fn sorted(mut ids: Vec<UnitId>) -> Vec<UnitId> {
    ids.sort();
    ids
}

#[test]
fn a_line_goes_through_everyone_in_its_way() {
    let mut c = fight();
    let a = spawn(&mut c, "a", 0, at(0, 3));
    let first = spawn(&mut c, "first", 1, at(1, 3));
    let second = spawn(&mut c, "second", 1, at(2, 3));
    let third = spawn(&mut c, "third", 1, at(3, 3));
    let _far = spawn(&mut c, "far", 1, at(5, 3));
    let _aside = spawn(&mut c, "aside", 1, at(2, 1));
    let _friend = spawn(&mut c, "friend", 0, at(4, 3));
    assert_eq!(c.targets(a, &Selector::EnemiesInLine { length: 3 }), vec![first, second, third]);
    assert_eq!(c.targets(a, &Selector::EnemiesInLine { length: 1 }), vec![first]);
}

#[test]
fn a_cone_widens_toward_the_target() {
    let mut c = fight();
    let src = Hex::new(1, 3);
    let a = c.spawn(dummy("a"), Team(0), src).unwrap();
    let aim = c.spawn(dummy("aim"), Team(1), Hex::new(2, 3)).unwrap();
    // Thirty degrees off the aim, two hexes out: inside.
    let edge = c.spawn(dummy("edge"), Team(1), Hex::new(3, 2)).unwrap();
    // Sixty degrees off: outside.
    let _side = c.spawn(dummy("side"), Team(1), Hex::new(2, 2)).unwrap();
    // Behind, and too far.
    let _behind = c.spawn(dummy("behind"), Team(1), Hex::new(0, 3)).unwrap();
    let far = c.spawn(dummy("far"), Team(1), Hex::new(4, 3)).unwrap();
    assert_eq!(c.targets(a, &Selector::EnemiesInCone { length: 2 }), vec![aim, edge]);
    assert_eq!(c.targets(a, &Selector::EnemiesInCone { length: 3 }), vec![aim, edge, far]);
}

#[test]
fn rows_clusters_and_chains() {
    let mut c = fight();
    let a = spawn(&mut c, "a", 0, at(3, 5));
    let left = spawn(&mut c, "left", 0, at(0, 5));
    let _behind = spawn(&mut c, "behind", 0, at(3, 6));
    assert_eq!(sorted(c.targets(a, &Selector::AlliesInRow)), vec![a, left]);

    // One enemy close by, three bunched up further off.
    let lone = spawn(&mut c, "lone", 1, at(3, 4));
    let g1 = spawn(&mut c, "g1", 1, at(1, 1));
    let g2 = spawn(&mut c, "g2", 1, at(2, 1));
    let g3 = spawn(&mut c, "g3", 1, at(1, 0));
    assert_eq!(sorted(c.targets(a, &Selector::EnemyCluster { radius: 1 })), vec![g1, g2, g3]);

    // A chain starts on the target (the nearest, `lone`) and jumps to the
    // nearest enemy in reach that it has not hit yet.
    assert_eq!(c.targets(a, &Selector::Chain { count: 5, range: 2 }), vec![lone]);
    // g1 and g2 are both three hexes from `lone`: the lower id goes first.
    assert_eq!(c.targets(a, &Selector::Chain { count: 3, range: 4 }), vec![lone, g1, g2]);
    assert_eq!(c.targets(a, &Selector::Chain { count: 9, range: 4 }), vec![lone, g1, g2, g3]);
}

#[test]
fn an_effect_that_travels_reaches_the_far_ones_later() {
    let mut c = fight();
    let wave = Passive::new("Wave", Trigger::CombatStart).effect(Effect::true_damage(100).to(Selector::AllEnemies).travel(500));
    c.spawn(dummy("a").with_passive(wave), Team(0), at(3, 7)).unwrap();
    let near = spawn(&mut c, "near", 1, at(3, 6));
    let far = spawn(&mut c, "far", 1, at(3, 3));
    // One hex is half a second (10 ticks), four hexes two seconds.
    steps(&mut c, 9);
    assert_eq!((lost(&c, near), lost(&c, far)), (0, 0));
    steps(&mut c, 1);
    assert_eq!((lost(&c, near), lost(&c, far)), (100, 0));
    steps(&mut c, 29);
    assert_eq!(lost(&c, far), 0);
    steps(&mut c, 1);
    assert_eq!(lost(&c, far), 100);
}

#[test]
fn nobody_can_choose_an_untargetable_unit() {
    let mut c = fight();
    let hitter = UnitTemplate::new("hitter", StatBlock::base().with(Stat::AttackDamage, 100).with(Stat::CritChance, 0).with(Stat::MoveSpeed, 0));
    let hitter = c.spawn(hitter, Team(0), at(3, 4)).unwrap();
    let hidden = spawn(&mut c, "hidden", 1, at(3, 3));
    let seen = spawn(&mut c, "seen", 1, at(5, 4));
    c.spawn(
        dummy("helper").with_passive(Passive::new("Veil", Trigger::CombatStart).effect(Effect::untargetable(2000).to(Selector::AllAllies))),
        Team(1),
        at(0, 0),
    )
    .unwrap();
    // The veil has not landed before the fight starts.
    assert_eq!(c.targets(hitter, &Selector::NearestEnemies(1)), vec![hidden]);
    steps(&mut c, 1);
    assert!(!c.unit(hidden).is_targetable() && c.unit(hidden).is_alive());
    assert!(!c.targets(hitter, &Selector::AllEnemies).contains(&hidden));
    assert_eq!(c.targets(hitter, &Selector::CurrentTarget).len(), 0);
    // The unit next to it is passed over: the hitter walks to... nobody it
    // can reach, since everyone on the other side is veiled.
    steps(&mut c, 30);
    assert_eq!((lost(&c, hidden), lost(&c, seen)), (0, 0));
    // When the veil wears off it is fair game again.
    steps(&mut c, 40);
    assert!(c.unit(hidden).is_targetable());
    steps(&mut c, 40);
    assert!(lost(&c, hidden) > 0);
}

#[test]
fn a_unit_can_turn_into_another_and_keep_what_it_was_given() {
    let mut c = fight();
    let plain = UnitTemplate::new("Pup", StatBlock::base().with(Stat::MaxHealth, 1000).with(Stat::AttackDamage, 100));
    let form = UnitTemplate::new("Wolf", StatBlock::base().with(Stat::MaxHealth, 2000).with(Stat::AttackDamage, 300).with(Stat::Armor, 40))
        .with_ability(Ability::new("Howl").cost(Cost::Free).effect(Effect::true_damage(1).to(Selector::Source)));
    c.register_template("pup", UnitTemplate { id: "pup".into(), ..plain.clone() });
    c.register_template("wolf", form);
    // This pup holds an item worth 50 attack damage, and hurts itself
    // before it changes.
    let mut held = plain;
    held.id = "pup".into();
    held.stats = held.stats.with(Stat::AttackDamage, 150);
    let change = Passive::new("Moon", Trigger::CombatStart)
        .effect(Effect::true_damage(500).to(Selector::Source))
        .effect(Effect::transform("wolf").to(Selector::Source));
    let pup = c.spawn(held.with_passive(change), Team(0), at(3, 4)).unwrap();
    spawn(&mut c, "dummy", 1, at(3, 0));
    steps(&mut c, 1);
    let wolf = c.unit(pup);
    assert_eq!(wolf.name(), "Wolf");
    assert_eq!((wolf.stat(Stat::AttackDamage), wolf.stat(Stat::Armor)), (350, 40));
    // Half of its health before, half after.
    assert_eq!(wolf.stat(Stat::MaxHealth), 2000);
    assert_eq!(wolf.template().abilities.len(), 1);
    assert_eq!(wolf.template().passives.len(), 1);
    let events: Vec<&EventKind> = c.events().iter().map(|e| &e.kind).collect();
    assert!(events.iter().any(|e| matches!(e, EventKind::Transformed { into, name, .. } if into == "wolf" && name == "Wolf")));
    assert!(events.iter().any(|e| matches!(e, EventKind::HealthSet { health: 1000, max_health: 2000, .. })));
    // Its free ability goes off on its own, at once.
    assert!(c.unit(pup).health() < 1000);
}

#[test]
fn copies_and_swaps() {
    let mut c = fight();
    let copier = Passive::new("Mirror", Trigger::CombatStart).effect(Effect::copy(500).to(Selector::CurrentTarget));
    let a = c.spawn(dummy("a").with_passive(copier), Team(0), at(3, 7)).unwrap();
    let foe = spawn(&mut c, "foe", 1, at(3, 6));
    steps(&mut c, 1);
    // A copy of the enemy at half health, on the copier's side.
    let copy = c.units().iter().find(|u| u.id() != a && u.id() != foe).expect("a copy").id();
    let unit = c.unit(copy);
    assert_eq!((unit.name(), unit.team(), unit.health()), ("foe", Team(0), 50_000));
    assert_eq!(unit.pos().distance(c.unit(foe).pos()), 1);

    let mut c = fight();
    let swapper = Passive::new("Switch", Trigger::CombatStart).effect(Effect::Swap.to(Selector::FarthestEnemies(1)));
    let a = c.spawn(dummy("a").with_passive(swapper), Team(0), at(3, 7)).unwrap();
    let far = spawn(&mut c, "far", 1, at(1, 0));
    steps(&mut c, 1);
    assert_eq!((c.unit(a).pos(), c.unit(far).pos()), (at(1, 0), at(3, 7)));
    assert_eq!((c.unit_at(at(1, 0)), c.unit_at(at(3, 7))), (Some(a), Some(far)));
}

#[test]
fn a_resource_can_pause_after_a_cast() {
    let config = CombatConfig {
        overtime_after_ms: 0,
        resources: vec![ResourceDef::mana().lock_after_cast(1000)],
        ..CombatConfig::default()
    };
    let mut c = Combat::with_config(HexBoard::new(7, 8), 1, config);
    // Starts full, attacks five times a second for 10 mana each.
    let caster = UnitTemplate::new(
        "caster",
        StatBlock::base()
            .with(Stat::MaxMana, 50)
            .with(Stat::StartingMana, 50)
            .with(Stat::ManaPerAttack, 10)
            .with(Stat::AttackSpeed, 5000)
            .with(Stat::AttackDamage, 1)
            .with(Stat::CritChance, 0),
    )
    .with_ability(Ability::new("Pop").effect(Effect::true_damage(1).to(Selector::CurrentTarget)));
    let caster = c.spawn(caster, Team(0), at(3, 4)).unwrap();
    spawn(&mut c, "dummy", 1, at(3, 3));
    steps(&mut c, 1);
    assert_eq!(c.unit(caster).resource("mana"), 0);
    // Attacks during the second after the cast give nothing.
    steps(&mut c, 18);
    assert_eq!(c.unit(caster).resource("mana"), 0);
    steps(&mut c, 10);
    assert!(c.unit(caster).resource("mana") > 0);
}

#[test]
fn the_checker_knows_the_new_effects() {
    use autoengine::content::UnitDef;
    let base = || autoengine::sample::content().into_builder();
    let with = |effect: Effect| base().unit(UnitDef::new("x", "X").cost(1).passive(Passive::new("P", Trigger::CombatStart).effect(effect.to(Selector::Source))));
    let err = |b: autoengine::content::GameBuilder| b.build().err().expect("should not build").to_string();
    assert!(err(with(Effect::transform("wolff"))).contains("transforms into unknown unit `wolff`"));
    assert!(err(with(Effect::grow([]))).contains("grows a unit by nothing"));
    assert!(err(with(Effect::reward(autoengine::game::Grant::Unit("nobody".into())))).contains("gives unknown unit `nobody`"));
    assert!(with(Effect::transform("moss_golem")).build().is_ok());
}

#[test]
fn a_condition_can_tell_the_front_rows_from_the_back() {
    use autoengine::effect::{Condition, Who};
    let mut c = fight();
    let front = Condition::InFrontRows { who: Who::Source, rows: 1 };
    let guard = |name: &str| {
        dummy(name).with_passive(Passive::new("Stance", Trigger::CombatStart).effect(Effect::true_damage(10).to(Selector::Source).when(front.clone())))
    };
    // The first side's front row is the fifth (row 4), the second side's
    // the fourth (row 3).
    let a_front = c.spawn(guard("a front"), Team(0), at(1, 4)).unwrap();
    let a_back = c.spawn(guard("a back"), Team(0), at(1, 6)).unwrap();
    let b_front = c.spawn(guard("b front"), Team(1), at(5, 3)).unwrap();
    let b_back = c.spawn(guard("b back"), Team(1), at(5, 1)).unwrap();
    steps(&mut c, 1);
    assert_eq!([a_front, a_back, b_front, b_back].map(|u| lost(&c, u)), [10, 0, 10, 0]);
}
