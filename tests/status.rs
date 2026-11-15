//! Crowd control and the other statuses: silence, disarm, root, taunt,
//! wound, invulnerable, and cleansing them.

use autoengine::combat::{Combat, EventKind, Team, UnitId};
use autoengine::effect::{Ability, Condition, Effect, Passive, Selector, StatusKind, Trigger, UnitTemplate, Who};
use autoengine::hex::{Hex, HexBoard};
use autoengine::stats::{Stat, StatBlock, StatMod};

fn at(col: i32, row: i32) -> Hex {
    Hex::from_offset(col, row)
}

/// No crits, 100 attack damage, one attack per second.
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

/// Does `effect` to all enemies when the fight starts.
fn opener(effect: Effect) -> Passive {
    Passive::new("Opener", Trigger::CombatStart).effect(effect.to(Selector::AllEnemies))
}

fn run_ms(c: &mut Combat, ms: u32) {
    while c.time_ms() < ms && c.outcome().is_none() {
        c.step();
    }
}

fn count(c: &Combat, f: impl Fn(&EventKind) -> bool) -> usize {
    c.events().iter().filter(|e| f(&e.kind)).count()
}

/// The first time (in ms) an event matching `f` happened.
fn first_ms(c: &Combat, f: impl Fn(&EventKind) -> bool) -> Option<u32> {
    let tick = c.config().tick_ms;
    c.events().iter().find(|e| f(&e.kind)).map(|e| e.tick * tick)
}

#[test]
fn silence_stops_casting_but_not_attacking() {
    let caster = hitter("Caster")
        .with_stat(Stat::MaxMana, 10)
        .with_stat(Stat::StartingMana, 10)
        .with_ability(Ability::new("Bolt").effect(Effect::magic(50).to(Selector::CurrentTarget)));
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    let me = c.spawn(caster, Team(0), at(3, 3)).unwrap();
    c.spawn(dummy("Silencer").with_passive(opener(Effect::silence(2_000))), Team(1), at(3, 4)).unwrap();
    run_ms(&mut c, 4_000);
    let cast = first_ms(&c, |e| matches!(e, EventKind::CastStarted { unit } if *unit == me)).unwrap();
    let attack = first_ms(&c, |e| matches!(e, EventKind::Attacked { source, .. } if *source == me)).unwrap();
    assert!(cast >= 2_000, "cast at {cast} ms");
    assert!(attack < 2_000, "attacked at {attack} ms");
}

#[test]
fn disarm_stops_attacks_and_root_stops_walking() {
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    let me = c.spawn(hitter("Hitter"), Team(0), at(3, 3)).unwrap();
    c.spawn(dummy("Disarmer").with_passive(opener(Effect::disarm(1_500))), Team(1), at(3, 4)).unwrap();
    run_ms(&mut c, 3_000);
    let attack = first_ms(&c, |e| matches!(e, EventKind::Attacked { source, .. } if *source == me)).unwrap();
    assert!(attack >= 1_500, "attacked at {attack} ms");

    // Far apart: the rooted unit cannot close in, then walks once free.
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    let me = c.spawn(hitter("Hitter"), Team(0), at(3, 0)).unwrap();
    let rooter = UnitTemplate::new("Rooter", StatBlock::base().with(Stat::MaxHealth, 100_000).with(Stat::AttackDamage, 0).with(Stat::MoveSpeed, 0));
    c.spawn(rooter.with_passive(opener(Effect::root(1_500))), Team(1), at(3, 7)).unwrap();
    run_ms(&mut c, 1_400);
    assert_eq!(c.unit(me).pos(), at(3, 0));
    assert!(c.unit(me).statuses().any(|(s, _)| *s == StatusKind::Root));
    run_ms(&mut c, 3_000);
    assert_ne!(c.unit(me).pos(), at(3, 0));
}

#[test]
fn taunted_units_attack_the_taunter() {
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    let me = c.spawn(hitter("Hitter").with_stat(Stat::Range, 5), Team(0), at(3, 3)).unwrap();
    let near = c.spawn(dummy("Near"), Team(1), at(3, 4)).unwrap();
    let tank = c.spawn(dummy("Tank").with_passive(opener(Effect::taunt(3_000))), Team(1), at(3, 6)).unwrap();
    run_ms(&mut c, 2_500);
    let hit = |c: &Combat, who: UnitId| count(c, |e| matches!(e, EventKind::Attacked { source, target } if *source == me && *target == who));
    assert!(hit(&c, tank) >= 2);
    assert_eq!(hit(&c, near), 0, "the nearer unit is ignored while the taunt lasts");
}

