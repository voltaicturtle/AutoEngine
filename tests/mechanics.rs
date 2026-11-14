//! Conditions, stacks, tag scaling, movement effects, per-member trait
//! bonuses and custom Rust hooks.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use autoengine::combat::{Combat, EventKind, Team, UnitId};
use autoengine::content::{BoardUnit, Bonus, ContentError, ContentSet, TeamSetup, TraitDef, UnitDef};
use autoengine::effect::{
    Amount, Condition, DamageKind, Effect, Passive, ScaleBy, Selector, Trigger, UnitTemplate, Who,
};
use autoengine::hex::{Hex, HexBoard};
use autoengine::stats::{Stat, StatBlock};

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
fn dummy() -> UnitTemplate {
    UnitTemplate::new("Dummy", StatBlock::base().with(Stat::MaxHealth, 100_000).with(Stat::AttackDamage, 0))
}

fn duel(a: UnitTemplate, b: UnitTemplate) -> (Combat, UnitId, UnitId) {
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    let ia = c.spawn(a, Team(0), at(3, 3)).unwrap();
    let ib = c.spawn(b, Team(1), at(3, 4)).unwrap();
    (c, ia, ib)
}

fn steps(c: &mut Combat, n: u32) {
    for _ in 0..n {
        c.step();
    }
}

fn damage_amounts(c: &Combat, kind: DamageKind) -> Vec<i64> {
    c.events()
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::Damaged { amount, kind: k, .. } if k == kind => Some(amount),
            _ => None,
        })
        .collect()
}

#[test]
fn passive_condition_waits_without_using_a_proc() {
    let execute = Passive::new("Execute", Trigger::OnAttack)
        .max_procs(1)
        .when(Condition::HealthBelow { who: Who::Target, permille: 500 })
        .effect(Effect::true_damage(1).to(Selector::EventOther));
    let target = UnitTemplate::new("Target", StatBlock::base().with(Stat::MaxHealth, 1000).with(Stat::AttackDamage, 0));
    let (mut c, _, _) = duel(hitter("Hitter").with_passive(execute), target);
    c.run();

    let mut physical_before = 0;
    let mut fired = 0;
    for e in c.events() {
        match e.kind {
            EventKind::Damaged { amount, kind: DamageKind::Physical, .. } if fired == 0 => physical_before += amount,
            EventKind::PassiveTriggered { .. } => fired += 1,
            _ => {}
        }
    }
    assert_eq!(fired, 1, "the single proc is only spent once the condition holds");
    assert!(physical_before >= 500, "it fired at half health, after {physical_before} damage");
}

#[test]
fn effect_conditions_are_checked_per_target() {
    let bonus_on_stunned = Passive::new("Opportunist", Trigger::OnAttack)
        .effect(Effect::true_damage(7).to(Selector::EventOther).when(Condition::Stunned(Who::Target)));

    let (mut c, _, _) = duel(hitter("Hitter").with_passive(bonus_on_stunned.clone()), dummy());
    steps(&mut c, 100);
    assert!(damage_amounts(&c, DamageKind::True).is_empty());

    let stun_all = Passive::new("Freeze", Trigger::CombatStart).effect(Effect::stun(60_000).to(Selector::AllEnemies));
    let (mut c, _, _) = duel(hitter("Hitter").with_passive(bonus_on_stunned).with_passive(stun_all), dummy());
    steps(&mut c, 100);
    assert!(damage_amounts(&c, DamageKind::True).len() >= 4);
}

