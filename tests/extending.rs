//! The parts of a fight a game can add to: damage types, signals, marks,
//! selectors and targeting.

use autoengine::combat::{Combat, CombatConfig, EventKind, Team, UnitId};
use autoengine::content::{ContentSet, UnitDef};
use autoengine::damage::DamageType;
use autoengine::effect::{
    Ability, Condition, DamageKind, Effect, Passive, Selector, StatusKind, Targeting, Trigger, UnitTemplate, Who,
};
use autoengine::hex::{Hex, HexBoard};
use autoengine::stats::{Stat, StatBlock};

const FIRE: DamageKind = DamageKind::custom("fire");
const FIRE_RESIST: Stat = Stat::custom("fire_resist");
const FIRE_POWER: Stat = Stat::custom("fire_power");
const VENOM: DamageKind = DamageKind::custom("venom");

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

fn fight(config: CombatConfig) -> Combat {
    Combat::with_config(HexBoard::new(7, 8), 1, CombatConfig { overtime_after_ms: 0, ..config })
}

fn duel(config: CombatConfig, a: UnitTemplate, b: UnitTemplate) -> (Combat, UnitId, UnitId) {
    let mut c = fight(config);
    let ia = c.spawn(a, Team(0), at(3, 3)).unwrap();
    let ib = c.spawn(b, Team(1), at(3, 4)).unwrap();
    (c, ia, ib)
}

fn steps(c: &mut Combat, n: u32) {
    for _ in 0..n {
        c.step();
    }
}

fn opener(effect: Effect, to: Selector) -> Passive {
    Passive::new("Opener", Trigger::CombatStart).effect(effect.to(to))
}

fn lost(c: &Combat, id: UnitId) -> i64 {
    c.unit(id).stat(Stat::MaxHealth) - c.unit(id).health()
}

#[test]
fn a_game_can_define_damage_types() {
    let mut config = CombatConfig::default();
    config.damage_types.push(DamageType::new(FIRE, "Fire").resisted_by(FIRE_RESIST).amplified_by(FIRE_POWER));
    config.damage_types.push(DamageType::new(VENOM, "Venom").ignores_shields().ignores_reduction());

    // Fire attacks ignore armor and answer to fire resistance instead:
    // 100 × (1 + 50%) × 100 / (100 + 100) = 75.
    let pyre = hitter("Pyre").with_attack_kind(FIRE).with_stat(FIRE_POWER, 500);
    let wall = dummy("Wall").with_stat(Stat::Armor, 900).with_stat(FIRE_RESIST, 100);
    let (mut c, _, b) = duel(config.clone(), pyre, wall);
    steps(&mut c, 20);
    assert_eq!(lost(&c, b), 75);
    let kinds: Vec<DamageKind> = c
        .events()
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::Damaged { kind, .. } => Some(kind),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, [FIRE]);

    // Venom goes past shields and damage reduction.
    let viper = dummy("Viper").with_passive(opener(Effect::damage(40, VENOM), Selector::AllEnemies));
    let turtle = dummy("Turtle")
        .with_stat(Stat::DamageReduction, 500)
        .with_passive(opener(Effect::shield(1000), Selector::Source));
    let (mut c, _, b) = duel(config, viper, turtle);
    c.step();
    assert_eq!((lost(&c, b), c.unit(b).shield()), (40, 1000));
}

#[test]
fn built_in_damage_types_can_be_given_other_rules() {
    // In this game armor does nothing against physical damage.
    let mut config = CombatConfig::default();
    config.damage_types[0] = DamageType::physical().unresisted();
    let (mut c, _, b) = duel(config, hitter("Hitter"), dummy("Plated").with_stat(Stat::Armor, 300));
    steps(&mut c, 20);
    assert_eq!(lost(&c, b), 100);
}

#[test]
fn signals_let_content_make_its_own_occasions() {
    // The captain's ability calls "rally"; everyone who listens for it
    // shields themselves, naming the captain as the cause.
    let listen = Passive::new("Answer", Trigger::signal("rally")).effect(Effect::shield(50).to(Selector::Source));
    let captain = dummy("Captain")
        .with_stat(Stat::MaxMana, 10)
        .with_stat(Stat::StartingMana, 10)
        .with_ability(Ability::new("Rally").effect(Effect::signal("rally").to(Selector::OtherAllies)));
    let mut c = fight(CombatConfig::default());
    let cap = c.spawn(captain, Team(0), at(3, 3)).unwrap();
    let soldier = c.spawn(dummy("Soldier").with_passive(listen.clone()), Team(0), at(2, 3)).unwrap();
    let deaf = c.spawn(dummy("Deaf"), Team(0), at(4, 3)).unwrap();
    let enemy = c.spawn(dummy("Enemy").with_passive(listen), Team(1), at(3, 4)).unwrap();
    c.step();
    assert_eq!(
        [cap, soldier, deaf, enemy].map(|id| c.unit(id).shield()),
        [0, 50, 0, 0],
        "only allies that listen, and not the caster itself"
    );
}

