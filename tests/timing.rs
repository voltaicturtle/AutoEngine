//! Effects that land later or more than once, and bringing units back.

use autoengine::combat::{Combat, CombatConfig, EventKind, Team, UnitId};
use autoengine::content::{ContentSet, UnitDef};
use autoengine::effect::{Ability, Amount, Effect, Passive, ScaleBy, Selector, Trigger, UnitTemplate, Who};
use autoengine::hex::{Hex, HexBoard};
use autoengine::playback::Playback;
use autoengine::stats::{Stat, StatBlock};
use autoengine::tools::lint;

fn at(col: i32, row: i32) -> Hex {
    Hex::from_offset(col, row)
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

/// A dummy that does `spec` when the fight starts, next to a plain dummy.
fn opening(spec: autoengine::effect::EffectSpec) -> (Combat, UnitId, UnitId) {
    let mut c = fight();
    let a = c.spawn(dummy("a").with_passive(Passive::new("Opener", Trigger::CombatStart).effect(spec)), Team(0), at(3, 3)).unwrap();
    let b = c.spawn(dummy("b"), Team(1), at(3, 4)).unwrap();
    (c, a, b)
}

#[test]
fn a_delayed_effect_lands_later() {
    // One second is 20 ticks.
    let (mut c, _, b) = opening(Effect::true_damage(100).to(Selector::CurrentTarget).after(1000));
    steps(&mut c, 19);
    assert_eq!(lost(&c, b), 0);
    steps(&mut c, 1);
    assert_eq!(lost(&c, b), 100);
    steps(&mut c, 40);
    assert_eq!(lost(&c, b), 100);
}

#[test]
fn a_repeated_effect_lands_several_times() {
    // At once, then twice more half a second apart.
    let (mut c, _, b) = opening(Effect::true_damage(100).to(Selector::CurrentTarget).repeat(3, 500));
    steps(&mut c, 1);
    assert_eq!(lost(&c, b), 100);
    steps(&mut c, 8);
    assert_eq!(lost(&c, b), 100);
    steps(&mut c, 1);
    assert_eq!(lost(&c, b), 200);
    steps(&mut c, 10);
    assert_eq!(lost(&c, b), 300);
    steps(&mut c, 60);
    assert_eq!(lost(&c, b), 300);

    // With a delay, the first landing waits too.
    let (mut c, _, b) = opening(Effect::true_damage(100).to(Selector::CurrentTarget).after(500).repeat(2, 250));
    let mut landed = Vec::new();
    for tick in 1..=40 {
        let before = lost(&c, b);
        c.step();
        if lost(&c, b) > before {
            landed.push(tick);
        }
    }
    assert_eq!(landed, vec![10, 15]);
}

#[test]
fn a_delayed_effect_outlives_its_source_and_skips_the_dead() {
    // The bomber dies long before its two-second fuse runs out.
    let mut c = fight();
    let bomb = Passive::new("Fuse", Trigger::CombatStart).effect(Effect::true_damage(100).to(Selector::AllEnemies).after(2000));
    let bomber = UnitTemplate::new("bomber", StatBlock::base().with(Stat::MaxHealth, 50).with(Stat::AttackDamage, 0)).with_passive(bomb);
    let a = c.spawn(bomber, Team(0), at(3, 3)).unwrap();
    c.spawn(dummy("friend"), Team(0), at(0, 0)).unwrap();
    let killer = c.spawn(hitter("killer").with_stat(Stat::MaxHealth, 100_000), Team(1), at(3, 4)).unwrap();
    let weak = c.spawn(UnitTemplate::new("weak", StatBlock::base().with(Stat::MaxHealth, 1).with(Stat::AttackDamage, 0)), Team(1), at(6, 7)).unwrap();
    steps(&mut c, 39);
    assert!(!c.unit(a).is_alive());
    assert_eq!(lost(&c, killer), 0);
    steps(&mut c, 1);
    assert_eq!(lost(&c, killer), 100);
    assert!(!c.unit(weak).is_alive());
    // Nothing more comes of it.
    steps(&mut c, 40);
    assert_eq!(lost(&c, killer), 100);
}

#[test]
fn a_unit_can_get_back_up_once() {
    let second_wind = Passive::new("Second Wind", Trigger::OnDeath).max_procs(1).effect(Effect::revive(50).to(Selector::Source));
    let mut c = fight();
    let phoenix = UnitTemplate::new("phoenix", StatBlock::base().with(Stat::MaxHealth, 100).with(Stat::AttackDamage, 0)).with_passive(second_wind);
    let a = c.spawn(phoenix, Team(0), at(3, 3)).unwrap();
    c.spawn(hitter("killer"), Team(1), at(3, 4)).unwrap();
    let outcome = c.run().clone();
    assert_eq!(outcome.winner, Some(Team(1)));
    let deaths = c.events().iter().filter(|e| matches!(e.kind, EventKind::Died { unit, .. } if unit == a)).count();
    let revivals: Vec<(Hex, i64)> = c
        .events()
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::Revived { unit, pos, health, .. } if unit == a => Some((pos, health)),
            _ => None,
        })
        .collect();
    assert_eq!(deaths, 2);
    assert_eq!(revivals, vec![(at(3, 3), 50)]);

    // Replaying the events shows it standing again in between.
    let first_death = c.events().iter().find(|e| matches!(e.kind, EventKind::Died { .. })).unwrap().tick;
    let mut playback = Playback::from_events(c.events().to_vec(), CombatConfig::default().tick_ms);
    playback.advance_to((first_death + 1) * CombatConfig::default().tick_ms);
    let view = playback.unit(a).unwrap();
    assert!(view.alive);
    assert_eq!(view.health, 50);
    playback.advance_to(u32::MAX);
    assert!(!playback.unit(a).unwrap().alive);
}

