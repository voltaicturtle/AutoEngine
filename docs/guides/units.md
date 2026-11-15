# Units

A unit is something players buy from the shop and put on their board, or
something that only appears in fights, such as a summon or a neutral
monster. Every unit is a `UnitDef`.

```rust
use autoengine::content::UnitDef;
use autoengine::effect::{Ability, Amount, Effect, Passive, ScaleBy, Selector, Targeting, Trigger, Who};
use autoengine::stats::Stat::*;

let duelist = UnitDef::new("emberlash_duelist", "Emberlash Duelist")
    .cost(3)
    .traits(["cinder"])
    .stats([(MaxHealth, 650), (AttackDamage, 65), (AttackSpeed, 850), (MaxMana, 60)])
    .targeting(Targeting::LowestHealth)
    .ability(
        Ability::new("Searing Flurry")
            .description("Strikes its target for 80 plus 150% attack damage.")
            .cast_time(250)
            .effect(
                Effect::physical(Amount::power(80).plus(Who::Source, ScaleBy::Stat(AttackDamage), 1500))
                    .with_crit()
                    .to(Selector::CurrentTarget),
            ),
    )
    .passive(
        Passive::new("Last Spark", Trigger::HealthBelow { permille: 400 })
            .effect(Effect::modify_stats([AttackDamage.percent(300)]).to(Selector::Source)),
    )
    .model("models/emberlash_duelist.gltf", 100);
assert_eq!(duelist.cost, 3);
```

## `UnitDef` calls

| Call | What it does |
| --- | --- |
| `UnitDef::new(id, name)` | a unit with base stats, no traits and no ability, that appears in shops. Its cost starts at 0, which no shop offers, so set one |
| `.cost(n)` | shop price. It is also the unit's tier: shop odds and pool sizes are given per cost |
| `.traits(["a", "b"])` | the traits it belongs to, by id |
| `.stat(stat, value)` | sets one base stat |
| `.stats([(stat, value), ...])` | sets several base stats |
| `.ability(Ability)` | the ability it casts when its mana is full |
| `.passive(Passive)` | an effect that fires on a trigger. Can be called more than once |
| `.targeting(Targeting)` | which enemy it goes after (see below) |
| `.summon_only()` | keeps it out of shops and the pool, for summons and neutral monsters |
| `.model(file, scale_percent)` | the 3D model clients draw, relative to the assets folder, and its height as a percentage of a standard unit. The engine itself never reads it |

## Stats

A unit starts from `StatBlock::base()`, and `.stat` and `.stats` replace
values. Stats that aren't set keep the base values.

| Stat | Base | Unit | Meaning |
| --- | --- | --- | --- |
| `MaxHealth` | 500 | points | health at the start of a fight |
| `AttackDamage` | 50 | points | physical damage of each basic attack |
| `AbilityPower` | 100 | percent | multiplies power-scaled amounts (`Amount::power`). 150 is 1.5× |
| `Armor` | 0 | points | cuts physical damage: taken × 100 / (100 + armor) |
| `MagicResist` | 0 | points | cuts magic damage in the same way |
| `AttackSpeed` | 700 | thousandths per second | 700 is 0.7 attacks a second |
| `Range` | 1 | hexes | how far away it can attack. 1 is melee |
| `MaxMana` | 0 | points | mana needed to cast. 0 means the unit never casts |
| `StartingMana` | 0 | points | mana at the start of a fight |
| `ManaPerAttack` | 10 | points | mana gained per basic attack |
| `CritChance` | 250 | permille | chance for an attack (or crit-enabled damage) to crit |
| `CritDamage` | 1400 | permille | a crit deals this share of normal damage: 1400 is 140% |
| `MoveSpeed` | 1800 | thousandths of a hex per second | 1800 is 1.8 hexes a second |
| `DamageAmp` | 0 | permille | more damage dealt, of every kind |
| `DamageReduction` | 0 | permille | less physical and magic damage taken, capped at 900 |
| `Lifesteal` | 0 | permille | heals for this share of the damage it deals |
| `Dodge` | 0 | permille | chance for a basic attack against it to miss |
| `Tenacity` | 0 | permille | cuts how long stun, silence, disarm, root and taunt last, at most 800 |