#[test]
fn conditions_combine_and_chance_is_seeded() {
    let c = Condition::Stunned(Who::Target).and(Condition::Shielded(Who::Source)).and(Condition::chance(10));
    assert!(matches!(&c, Condition::All(v) if v.len() == 3));
    assert!(matches!(Condition::Stunned(Who::Source).not(), Condition::Not(_)));

    let run = |seed| {
        let lucky = Passive::new("Lucky", Trigger::OnAttack)
            .when(Condition::chance(500))
            .effect(Effect::add_stacks("luck", 1).to(Selector::Source));
        let mut c = Combat::new(HexBoard::new(7, 8), seed);
        let a = c.spawn(hitter("Hitter").with_passive(lucky), Team(0), at(3, 3)).unwrap();
        c.spawn(dummy(), Team(1), at(3, 4)).unwrap();
        steps(&mut c, 2000); // 100 attacks
        c.unit(a).stacks("luck")
    };
    let luck = run(5);
    assert_eq!(luck, run(5));
    assert!((20..80).contains(&luck), "about half of 100 attacks, got {luck}");
}

#[test]
fn stacks_grant_stats_up_to_a_cap_and_clear() {
    let frenzy = Passive::new("Frenzy", Trigger::OnAttack)
        .effect(Effect::add_stacks("frenzy", 1).capped_at(3).per_stack([Stat::AttackDamage.flat(10)]).to(Selector::Source));
    let (mut c, a, _) = duel(hitter("Hitter").with_passive(frenzy.clone()), dummy());
    steps(&mut c, 200); // 10 attacks
    assert_eq!(c.unit(a).stacks("frenzy"), 3);
    assert_eq!(c.unit(a).stat(Stat::AttackDamage), 130);
    assert_eq!(damage_amounts(&c, DamageKind::Physical)[..5], [100, 110, 120, 130, 130]);

    // Releasing at three stacks resets them and the stats they gave.
    let release = Passive::new("Release", Trigger::OnAttack)
        .when(Condition::StacksAtLeast { who: Who::Source, stack: "frenzy".into(), count: 3 })
        .effect(Effect::clear_stacks("frenzy").to(Selector::Source));
    let (mut c, a, _) = duel(hitter("Hitter").with_passive(frenzy).with_passive(release), dummy());
    steps(&mut c, 200);
    let counts: Vec<u32> = c
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::StacksChanged { count, .. } => Some(*count),
            _ => None,
        })
        .collect();
    assert_eq!(counts[..5], [1, 2, 3, 0, 1]);
    assert_eq!(c.unit(a).stat(Stat::AttackDamage), 100 + 10 * c.unit(a).stacks("frenzy") as i64);
}

#[test]
fn amounts_scale_with_stacks_and_tagged_allies() {
    let charge = Passive::new("Charge", Trigger::CombatStart).effect(Effect::add_stacks("charge", 4).to(Selector::Source));
    let release = Passive::new("Discharge", Trigger::CombatStart).effect(
        Effect::true_damage(Amount::flat(0).plus(Who::Source, ScaleBy::Stacks("charge".into()), 25_000))
            .to(Selector::CurrentTarget),
    );
    let (mut c, _, _) = duel(hitter("Hitter").with_passive(charge).with_passive(release), dummy());
    c.step();
    assert_eq!(damage_amounts(&c, DamageKind::True), [100]);

    let howl = Passive::new("Howl", Trigger::CombatStart)
        .effect(
            Effect::true_damage(Amount::flat(0).plus(Who::Source, ScaleBy::AlliesWithTag("beast".into()), 10_000))
                .to(Selector::CurrentTarget),
        )
        .effect(
            Effect::true_damage(1000)
                .to(Selector::CurrentTarget)
                .when(Condition::AlliesWithTagAtLeast { tag: "beast".into(), count: 4 }),
        );
    let beast = || hitter("Beast").with_tag("beast");
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    c.spawn(beast().with_passive(howl), Team(0), at(3, 3)).unwrap();
    c.spawn(beast(), Team(0), at(0, 0)).unwrap();
    c.spawn(beast(), Team(0), at(1, 0)).unwrap();
    c.spawn(hitter("Not a beast"), Team(0), at(2, 0)).unwrap();
    c.spawn(dummy(), Team(1), at(3, 4)).unwrap();
    c.step();
    assert_eq!(damage_amounts(&c, DamageKind::True), [30], "three beasts, and fewer than four");
}