#[test]
fn passives_can_react_to_dodges_heals_casts_and_enemy_deaths() {
    let count = |name: &'static str, trigger| Passive::new(name, trigger).effect(Effect::add_stacks(name, 1).to(Selector::Source));

    // Dodging: the other unit is the attacker.
    let slippery = dummy("Slippery")
        .with_stat(Stat::Dodge, 1000)
        .with_passive(Passive::new("Riposte", Trigger::OnDodge).effect(Effect::true_damage(7).to(Selector::EventOther)));
    let (mut c, a, _) = duel(CombatConfig::default(), hitter("Hitter"), slippery);
    steps(&mut c, 20);
    assert_eq!(lost(&c, a), 7);

    // A healer's ability heals an ally (who notices), and the ally also
    // notices the cast. Lifesteal and passive heals do not count as being
    // healed, so reactions cannot feed each other.
    let healer = dummy("Healer")
        .with_stat(Stat::MaxMana, 10)
        .with_stat(Stat::StartingMana, 10)
        .with_stat(Stat::ManaPerAttack, 0)
        .with_ability(Ability::new("Mend").effect(Effect::heal(50).to(Selector::OtherAllies)));
    let patient = hitter("Patient")
        .with_stat(Stat::Lifesteal, 500)
        .with_passive(opener(Effect::true_damage(200), Selector::Source))
        .with_passive(count("healed", Trigger::OnHealed))
        .with_passive(count("ally_cast", Trigger::OnAllyCast))
        .with_passive(count("enemy_down", Trigger::OnEnemyDeath));
    let mut c = fight(CombatConfig::default());
    c.spawn(healer, Team(0), at(2, 3)).unwrap();
    let p = c.spawn(patient, Team(0), at(3, 3)).unwrap();
    c.spawn(UnitTemplate::new("Victim", StatBlock::base().with(Stat::MaxHealth, 150).with(Stat::AttackDamage, 0)), Team(1), at(3, 4)).unwrap();
    c.spawn(dummy("Bystander"), Team(1), at(6, 7)).unwrap();
    steps(&mut c, 40);
    let stacks = |name| c.unit(p).stacks(name);
    assert_eq!((stacks("healed"), stacks("ally_cast"), stacks("enemy_down")), (1, 1, 1));
}

#[test]
fn marks_are_named_statuses_that_conditions_can_find() {
    // The first hit marks the target; later hits on a marked target deal
    // bonus damage. Any status can be asked for by name.
    let marked = Condition::HasStatus { who: Who::Target, status: "hunted".into() };
    let hunter = hitter("Hunter")
        .with_passive(Passive::new("Finish", Trigger::OnAttack).when(marked.clone()).effect(Effect::true_damage(50).to(Selector::EventOther)))
        .with_passive(Passive::new("Mark", Trigger::OnAttack).when(marked.not()).effect(Effect::harmful_mark("hunted", 1500).to(Selector::EventOther)));
    let (mut c, _, b) = duel(CombatConfig::default(), hunter, dummy("Prey"));
    steps(&mut c, 20);
    assert_eq!(lost(&c, b), 100);
    assert!(c.unit(b).has_status(|s| s.name() == "hunted"));
    steps(&mut c, 20);
    assert_eq!(lost(&c, b), 250, "the second hit found the mark");
    steps(&mut c, 20);
    assert_eq!(lost(&c, b), 350, "the mark ran out, so the third hit only marked again");

    assert!(StatusKind::Marker { name: "hunted".into(), harmful: true }.is_debuff());
    assert!(!StatusKind::Marker { name: "blessed".into(), harmful: false }.is_debuff());
    let stunned = Condition::HasStatus { who: Who::Target, status: "stunned".into() };
    let opportunist = hitter("Opportunist")
        .with_passive(opener(Effect::stun(5000), Selector::AllEnemies))
        .with_passive(Passive::new("Exploit", Trigger::OnAttack).when(stunned).effect(Effect::true_damage(11).to(Selector::EventOther)));
    let (mut c, _, b) = duel(CombatConfig::default(), opportunist, dummy("Target"));
    steps(&mut c, 20);
    assert_eq!(lost(&c, b), 111);
}