Stats change during a game and a fight through **stat modifiers**:
`Armor.flat(15)` adds 15 and `AttackSpeed.percent(200)` adds 20%. The value
used in a fight is `(base + all flat) × (1000 + all permille) / 1000`, never
below zero. Percentages add together: two +20% modifiers give +40%, not
+44%.

## Star levels

Three copies of a one-star unit merge into a two-star unit, and three
two-stars into a three-star. The number of copies is
`GameRules::copies_to_upgrade`. Each star level adds stat modifiers from
`GameRules::star_levels`. By default two stars gives +80% health and +50%
attack damage and ability power, and three stars gives +224% health and
+125% attack damage and ability power. See [Rules](rules.md).

## Abilities

A unit with `MaxMana` above zero and an ability casts it as soon as its
mana is full. Its mana then goes back to zero. Mana comes from
`StartingMana`, from attacks (`ManaPerAttack`), from taking damage (3% of the
damage, at most 40 per hit) and from effects (`Effect::mana`).

```rust
use autoengine::effect::{Ability, Amount, Anchor, DamageKind, Effect, Selector};

let wave = Selector::EnemiesWithin { anchor: Anchor::CurrentTarget, radius: 1 };
let ability = Ability::new("Breaking Wave")
    .description("180 magic damage to the target and the enemies next to it, then 30 a second for 3 seconds.")
    .cast_time(500)
    .effect(Effect::magic(Amount::power(180)).to(wave.clone()))
    .effect(Effect::damage_over_time(Amount::power(30), DamageKind::Magic, 3000).to(wave));
assert_eq!(ability.effects.len(), 2);
```

| Call | What it does |
| --- | --- |
| `Ability::new(name)` | an ability with no effects |
| `.description(text)` | the tooltip text. The engine doesn't check it against the effects |
| `.cast_time(ms)` | how long the unit stands still casting before the effects happen. A stun pauses the cast rather than cancelling it |
| `.effect(spec)` | one effect and who it hits, made with `Effect::...().to(Selector::...)`. Effects happen in the order they are added |

Everything an ability can do is in [Effects](effects.md).

## Passives

A passive is a set of effects that happens when something triggers it, such
as the start of the fight, an attack, a kill or a timer. Units, trait
bonuses, items and augments all carry passives in the same way.

```rust
use autoengine::effect::{Condition, Effect, Passive, Selector, Trigger, Who};
use autoengine::stats::Stat::*;

let frenzy = Passive::new("Frenzy", Trigger::OnAttack)
    .effect(Effect::add_stacks("frenzy", 1).capped_at(10).per_stack([AttackSpeed.percent(50)]).to(Selector::Source));

let finisher = Passive::new("Finisher", Trigger::OnAttack)
    .when(Condition::StacksAtLeast { who: Who::Source, stack: "frenzy".into(), count: 10 })
    .max_procs(1)
    .effect(Effect::stun(1000).to(Selector::EventOther));
assert!(finisher.condition.is_some() && frenzy.max_procs.is_none());
```