#[test]
fn knockback_pushes_directly_away() {
    let shove = Passive::new("Shove", Trigger::CombatStart).effect(Effect::knockback(2).to(Selector::NearestEnemies(1)));
    let (mut c, a, b) = duel(hitter("Shover").with_passive(shove), dummy());
    c.step();
    assert_eq!(c.unit(a).pos().distance(c.unit(b).pos()), 3);
    assert_eq!(c.unit_at(at(3, 4)), None);
    assert_eq!(c.unit_at(c.unit(b).pos()), Some(b));
    assert!(c.events().iter().any(|e| matches!(e.kind, EventKind::Displaced { unit, .. } if unit == b)));

    // Against the board edge there is nowhere to go.
    let shove = Passive::new("Shove", Trigger::CombatStart).effect(Effect::knockback(3).to(Selector::NearestEnemies(1)));
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    c.spawn(hitter("Shover").with_passive(shove), Team(0), at(3, 6)).unwrap();
    let b = c.spawn(dummy(), Team(1), at(3, 7)).unwrap();
    c.step();
    assert_eq!(c.unit(b).pos(), at(3, 7));
}

#[test]
fn leap_lands_next_to_the_target() {
    let pounce = Passive::new("Pounce", Trigger::CombatStart).effect(Effect::leap().to(Selector::FarthestEnemies(1)));
    let mut c = Combat::new(HexBoard::new(7, 8), 1);
    let a = c.spawn(hitter("Pouncer").with_passive(pounce), Team(0), at(3, 0)).unwrap();
    c.spawn(dummy(), Team(1), at(3, 1)).unwrap();
    let far = c.spawn(dummy(), Team(1), at(5, 7)).unwrap();
    c.step();
    assert_eq!(c.unit(a).pos().distance(c.unit(far).pos()), 1);
    assert_eq!(c.unit(a).target(), Some(far), "it fights what it leapt at");
}

#[test]
fn per_member_bonuses_scale_with_trait_count() {
    let c = ContentSet::builder()
        .trait_def(TraitDef::new("pack", "Pack").breakpoint(2, [Bonus::members().mods_per_member([Stat::Armor.flat(5)])]))
        .units(["a", "b", "c"].map(|id| UnitDef::new(id, id).traits(["pack"])))
        .unit(UnitDef::new("loner", "Loner"))
        .build()
        .unwrap();
    let team = |ids: &[&str]| {
        let mut t = TeamSetup::new()
            .bonus(Bonus::team().mods_per_member([Stat::MagicResist.flat(1)]))
            .bonus(Bonus::with_trait("pack").mods_per_member([Stat::AbilityPower.flat(2)]));
        for (i, id) in ids.iter().enumerate() {
            t = t.unit(BoardUnit::new(*id, (i as i32, 4)));
        }
        c.build_team(&t).unwrap()
    };
    let built = team(&["a", "b", "c", "c", "loner"]);
    let stat = |i: usize, s: Stat| built[i].template.stats.get(s);
    assert_eq!(stat(0, Stat::Armor), 15, "three distinct Pack units");
    assert_eq!(stat(4, Stat::Armor), 0);
    assert_eq!(stat(4, Stat::MagicResist), 4, "four distinct units on the board");
    assert_eq!(stat(0, Stat::AbilityPower), 106);
    assert_eq!(stat(4, Stat::AbilityPower), 100);
    assert_eq!(team(&["a"])[0].template.stats.get(Stat::Armor), 0, "below the breakpoint");
}

