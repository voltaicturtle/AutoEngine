# Damage types

Every bit of damage has a kind, and the kind decides how it is mitigated.
The engine ships with three: physical (cut by `Armor`), magic (cut by
`MagicResist`) and true (cut by nothing). Each is described by a
`DamageType`, and a game can describe more of its own or change the three
it starts with.

## A damage type of your own

```rust
use autoengine::content::{ContentSet, ItemDef, UnitDef};
use autoengine::damage::DamageType;
use autoengine::effect::{Ability, DamageKind, Effect, Selector};
use autoengine::stats::Stat::{self, *};

// Declare the kind and the stats that go with it once, as constants.
const FIRE: DamageKind = DamageKind::custom("fire");
const FIRE_RESIST: Stat = Stat::custom("fire_resist");
const FIRE_POWER: Stat = Stat::custom("fire_power");

let content = ContentSet::builder()
    .damage_type(DamageType::new(FIRE, "Fire").resisted_by(FIRE_RESIST).amplified_by(FIRE_POWER).color([255, 110, 40]))
    .unit(
        UnitDef::new("pyre", "Pyre")
            .cost(2)
            .attack_kind(FIRE) // its basic attacks burn
            .stats([(MaxMana, 60), (FIRE_POWER, 200)])
            .ability(Ability::new("Flare").effect(Effect::damage(150, FIRE).to(Selector::CurrentTarget))),
    )
    .item(ItemDef::new("wet_cloak", "Wet Cloak").mods([FIRE_RESIST.flat(60)]))
    .build()
    .expect("the content is valid");
assert_eq!(content.damage_types().len(), 4);
```

Anything that deals damage takes a kind: `Effect::damage(amount, kind)`,
`Effect::damage_over_time(amount, kind, ms)` and a unit's `.attack_kind(kind)`.
`build()` rejects content that deals a kind the game hasn't described.

| Call | What it does |
| --- | --- |
| `DamageKind::custom(id)` | a kind of the game's own. Declare it as a constant |
| `DamageType::new(kind, name)` | rules for a kind: nothing resists it, `DamageReduction` cuts it and shields soak it |
| `.resisted_by(stat)` | the target's stat that cuts it: taken × 100 / (100 + stat) |
| `.unresisted()` | nothing cuts it |
| `.amplified_by(stat)` | a stat of the source that adds to it, in thousandths, on top of `DamageAmp` |
| `.ignores_reduction()` | the target's `DamageReduction` doesn't cut it |
| `.ignores_shields()` | goes straight to health, past any shields |
| `.color([r, g, b])` | the color of its damage numbers in clients |

The stats are usually [stats of your own](resources.md#stats-of-your-own),
which items, traits and statuses can change like any other stat.

## Changing the built-in kinds

`DamageType::physical()`, `DamageType::magic()` and
`DamageType::true_damage()` are the descriptions the engine starts with.
Describing a kind again replaces its rules. In this game shields don't stop
magic:

```rust
use autoengine::content::ContentSet;
use autoengine::damage::DamageType;
use autoengine::effect::DamageKind;

let content = ContentSet::builder().damage_type(DamageType::magic().ignores_shields()).build().unwrap();
assert!(!content.damage_type(DamageKind::Magic).unwrap().shieldable);
```

How a hit is worked out step by step is in
[Fights](fights.md#damage). `Damaged` fight events carry the kind, and
`kind.id()` gives a short name for it (`"physical"`, `"fire"`).