#[test]
fn selectors_and_targeting_work_on_any_stat() {
    const BOUNTY: Stat = Stat::custom("bounty");
    let mut c = fight(CombatConfig::default());
    let sniper = dummy("Sniper")
        .with_stat(Stat::Range, 9)
        .with_stat(Stat::AttackDamage, 10)
        .with_stat(Stat::CritChance, 0)
        .with_targeting(Targeting::HighestStat(BOUNTY))
        .with_passive(opener(Effect::true_damage(5), Selector::LowestStatEnemies { stat: BOUNTY, n: 1 }));
    let s = c.spawn(sniper, Team(0), at(3, 0)).unwrap();
    let near = c.spawn(dummy("Near").with_stat(BOUNTY, 1), Team(1), at(3, 1)).unwrap();
    let rich = c.spawn(dummy("Rich").with_stat(BOUNTY, 9), Team(1), at(6, 7)).unwrap();
    let poor = c.spawn(dummy("Poor"), Team(1), at(0, 7)).unwrap();
    c.step();
    assert_eq!(c.unit(s).target(), Some(rich), "not the nearest: the one worth the most");
    assert_eq!([near, rich, poor].map(|id| lost(&c, id)), [0, 0, 5]);
    assert_eq!(c.targets(s, &Selector::LowestStatAllies { stat: BOUNTY, n: 1 }), [s]);
    assert!(c.targets(s, &Selector::OtherAllies).is_empty());
    // Random picks come from the fight's seed, so they repeat.
    assert_eq!(c.targets(near, &Selector::RandomAllies(2)), c.targets(near, &Selector::RandomAllies(2)));
    assert_eq!(c.targets(near, &Selector::RandomAllies(2)).len(), 2);

    // A unit that prefers the weakest in some stat.
    let mut c = fight(CombatConfig::default());
    let bully = hitter("Bully").with_stat(Stat::Range, 9).with_targeting(Targeting::LowestStat(Stat::MagicResist));
    let b = c.spawn(bully, Team(0), at(3, 0)).unwrap();
    c.spawn(dummy("Warded").with_stat(Stat::MagicResist, 50), Team(1), at(3, 1)).unwrap();
    let bare = c.spawn(dummy("Bare"), Team(1), at(6, 7)).unwrap();
    c.step();
    assert_eq!(c.unit(b).target(), Some(bare));
}

#[test]
fn rust_hooks_can_pick_targets() {
    // Enemies in the caster's column, nearest first.
    let game = ContentSet::builder()
        .custom_selector("my_column", |ctx| {
            let me = ctx.combat().unit(ctx.source());
            let column = me.pos().to_offset().0;
            let mut found: Vec<_> = ctx
                .combat()
                .units()
                .iter()
                .filter(|u| u.team() != me.team() && u.pos().to_offset().0 == column)
                .map(|u| (me.pos().distance(u.pos()), u.id()))
                .collect();
            found.sort();
            found.into_iter().map(|(_, id)| id).collect()
        })
        .unit(
            UnitDef::new("lancer", "Lancer")
                .stats([(Stat::MaxMana, 10), (Stat::StartingMana, 10), (Stat::AttackDamage, 0)])
                .ability(Ability::new("Skewer").effect(Effect::true_damage(90).to(Selector::Custom("my_column".into())))),
        )
        .unit(UnitDef::new("post", "Post").stats([(Stat::MaxHealth, 5000), (Stat::AttackDamage, 0)]))
        .build()
        .unwrap();
    let template = |id| game.base_template(id, 1).unwrap();
    let mut c = game.setup_combat(1, &[]).unwrap();
    let lancer = c.spawn(template("lancer"), Team(0), at(3, 0)).unwrap();
    let front = c.spawn(template("post"), Team(1), at(3, 2)).unwrap();
    let back = c.spawn(template("post"), Team(1), at(3, 6)).unwrap();
    let aside = c.spawn(template("post"), Team(1), at(5, 2)).unwrap();
    assert_eq!(c.targets(lancer, &Selector::Custom("my_column".into())), [front, back]);
    c.step();
    assert_eq!([front, back, aside].map(|id| lost(&c, id)), [90, 90, 0]);
    assert!(c.targets(lancer, &Selector::Custom("nobody_registered_this".into())).is_empty());
}