#[test]
fn custom_hooks_run_from_content() {
    let game = ContentSet::builder()
        // Steals `arg0` health from the target.
        .custom_effect("siphon", |ctx| {
            let amount = ctx.arg(0);
            ctx.apply(ctx.target(), &Effect::true_damage(amount));
            ctx.apply(ctx.source(), &Effect::heal(amount));
        })
        // Swaps places with the target.
        .custom_effect("swap", |ctx| {
            let (me, them) = (ctx.source(), ctx.target());
            let (mine, theirs) = (ctx.combat().unit(me).pos(), ctx.combat().unit(them).pos());
            // Park one unit on a free hex first so both moves are legal.
            let parking = ctx.combat().board().hexes().find(|h| ctx.combat().unit_at(*h).is_none()).unwrap();
            assert!(ctx.move_unit(me, parking));
            assert!(ctx.move_unit(them, mine));
            assert!(ctx.move_unit(me, theirs));
        })
        .custom_condition("healthier", |ctx| {
            let c = ctx.combat();
            ctx.target().is_some_and(|t| c.unit(t).health() > c.unit(ctx.source()).health())
        })
        .unit(
            UnitDef::new("vampire", "Vampire")
                .stat(Stat::MaxHealth, 1000)
                .stat(Stat::AttackDamage, 0)
                .passive(
                    Passive::new("Siphon", Trigger::OnAttack)
                        .when(Condition::custom("healthier"))
                        .effect(Effect::custom_with("siphon", [50]).to(Selector::EventOther)),
                )
                .passive(Passive::new("Swap", Trigger::CombatStart).effect(Effect::custom("swap").to(Selector::CurrentTarget))),
        )
        .unit(UnitDef::new("tank", "Tank").stat(Stat::MaxHealth, 5000).stat(Stat::AttackDamage, 0))
        .build()
        .unwrap();

    let teams = [
        TeamSetup::new().unit(BoardUnit::new("vampire", (3, 4))),
        TeamSetup::new().unit(BoardUnit::new("tank", (3, 4))),
    ];
    let mut c = game.setup_combat(1, &teams).unwrap();
    let (vampire, tank) = (UnitId(0), UnitId(1));
    let (start_v, start_t) = (c.unit(vampire).pos(), c.unit(tank).pos());
    c.step();
    assert_eq!((c.unit(vampire).pos(), c.unit(tank).pos()), (start_t, start_v), "swapped");

    steps(&mut c, 400);
    let siphons = damage_amounts(&c, DamageKind::True);
    assert!(!siphons.is_empty() && siphons.iter().all(|a| *a == 50));
    assert!(c.unit(tank).health() < 5000);
}

#[test]
fn custom_effects_cannot_recurse_forever() {
    let calls = Arc::new(AtomicU32::new(0));
    let counter = calls.clone();
    let game = ContentSet::builder()
        .custom_effect("echo", move |ctx| {
            counter.fetch_add(1, Ordering::Relaxed);
            ctx.apply(ctx.target(), &Effect::custom("echo"));
        })
        .unit(UnitDef::new("x", "X").passive(Passive::new("Echo", Trigger::CombatStart).effect(Effect::custom("echo").to(Selector::Source))))
        .build()
        .unwrap();
    let mut c = game.setup_combat(1, &[TeamSetup::new().unit(BoardUnit::new("x", (0, 4)))]).unwrap();
    c.step();
    assert_eq!(calls.load(Ordering::Relaxed), 16);
}

#[test]
fn unregistered_hooks_and_bad_stacks_are_rejected() {
    let with = |p: Passive| ContentSet::builder().unit(UnitDef::new("x", "X").passive(p)).build();
    let err = |r: Result<ContentSet, ContentError>| r.unwrap_err().to_string();

    let p = Passive::new("", Trigger::CombatStart).effect(Effect::custom("nope").to(Selector::Source));
    assert!(err(with(p)).contains("unregistered custom effect `nope`"));
    let p = Passive::new("", Trigger::CombatStart)
        .when(Condition::chance(5).and(Condition::custom("hidden").not()))
        .effect(Effect::mana(1).to(Selector::Source));
    assert!(err(with(p)).contains("unregistered custom condition `hidden`"));
    let p = Passive::new("", Trigger::CombatStart)
        .effect(Effect::mana(1).to(Selector::Source).when(Condition::custom("inner")));
    assert!(err(with(p)).contains("`inner`"));
    let p = Passive::new("", Trigger::CombatStart).effect(Effect::add_stacks("", 1).to(Selector::Source));
    assert!(err(with(p)).contains("empty name"));
}