| Call | What it does |
| --- | --- |
| `Passive::new(name, trigger)` | a passive with no effects (triggers are listed in [Effects](effects.md#triggers)) |
| `.effect(spec)` | adds an effect |
| `.when(condition)` | only fires when the condition holds. Several calls combine with "and" |
| `.max_procs(n)` | fires at most `n` times per fight. `Trigger::HealthBelow` passives fire once unless this says otherwise |

## Targeting

When a unit has no target, or its target dies, it picks a new one:

| `Targeting` | Picks |
| --- | --- |
| `Nearest` (default) | the closest enemy |
| `Farthest` | the farthest enemy: assassins that dive the back line |
| `LowestHealth` | the enemy with the least health left |
| `HighestDamage` | the enemy with the most attack damage |
| `LowestArmor` | the enemy with the least armor |

Ties go to the closest enemy. A taunted unit attacks whoever taunted it,
whatever its targeting says.

## Summons and neutral monsters

A summon-only unit never shows up in shops, but effects can summon it and
PvE rounds can field it:

```rust
use autoengine::content::{ContentSet, UnitDef};
use autoengine::effect::{Effect, Passive, Selector, Trigger};
use autoengine::stats::Stat::*;

let content = ContentSet::builder()
    .unit(
        UnitDef::new("lantern_keeper", "Lantern Keeper")
            .cost(2)
            .passive(Passive::new("Release", Trigger::OnDeath).effect(Effect::summon("wisp", 2).to(Selector::Source))),
    )
    .unit(UnitDef::new("wisp", "Wisp").summon_only().stats([(MaxHealth, 200), (AttackDamage, 30)]))
    .build()
    .unwrap();
assert!(!content.unit("wisp").unwrap().in_shop);
```

Summoned units appear on the hexes closest to the unit the effect selected
and start acting on the next tick. A summon can be anchored on a unit that
just died, as above.

## Examples

**A tank that taunts and gets tougher as it is hit:**

```rust
use autoengine::content::UnitDef;
use autoengine::effect::{Ability, Anchor, Effect, Passive, Selector, Trigger};
use autoengine::stats::Stat::*;

let tank = UnitDef::new("stone_sentinel", "Stone Sentinel")
    .cost(3)
    .stats([(MaxHealth, 1100), (Armor, 50), (MagicResist, 40), (MaxMana, 90), (StartingMana, 40)])
    .ability(
        Ability::new("Challenge")
            .description("Taunts nearby enemies for 2 seconds and gains a 300 shield.")
            .effect(Effect::taunt(2000).to(Selector::EnemiesWithin { anchor: Anchor::Source, radius: 2 }))
            .effect(Effect::shield_for(300, 4000).to(Selector::Source)),
    )
    .passive(
        Passive::new("Hardening", Trigger::OnDamaged)
            .max_procs(20)
            .effect(Effect::modify_stats([Armor.flat(2)]).to(Selector::Source)),
    );
assert_eq!(tank.passives.len(), 1);
```

**An assassin that leaps to the back line and finishes off weak targets:**

```rust
use autoengine::content::UnitDef;
use autoengine::effect::{Ability, Effect, Passive, Selector, Targeting, Trigger};
use autoengine::stats::Stat::*;

let assassin = UnitDef::new("night_blade", "Night Blade")
    .cost(4)
    .stats([(MaxHealth, 600), (AttackDamage, 70), (AttackSpeed, 900), (CritChance, 400), (MaxMana, 50)])
    .targeting(Targeting::Farthest)
    .passive(Passive::new("Shadowstep", Trigger::CombatStart).effect(Effect::leap().to(Selector::FarthestEnemies(1))))
    .ability(
        Ability::new("Cull")
            .description("Kills its target if it is below 20% health; otherwise hits it for 200.")
            .effect(Effect::execute(200).to(Selector::CurrentTarget))
            .effect(Effect::physical(200).with_crit().to(Selector::CurrentTarget)),
    );
assert_eq!(assassin.targeting, Targeting::Farthest);
```

**A healer that cleanses:**

```rust
use autoengine::content::UnitDef;
use autoengine::effect::{Ability, Amount, Condition, Effect, ScaleBy, Selector, Who};
use autoengine::stats::Stat::*;

let healer = UnitDef::new("dawn_cleric", "Dawn Cleric")
    .cost(2)
    .stats([(MaxHealth, 500), (Range, 3), (MaxMana, 60)])
    .ability(
        Ability::new("Purify")
            .description("Heals the two weakest allies for 150 plus 15% of their missing health and cleanses them.")
            .effect(
                Effect::heal(Amount::power(150).plus(Who::Target, ScaleBy::MissingHealth, 150))
                    .to(Selector::LowestHealthAllies(2)),
            )
            .effect(Effect::cleanse().to(Selector::LowestHealthAllies(2)).when(Condition::HasDebuff(Who::Target))),
    );
assert!(healer.ability.is_some());
```

**A neutral monster for PvE rounds:**

```rust
use autoengine::content::UnitDef;
use autoengine::stats::Stat::*;

let golem = UnitDef::new("moss_golem", "Moss Golem")
    .summon_only()
    .stats([(MaxHealth, 450), (AttackDamage, 35), (AttackSpeed, 550)]);
assert!(!golem.in_shop);
```

It is used in the schedule with `RoundKindDef::pve([BoardUnit::new("moss_golem", (3, 4))])`
(see [Rounds](rounds.md)).