#[test]
fn content_that_uses_what_the_game_lacks_is_rejected() {
    let error = |builder: autoengine::content::GameBuilder| builder.build().err().map(|e| e.to_string()).unwrap_or_default();
    let x = || UnitDef::new("x", "X");
    let burn = Ability::new("Burn").effect(Effect::damage(10, FIRE).to(Selector::CurrentTarget));

    let undefined = error(ContentSet::builder().unit(x().ability(burn.clone())));
    assert!(undefined.contains("deals `fire` damage, which the game does not describe"), "{undefined}");
    assert!(error(ContentSet::builder().unit(x().attack_kind(FIRE))).contains("`fire` damage"));
    let dot = Ability::new("Smoulder").effect(Effect::damage_over_time(5, FIRE, 2000).to(Selector::CurrentTarget));
    assert!(error(ContentSet::builder().unit(x().ability(dot))).contains("`fire` damage"));
    let fire = || DamageType::new(FIRE, "Fire");
    assert!(error(ContentSet::builder().damage_type(fire()).unit(x().ability(burn))).is_empty());

    let aim = Ability::new("Aim").effect(Effect::physical(10).to(Selector::Custom("nope".into())));
    assert!(error(ContentSet::builder().unit(x().ability(aim))).contains("unregistered custom selector `nope`"));

    // Describing a kind again replaces the first description.
    let game = ContentSet::builder().damage_type(fire()).damage_type(fire().ignores_shields()).build().unwrap();
    assert_eq!(game.damage_types().len(), 4);
    assert!(!game.damage_type(FIRE).unwrap().shieldable);
}

#[test]
fn game_defined_names_survive_being_written_and_read() {
    let ability = Ability::new("Flare")
        .effect(Effect::damage(10, FIRE).to(Selector::CurrentTarget))
        .effect(Effect::signal("flared").to(Selector::AllAllies));
    let passive = Passive::new("Echo", Trigger::signal("flared")).effect(Effect::mark("lit", 1000).to(Selector::Source));
    let text = serde_json::to_string(&(&ability, &passive, DamageType::new(FIRE, "Fire").resisted_by(FIRE_RESIST))).unwrap();
    let back: (Ability, Passive, DamageType) = serde_json::from_str(&text).unwrap();
    assert_eq!(back.0, ability);
    assert_eq!(back.1, passive);
    assert_eq!(back.2.resist, Some(FIRE_RESIST));
    assert_eq!((FIRE.id(), DamageKind::Physical.id()), ("fire", "physical"));
}

#[test]
fn lint_catches_signals_and_statuses_that_lead_nowhere() {
    let wait = |signal| Passive::new("Wait", Trigger::signal(signal)).effect(Effect::shield(10).to(Selector::Source));
    let hunted = Condition::HasStatus { who: Who::Target, status: "huntd".into() };
    let game = ContentSet::builder()
        .unit(
            UnitDef::new("caller", "Caller")
                .stat(Stat::MaxMana, 50)
                .ability(
                    Ability::new("Call")
                        .effect(Effect::signal("rally").to(Selector::AllAllies))
                        .effect(Effect::signal("unheard").to(Selector::AllAllies))
                        .effect(Effect::harmful_mark("hunted", 1000).to(Selector::CurrentTarget)),
                )
                .passive(wait("rally"))
                .passive(wait("raly"))
                .passive(Passive::new("Finish", Trigger::OnAttack).when(hunted).effect(Effect::true_damage(5).to(Selector::EventOther)))
                .passive(
                    Passive::new("Exploit", Trigger::OnAttack)
                        .when(Condition::HasStatus { who: Who::Target, status: "stunned".into() })
                        .effect(Effect::true_damage(5).to(Selector::EventOther)),
                ),
        )
        .build()
        .unwrap();
    let warnings: Vec<String> = autoengine::tools::lint(&game, None).iter().map(|w| w.to_string()).collect();
    let about = |needle: &str| warnings.iter().filter(|w| w.contains(needle)).count();
    assert_eq!(about("waits for the signal \"raly\""), 1, "{warnings:?}");
    assert_eq!(about("sends the signal \"unheard\""), 1, "{warnings:?}");
    assert_eq!(about("asks for the status \"huntd\""), 1, "{warnings:?}");
    assert_eq!(about("signal") + about("status"), 3, "nothing else is flagged: {warnings:?}");
    assert_eq!(game.passives().len(), 4);
    assert_eq!(game.effects().len(), 3 + 4);
}