#[test]
fn wounds_cut_healing_and_the_strongest_applies() {
    let healer = |wounds: &[u32]| {
        let mut target = dummy("Patient").with_stat(Stat::MaxHealth, 10_000);
        target = target.with_passive(Passive::new("Hurt", Trigger::CombatStart).effect(Effect::true_damage(5_000).to(Selector::Source)));
        target = target.with_passive(Passive::new("Mend", Trigger::Every { interval_ms: 1_000 }).effect(Effect::heal(1_000).to(Selector::Source)));
        let mut enemy = dummy("Cutter");
        for (i, w) in wounds.iter().enumerate() {
            enemy = enemy.with_passive(Passive::new(format!("Cut{i}"), Trigger::CombatStart).effect(Effect::wound(*w, 10_000).to(Selector::AllEnemies)));
        }
        let mut c = Combat::new(HexBoard::new(7, 8), 1);
        let me = c.spawn(target, Team(0), at(0, 0)).unwrap();
        c.spawn(enemy, Team(1), at(6, 7)).unwrap();
        run_ms(&mut c, 1_050);
        c.unit(me).health()
    };
    assert_eq!(healer(&[]), 6_000);
    assert_eq!(healer(&[500]), 5_500);
    assert_eq!(healer(&[300, 800, 500]), 5_200);
    assert_eq!(healer(&[2_000]), 5_000, "a wound over 100% just stops healing");
}

#[test]
fn invulnerable_units_take_no_damage() {
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    c.spawn(hitter("Hitter"), Team(0), at(3, 3)).unwrap();
    let guard = dummy("Guard").with_passive(Passive::new("Wall", Trigger::CombatStart).effect(Effect::invulnerable(2_000).to(Selector::Source)));
    let target = c.spawn(guard, Team(1), at(3, 4)).unwrap();
    run_ms(&mut c, 1_900);
    assert_eq!(c.unit(target).health(), 100_000);
    run_ms(&mut c, 3_500);
    assert!(c.unit(target).health() < 100_000);
}

#[test]
fn cleanse_removes_debuffs_and_keeps_buffs() {
    let cleanser = dummy("Monk")
        .with_passive(
            Passive::new("Purify", Trigger::Every { interval_ms: 500 })
                .max_procs(1)
                .when(Condition::HasDebuff(Who::Source))
                .effect(Effect::cleanse().to(Selector::Source)),
        )
        .with_passive(Passive::new("Pray", Trigger::CombatStart).effect(Effect::stat_status([StatMod::flat(Stat::Armor, 50)], 9_000).to(Selector::Source)));
    let curser = dummy("Witch")
        .with_passive(opener(Effect::stun(9_000)))
        .with_passive(opener(Effect::stat_status([StatMod::flat(Stat::Armor, -30)], 9_000)))
        .with_passive(opener(Effect::wound(500, 9_000)));
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    let monk = c.spawn(cleanser, Team(0), at(3, 3)).unwrap();
    c.spawn(curser, Team(1), at(3, 4)).unwrap();
    run_ms(&mut c, 100);
    assert!(c.unit(monk).is_stunned());
    assert_eq!(c.unit(monk).stat(Stat::Armor), 20);
    run_ms(&mut c, 700);
    assert!(!c.unit(monk).is_stunned());
    let left: Vec<&StatusKind> = c.unit(monk).statuses().map(|(s, _)| s).collect();
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(!left[0].is_debuff());
    assert_eq!(c.unit(monk).stat(Stat::Armor), 50, "the curse's armor cut is undone");
    let expired = count(&c, |e| matches!(e, EventKind::StatusExpired { target, .. } if *target == monk));
    assert_eq!(expired, 3);
}

#[test]
fn what_counts_as_a_debuff() {
    assert!(StatusKind::StatMods(vec![StatMod::flat(Stat::Armor, -1)]).is_debuff());
    assert!(!StatusKind::StatMods(vec![StatMod::percent(Stat::AttackSpeed, 200)]).is_debuff());
    assert!(StatusKind::StatMods(vec![StatMod::flat(Stat::MaxMana, 20)]).is_debuff(), "costlier casts hurt");
    assert!(!StatusKind::Invulnerable.is_debuff());
    assert!(StatusKind::Taunt.is_debuff());
}