#[test]
fn fallen_allies_can_be_raised() {
    // A healer that raises whoever falls, with 30% of their health.
    let share = Amount::default().plus(Who::Target, ScaleBy::Stat(Stat::MaxHealth), 300);
    let raise = Passive::new("Raise", Trigger::OnAllyDeath).max_procs(1).effect(Effect::revive(share).to(Selector::FallenAllies(1)));
    let mut c = fight();
    c.spawn(dummy("healer").with_passive(raise), Team(0), at(0, 0)).unwrap();
    let soldier = c
        .spawn(UnitTemplate::new("soldier", StatBlock::base().with(Stat::MaxHealth, 200).with(Stat::AttackDamage, 0)), Team(0), at(3, 3))
        .unwrap();
    c.spawn(hitter("killer").with_stat(Stat::MaxHealth, 100_000), Team(1), at(3, 4)).unwrap();
    let mut revived_with = None;
    for _ in 0..200 {
        c.step();
        if let Some(health) = c.events().iter().find_map(|e| match e.kind {
            EventKind::Revived { unit, health, .. } if unit == soldier => Some(health),
            _ => None,
        }) {
            revived_with = Some(health);
            break;
        }
    }
    assert_eq!(revived_with, Some(60));
    assert!(c.unit(soldier).is_alive());
    // The living are left alone: reviving is not a heal.
    let mut c = fight();
    let a = c
        .spawn(dummy("a").with_passive(Passive::new("Opener", Trigger::CombatStart).effect(Effect::revive(10).to(Selector::AllAllies))), Team(0), at(3, 3))
        .unwrap();
    c.spawn(dummy("b"), Team(1), at(3, 4)).unwrap();
    steps(&mut c, 5);
    assert_eq!(lost(&c, a), 0);
    assert!(!c.events().iter().any(|e| matches!(e.kind, EventKind::Revived { .. })));
}

#[test]
fn timing_is_checked_when_content_is_built() {
    let unit = |ability: Ability| UnitDef::new("pawn", "Pawn").cost(1).stat(Stat::MaxMana, 50).ability(ability);
    let bad = ContentSet::builder().unit(unit(Ability::new("Volley").effect(Effect::physical(10).to(Selector::CurrentTarget).repeat(0, 100)))).build();
    assert!(bad.err().unwrap().to_string().contains("repeats an effect 0 times"));
    let bad = ContentSet::builder().unit(unit(Ability::new("Volley").effect(Effect::physical(10).to(Selector::CurrentTarget).repeat(3, 0)))).build();
    assert!(bad.is_err());

    // Reviving the living is a mistake lint can see.
    let content = ContentSet::builder().unit(unit(Ability::new("Rise").effect(Effect::revive(100).to(Selector::AllAllies)))).build().unwrap();
    assert!(lint(&content, None).iter().any(|w| w.message.contains("revives units its selector only picks while they are alive")));
    let content = ContentSet::builder().unit(unit(Ability::new("Rise").effect(Effect::revive(100).to(Selector::FallenAllies(1))))).build().unwrap();
    assert!(!lint(&content, None).iter().any(|w| w.message.contains("revives")));
    // So is a unit that gets up every single time.
    let immortal = |passive: Passive| ContentSet::builder().unit(UnitDef::new("pawn", "Pawn").cost(1).passive(passive)).build().unwrap();
    let forever = Passive::new("Undying", Trigger::OnDeath).effect(Effect::revive(100).to(Selector::Source));
    assert!(lint(&immortal(forever.clone()), None).iter().any(|w| w.message.contains("can never be killed")));
    assert!(!lint(&immortal(forever.max_procs(1)), None).iter().any(|w| w.message.contains("can never be killed")));
}

#[test]
fn delayed_effects_replay_the_same() {
    let run = || {
        let (mut c, _, _) = opening(Effect::true_damage(100).to(Selector::CurrentTarget).after(300).repeat(4, 150));
        steps(&mut c, 60);
        serde_json::to_string(c.events()).unwrap()
    };
    assert_eq!(run(), run());
}
