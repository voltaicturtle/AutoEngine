use std::sync::Arc;

use autoengine::combat::{Combat, CombatConfig, EventKind, SpawnError, Team, UnitId};
use autoengine::effect::{
    Ability, Amount, DamageKind, Effect, EffectSpec, Passive, Selector, StatusKind, Trigger,
    UnitTemplate,
};
use autoengine::hex::{Hex, HexBoard};
use autoengine::stats::{Stat, StatBlock, StatMod};

fn at(col: i32, row: i32) -> Hex {
    Hex::from_offset(col, row)
}

/// A plain unit with no crits, so tests are exact.
fn grunt(name: &str) -> UnitTemplate {
    UnitTemplate::new(name, StatBlock::base().with(Stat::CritChance, 0))
}

fn duel(a: UnitTemplate, b: UnitTemplate, seed: u64) -> (Combat, UnitId, UnitId) {
    let mut c = Combat::new(HexBoard::new(7, 8), seed);
    let ia = c.spawn(a, Team(0), at(3, 3)).unwrap();
    let ib = c.spawn(b, Team(1), at(3, 4)).unwrap();
    (c, ia, ib)
}

fn count(c: &Combat, f: impl Fn(&EventKind) -> bool) -> usize {
    c.events().iter().filter(|e| f(&e.kind)).count()
}

#[test]
fn same_seed_gives_identical_fight() {
    let run = |seed| {
        let mut c = Combat::new(HexBoard::new(7, 8), seed);
        let t = Arc::new(UnitTemplate::new("Brawler", StatBlock::base()));
        let archer = Arc::new(UnitTemplate::new("Archer", StatBlock::base().with(Stat::Range, 4)));
        for col in 0..4 {
            c.spawn(t.clone(), Team(0), at(col, 0)).unwrap();
            c.spawn(t.clone(), Team(1), at(6 - col, 7)).unwrap();
        }
        c.spawn(archer.clone(), Team(0), at(5, 0)).unwrap();
        c.spawn(archer, Team(1), at(1, 7)).unwrap();
        c.run();
        c.events().to_vec()
    };
    let a = run(1234);
    assert_eq!(a, run(1234));
    assert!(a.len() > 50);
    // Different seeds change crit rolls, so the logs diverge.
    assert_ne!(a, run(4321));
}

#[test]
fn stronger_unit_wins_and_outcome_is_reported() {
    let strong = UnitTemplate::new("Strong", StatBlock::base().with(Stat::MaxHealth, 2000).with(Stat::CritChance, 0));
    let (mut c, a, b) = duel(strong, grunt("Weak"), 1);
    let outcome = c.run().clone();
    assert_eq!(outcome.winner, Some(Team(0)));
    assert!(!outcome.timed_out);
    assert_eq!(outcome.survivors, vec![a]);
    assert!(!c.unit(b).is_alive());
    assert!(matches!(c.events().last().unwrap().kind, EventKind::Ended { winner: Some(Team(0)), .. }));
    // Stepping after the end changes nothing.
    let n = c.events().len();
    c.step();
    assert_eq!(c.events().len(), n);
}

#[test]
fn attack_timing_and_armor_follow_the_formulas() {
    // 1.0 attacks/s with 50ms ticks: one attack every 20 ticks.
    let attacker = UnitTemplate::new(
        "A",
        StatBlock::base().with(Stat::AttackSpeed, 1000).with(Stat::AttackDamage, 100).with(Stat::CritChance, 0),
    );
    let dummy = UnitTemplate::new(
        "Dummy",
        StatBlock::base().with(Stat::MaxHealth, 10_000).with(Stat::AttackDamage, 0).with(Stat::Armor, 100),
    );
    let (mut c, a, b) = duel(attacker, dummy, 1);
    for _ in 0..100 {
        c.step();
    }
    assert_eq!(count(&c, |e| matches!(e, EventKind::Attacked { source, .. } if *source == a)), 5);
    // 100 armor halves physical damage.
    assert_eq!(c.unit(b).health(), 10_000 - 5 * 50);
}