#[test]
fn playback_shows_statuses_while_they_last() {
    use autoengine::game::{CombatRecord, Opponent, PlayerId};
    use autoengine::playback::Playback;

    let cleanser = dummy("Monk").with_passive(
        Passive::new("Purify", Trigger::Every { interval_ms: 1_000 })
            .max_procs(1)
            .effect(Effect::cleanse().to(Selector::Source)),
    );
    let curser = dummy("Witch").with_passive(opener(Effect::silence(5_000))).with_passive(opener(Effect::root(400)));
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    let monk = c.spawn(cleanser, Team(0), at(3, 3)).unwrap();
    c.spawn(curser, Team(1), at(3, 4)).unwrap();
    run_ms(&mut c, 2_000);
    let record = CombatRecord {
        home: PlayerId(0),
        away: Opponent::Neutral,
        seed: 1,
        tick_ms: c.config().tick_ms,
        winner: None,
        duration_ms: c.time_ms(),
        survivors: 0,
        teams: vec![],
        events: c.events().to_vec(),
    };
    let mut playback = Playback::new(&record);
    playback.advance_to(100);
    assert_eq!(playback.unit(monk).unwrap().statuses_at(100), vec!["silenced", "rooted"]);
    playback.advance_to(500);
    assert_eq!(playback.unit(monk).unwrap().statuses_at(500), vec!["silenced"]);
    // Cleansed at one second, long before the silence would have ended.
    playback.advance_to(1_100);
    assert!(playback.unit(monk).unwrap().statuses_at(1_100).is_empty());
}

#[test]
fn unstoppable_units_shrug_off_new_debuffs() {
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    let rock = dummy("Rock").with_passive(Passive::new("Steel", Trigger::CombatStart).effect(Effect::unstoppable(5_000).to(Selector::Source)));
    let me = c.spawn(rock, Team(0), at(3, 3)).unwrap();
    // Statuses from a passive that fires after the rock's own.
    let witch = dummy("Witch").with_passive(Passive::new("Hex", Trigger::Every { interval_ms: 500 }).max_procs(1).effect(Effect::stun(2_000).to(Selector::AllEnemies)).effect(Effect::stat_status([StatMod::flat(Stat::Armor, 40)], 2_000).to(Selector::AllEnemies)));
    c.spawn(witch, Team(1), at(3, 4)).unwrap();
    run_ms(&mut c, 600);
    assert!(!c.unit(me).is_stunned());
    assert_eq!(c.unit(me).stat(Stat::Armor), 40, "buffs still take hold");
}

#[test]
fn tenacity_shortens_crowd_control_only() {
    let stunned_for = |tenacity: i64| {
        let mut c = Combat::new(HexBoard::new(7, 8), 1);
        let me = c.spawn(dummy("Tough").with_stat(Stat::Tenacity, tenacity), Team(0), at(3, 3)).unwrap();
        c.spawn(dummy("Witch").with_passive(opener(Effect::stun(2_000))).with_passive(opener(Effect::wound(500, 2_000))), Team(1), at(3, 4)).unwrap();
        c.step();
        let stun = c.unit(me).statuses().find(|(s, _)| **s == StatusKind::Stun).map(|(_, ms)| ms).unwrap();
        let wound = c.unit(me).statuses().find(|(s, _)| matches!(s, StatusKind::Wound { .. })).map(|(_, ms)| ms).unwrap();
        (stun, wound)
    };
    let (plain, wound) = stunned_for(0);
    let (tough, wound_tough) = stunned_for(500);
    let (capped, _) = stunned_for(5_000);
    assert!(tough * 2 <= plain + 50, "{tough} vs {plain}");
    assert_eq!(wound, wound_tough, "wounds are not crowd control");
    assert!(capped > 0 && capped * 5 <= plain + 50, "at most 80% off: {capped}");
}

#[test]
fn dodged_attacks_miss() {
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    let me = c.spawn(hitter("Hitter"), Team(0), at(3, 3)).unwrap();
    let target = c.spawn(dummy("Dancer").with_stat(Stat::Dodge, 500), Team(1), at(3, 4)).unwrap();
    run_ms(&mut c, 20_000);
    let attacks = count(&c, |e| matches!(e, EventKind::Attacked { source, .. } if *source == me));
    let dodged = count(&c, |e| matches!(e, EventKind::Dodged { target: t, .. } if *t == target));
    let hits = count(&c, |e| matches!(e, EventKind::Damaged { target: t, .. } if *t == target));
    assert_eq!(attacks, dodged + hits);
    assert!(dodged * 3 > attacks && dodged * 3 < attacks * 2, "about half: {dodged} of {attacks}");
}

#[test]
fn crits_and_broken_shields_trigger_passives() {
    let critter = hitter("Critter")
        .with_stat(Stat::CritChance, 1000)
        .with_passive(Passive::new("Rush", Trigger::OnCrit).effect(Effect::mana(1).to(Selector::Source)));
    let shielded = dummy("Shell")
        .with_passive(Passive::new("Shell", Trigger::CombatStart).effect(Effect::shield(150).to(Selector::Source)))
        .with_passive(Passive::new("Burst", Trigger::OnShieldBroken).effect(Effect::stun(1_000).to(Selector::EventOther)));
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    let me = c.spawn(critter, Team(0), at(3, 3)).unwrap();
    let shell = c.spawn(shielded, Team(1), at(3, 4)).unwrap();
    run_ms(&mut c, 3_000);
    let fired = |unit: UnitId| count(&c, |e| matches!(e, EventKind::PassiveTriggered { unit: u, .. } if *u == unit));
    let attacks = count(&c, |e| matches!(e, EventKind::Damaged { source, .. } if *source == me));
    assert!(attacks >= 1);
    assert!(fired(me) >= attacks, "every hit crit");
    assert_eq!(fired(shell), 2, "the opening shield, then breaking it once");
    assert!(count(&c, |e| matches!(e, EventKind::StatusApplied { target, .. } if *target == me)) == 1);
}

#[test]
fn execute_finishes_low_units_and_pull_drags_them_in() {
    let finisher = |threshold| {
        hitter("Reaper").with_passive(Passive::new("Reap", Trigger::OnAttack).effect(Effect::execute(threshold).to(Selector::EventOther)))
    };
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    c.spawn(finisher(300), Team(0), at(3, 3)).unwrap();
    let victim = c.spawn(dummy("Victim").with_stat(Stat::MaxHealth, 1_000), Team(1), at(3, 4)).unwrap();
    run_ms(&mut c, 30_000);
    // 100 per hit: the hit that leaves it at 300 or below lets the execute finish it.
    let hits = count(&c, |e| matches!(e, EventKind::Attacked { target, .. } if *target == victim));
    assert!(!c.unit(victim).is_alive());
    assert_eq!(hits, 7);

    let puller = dummy("Hook").with_passive(opener(Effect::pull(10)));
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    c.spawn(puller, Team(0), at(3, 0)).unwrap();
    let far = c.spawn(dummy("Far").with_stat(Stat::MoveSpeed, 0), Team(1), at(3, 7)).unwrap();
    c.step();
    assert_eq!(c.unit(far).pos().distance(at(3, 0)), 1);
    assert!(count(&c, |e| matches!(e, EventKind::Displaced { unit, .. } if *unit == far)) == 1);
}

#[test]
fn random_and_highest_stat_selectors() {
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    let me = c.spawn(dummy("Picker"), Team(0), at(3, 0)).unwrap();
    let mut enemies = Vec::new();
    for (i, armor) in [10, 50, 30, 20].into_iter().enumerate() {
        enemies.push(c.spawn(dummy("E").with_stat(Stat::Armor, armor), Team(1), at(i as i32, 7)).unwrap());
    }
    let ally = c.spawn(dummy("Ally").with_stat(Stat::AttackDamage, 999), Team(0), at(5, 0)).unwrap();
    assert_eq!(c.targets(me, &Selector::HighestStatEnemies { stat: Stat::Armor, n: 2 }), vec![enemies[1], enemies[2]]);
    assert_eq!(c.targets(me, &Selector::HighestStatAllies { stat: Stat::AttackDamage, n: 1 }), vec![ally]);
    let picked = c.targets(me, &Selector::RandomEnemies(2));
    assert_eq!(picked.len(), 2);
    assert!(picked.iter().all(|p| enemies.contains(p)) && picked[0] != picked[1]);
    assert_eq!(c.targets(me, &Selector::RandomEnemies(9)).len(), 4);
    assert_eq!(c.targets(me, &Selector::RandomEnemies(2)), picked, "the same moment picks the same");
}