#[test]
fn units_walk_into_range_without_sharing_hexes() {
    let mut c = Combat::new(HexBoard::new(7, 8), 5);
    let t = Arc::new(grunt("Walker"));
    for col in 0..7 {
        c.spawn(t.clone(), Team(0), at(col, 0)).unwrap();
        c.spawn(t.clone(), Team(1), at(col, 7)).unwrap();
    }
    assert_eq!(c.spawn(t.clone(), Team(0), at(0, 0)), Err(SpawnError::Occupied(at(0, 0))));
    assert_eq!(c.spawn(t.clone(), Team(0), at(9, 0)), Err(SpawnError::OffBoard(at(9, 0))));
    while c.outcome().is_none() {
        c.step();
        let mut seen = std::collections::HashSet::new();
        for u in c.units().iter().filter(|u| u.is_alive()) {
            assert!(c.board().contains(u.pos()));
            assert!(seen.insert(u.pos()), "two units share a hex");
            assert_eq!(c.unit_at(u.pos()), Some(u.id()));
        }
    }
    assert!(count(&c, |e| matches!(e, EventKind::Moved { .. })) > 0);
    assert!(c.outcome().unwrap().winner.is_some() || c.outcome().unwrap().survivors.is_empty());
}

#[test]
fn ability_casts_when_mana_fills_and_scales_with_power() {
    let caster = UnitTemplate::new(
        "Caster",
        StatBlock::base()
            .with(Stat::AttackDamage, 0)
            .with(Stat::MaxMana, 30)
            .with(Stat::AttackSpeed, 1000)
            .with(Stat::AbilityPower, 150),
    )
    .with_ability(Ability {
        name: "Bolt".into(),
        cast_time_ms: 200,
        effects: vec![EffectSpec::new(
            Selector::CurrentTarget,
            Effect::Damage { amount: Amount::power(100), kind: DamageKind::True, can_crit: false },
        )],
    });
    let dummy = UnitTemplate::new(
        "Dummy",
        StatBlock::base().with(Stat::MaxHealth, 10_000).with(Stat::AttackDamage, 0),
    );
    let (mut c, a, b) = duel(caster, dummy, 1);
    // Three attacks (10 mana each) at ticks 20, 40, 60; cast starts on 61 and lands 4 ticks later.
    for _ in 0..64 {
        c.step();
    }
    assert!(c.unit(a).is_casting());
    assert_eq!(c.unit(b).health(), 10_000);
    c.step();
    assert!(!c.unit(a).is_casting());
    assert_eq!(c.unit(b).health(), 10_000 - 150);
    assert_eq!(c.unit(a).mana(), 0);
    assert_eq!(count(&c, |e| matches!(e, EventKind::CastFinished { .. })), 1);
}

#[test]
fn stun_stops_actions_and_expires() {
    let stunner = grunt("Stunner").with_passive(Passive {
        name: "Opening Stun".into(),
        trigger: Trigger::CombatStart,
        effects: vec![EffectSpec::new(
            Selector::AllEnemies,
            Effect::ApplyStatus { status: StatusKind::Stun, duration_ms: 2000 },
        )],
        max_procs: None,
    });
    let (mut c, a, b) = duel(stunner, grunt("Victim"), 1);
    for _ in 0..39 {
        c.step();
    }
    assert!(c.unit(b).is_stunned());
    assert_eq!(c.unit(a).health(), c.unit(a).stat(Stat::MaxHealth));
    c.step();
    assert!(!c.unit(b).is_stunned());
    c.run();
    assert_eq!(c.outcome().unwrap().winner, Some(Team(0)));
}

#[test]
fn reactive_passives_fire_without_looping() {
    let thorns = |name: &str| {
        UnitTemplate::new(name, StatBlock::base().with(Stat::CritChance, 0).with(Stat::MaxHealth, 5000)).with_passive(Passive {
            name: "Thorns".into(),
            trigger: Trigger::OnDamaged,
            effects: vec![EffectSpec::new(
                Selector::EventOther,
                Effect::Damage { amount: Amount::flat(10), kind: DamageKind::True, can_crit: false },
            )],
            max_procs: None,
        })
    };
    let (mut c, _, _) = duel(thorns("A"), thorns("B"), 1);
    for _ in 0..200 {
        c.step();
    }
    let attacks = count(&c, |e| matches!(e, EventKind::Attacked { .. }));
    let procs = count(&c, |e| matches!(e, EventKind::PassiveTriggered { .. }));
    assert!(attacks > 0);
    // Exactly one reflection per attack: reflected damage does not reflect again.
    assert_eq!(procs, attacks);
}