#[test]
fn overtime_ends_stalemates() {
    use autoengine::combat::CombatConfig;
    // Two sturdy units that heal faster than they hurt each other.
    let tank = |name: &str| {
        hitter(name)
            .with_stat(Stat::MaxHealth, 2_000)
            .with_stat(Stat::Armor, 200)
            .with_passive(Passive::new("Mend", Trigger::Every { interval_ms: 1_000 }).effect(Effect::heal(50).to(Selector::Source)))
    };
    let fight = |config: CombatConfig| {
        let mut c = Combat::with_config(HexBoard::new(7, 8), 1, config);
        c.spawn(tank("A"), Team(0), at(3, 3)).unwrap();
        c.spawn(tank("B"), Team(1), at(3, 4)).unwrap();
        let outcome = c.run().clone();
        (outcome, count(&c, |e| matches!(e, EventKind::Overtime)))
    };
    let (calm, signals) = fight(CombatConfig { overtime_after_ms: 0, ..CombatConfig::default() });
    assert!(calm.timed_out && calm.winner.is_none());
    assert_eq!(signals, 0);
    let (hot, signals) = fight(CombatConfig::default());
    assert!(!hot.timed_out && hot.winner.is_some(), "{hot:?}");
    assert!(hot.duration_ms > 30_000 && hot.duration_ms < 60_000);
    assert_eq!(signals, 1);
}

#[test]
fn games_set_their_own_fight_rules() {
    let content = autoengine::sample::content();
    let mut rules = content.rules().clone();
    rules.combat.max_duration_ms = 5_000;
    rules.combat.overtime_after_ms = 0;
    let content = content.with_game_rules(rules.clone()).unwrap();
    let setup = autoengine::sample::sample_match();
    let combat = content.setup_combat(1, &setup.teams).unwrap();
    assert_eq!(combat.config().max_duration_ms, 5_000);
    rules.combat.tick_ms = 0;
    assert!(content.with_game_rules(rules).is_err());
}

#[test]
fn damage_meters_add_up() {
    let content = autoengine::sample::content();
    let setup = autoengine::sample::sample_match();
    let mut combat = content.setup_combat(4, &setup.teams).unwrap();
    combat.run();
    let events = combat.events().to_vec();
    let stats = autoengine::playback::fight_stats(&events);
    assert_eq!(stats.len(), combat.units().len());
    let mut self_damage = 0;
    let mut healing = 0;
    for e in &events {
        match &e.kind {
            EventKind::Damaged { source, target, amount, absorbed, .. } if source == target => self_damage += amount + absorbed,
            EventKind::Healed { amount, .. } => healing += amount,
            _ => {}
        }
    }
    let dealt: i64 = stats.iter().map(|s| s.damage_dealt).sum();
    let taken: i64 = stats.iter().map(|s| s.damage_taken).sum();
    assert_eq!(dealt + self_damage, taken);
    assert!(dealt > 0);
    assert_eq!(stats.iter().map(|s| s.healing_done).sum::<i64>(), healing);
    let deaths = stats.iter().filter(|s| !s.survived).count() as u32;
    assert!(stats.iter().map(|s| s.kills).sum::<u32>() <= deaths);
    for s in &stats {
        assert_eq!(s.survived, combat.unit(s.unit).is_alive());
    }
}

#[test]
fn units_pick_targets_by_their_priority() {
    use autoengine::effect::Targeting;
    let first_target = |targeting: Targeting| {
        let mut c = Combat::new(HexBoard::new(7, 8), 1);
        let me = c.spawn(hitter("Picker").with_stat(Stat::Range, 9).with_targeting(targeting), Team(0), at(3, 0)).unwrap();
        let near = c.spawn(dummy("Near").with_stat(Stat::Armor, 50), Team(1), at(3, 2)).unwrap();
        let weak = c.spawn(dummy("Weak").with_stat(Stat::MaxHealth, 300), Team(1), at(0, 5)).unwrap();
        let brute = c.spawn(dummy("Brute").with_stat(Stat::AttackDamage, 500).with_stat(Stat::Armor, 80), Team(1), at(6, 5)).unwrap();
        let far = c.spawn(dummy("Far").with_stat(Stat::Armor, 30), Team(1), at(3, 7)).unwrap();
        c.step();
        c.step();
        let target = c.unit(me).target().unwrap();
        [("near", near), ("weak", weak), ("brute", brute), ("far", far)].into_iter().find(|(_, id)| *id == target).unwrap().0
    };
    assert_eq!(first_target(Targeting::Nearest), "near");
    assert_eq!(first_target(Targeting::Farthest), "far");
    assert_eq!(first_target(Targeting::LowestHealth), "weak");
    assert_eq!(first_target(Targeting::HighestDamage), "brute");
    // "weak" has the default armor of zero.
    assert_eq!(first_target(Targeting::LowestArmor), "weak");

    // Content sets it per unit.
    let unit = autoengine::content::UnitDef::new("x", "X").targeting(Targeting::Farthest);
    assert_eq!(unit.targeting, Targeting::Farthest);
}