#[test]
fn shields_absorb_before_health_and_buffs_revert() {
    let guarded = grunt("Guarded").with_passive(Passive {
        name: "Prepared".into(),
        trigger: Trigger::CombatStart,
        effects: vec![
            EffectSpec::new(Selector::Source, Effect::Shield { amount: Amount::flat(120), duration_ms: None }),
            EffectSpec::new(
                Selector::Source,
                Effect::ApplyStatus {
                    status: StatusKind::StatMods(vec![StatMod::flat(Stat::Armor, 100)]),
                    duration_ms: 1500,
                },
            ),
        ],
        max_procs: None,
    });
    let attacker = UnitTemplate::new(
        "Hitter",
        StatBlock::base().with(Stat::CritChance, 0).with(Stat::AttackSpeed, 1000).with(Stat::AttackDamage, 100),
    );
    let (mut c, a, _) = duel(guarded, attacker, 1);
    c.step();
    assert_eq!(c.unit(a).shield(), 120);
    assert_eq!(c.unit(a).stat(Stat::Armor), 100);
    for _ in 0..19 {
        c.step();
    }
    // First hit: 100 damage halved by the armor buff, fully shielded.
    assert_eq!(c.unit(a).shield(), 70);
    assert_eq!(c.unit(a).health(), 500);
    assert_eq!(c.unit(a).stat(Stat::Armor), 100);
    for _ in 0..20 {
        c.step();
    }
    assert_eq!(c.unit(a).stat(Stat::Armor), 0, "buff expired after 1.5s");
    // Second hit: 100 damage, 70 shielded, 30 to health.
    assert_eq!(c.unit(a).shield(), 0);
    assert_eq!(c.unit(a).health(), 470);
}

#[test]
fn death_trigger_summons_units_that_keep_fighting() {
    let minion = Arc::new(grunt("Minion"));
    let host = UnitTemplate::new("Host", StatBlock::base().with(Stat::MaxHealth, 100).with(Stat::CritChance, 0))
        .with_passive(Passive {
            name: "Burst".into(),
            trigger: Trigger::OnDeath,
            effects: vec![EffectSpec::new(Selector::Source, Effect::Summon { template: minion, count: 3 })],
            max_procs: None,
        });
    let (mut c, a, _) = duel(host, grunt("Enemy"), 1);
    c.run();
    assert!(!c.unit(a).is_alive());
    assert_eq!(c.units().len(), 5);
    assert!(c.units()[2..].iter().all(|u| u.team() == Team(0) && u.name() == "Minion"));
    // Three fresh minions beat one already-damaged enemy.
    assert_eq!(c.outcome().unwrap().winner, Some(Team(0)));
}

#[test]
fn health_threshold_fires_once_and_heal_respects_max() {
    let survivor = UnitTemplate::new("Survivor", StatBlock::base().with(Stat::CritChance, 0).with(Stat::MaxHealth, 1000))
        .with_passive(Passive {
            name: "Second Wind".into(),
            trigger: Trigger::HealthBelow { permille: 500 },
            effects: vec![EffectSpec::new(Selector::Source, Effect::Heal { amount: Amount::flat(5000) })],
            max_procs: None,
        });
    let enemy = UnitTemplate::new("Enemy", StatBlock::base().with(Stat::CritChance, 0).with(Stat::MaxHealth, 4000));
    let (mut c, a, _) = duel(survivor, enemy, 1);
    c.run();
    assert_eq!(count(&c, |e| matches!(e, EventKind::PassiveTriggered { unit, .. } if *unit == a)), 1);
    let heals: Vec<i64> = c
        .events()
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::Healed { amount, .. } => Some(amount),
            _ => None,
        })
        .collect();
    assert_eq!(heals.len(), 1);
    assert!(heals[0] >= 500 && heals[0] < 1000, "heal is capped at max health");
}

#[test]
fn stalemate_times_out_as_a_draw() {
    let pacifist = || UnitTemplate::new("Pacifist", StatBlock::base().with(Stat::AttackDamage, 0));
    let config = CombatConfig { max_duration_ms: 5000, ..CombatConfig::default() };
    let mut c = Combat::with_config(HexBoard::new(7, 8), 1, config);
    c.spawn(pacifist(), Team(0), at(0, 0)).unwrap();
    c.spawn(pacifist(), Team(1), at(6, 7)).unwrap();
    let outcome = c.run().clone();
    assert_eq!(outcome.winner, None);
    assert!(outcome.timed_out);
    assert_eq!(outcome.duration_ms, 5000);
    assert_eq!(outcome.survivors.len(), 2);
}

#[test]
fn three_way_fight_has_one_winner() {
    let mut c = Combat::new(HexBoard::new(9, 9), 77);
    let t = Arc::new(UnitTemplate::new("Brawler", StatBlock::base()));
    c.spawn(t.clone(), Team(0), at(0, 0)).unwrap();
    c.spawn(t.clone(), Team(1), at(8, 0)).unwrap();
    c.spawn(t.clone(), Team(2), at(4, 8)).unwrap();
    let outcome = c.run();
    assert!(outcome.winner.is_some());
    assert_eq!(outcome.survivors.len(), 1);
}
