# Traits

A trait (also called a synergy) rewards fielding several units that share
it. Units list their traits with `UnitDef::traits`, and items can grant a
trait with `ItemDef::grants_trait`.

```rust
use autoengine::content::{Bonus, ContentSet, TraitDef, UnitDef};
use autoengine::effect::{Effect, Passive, Selector, Trigger};
use autoengine::stats::Stat::*;

let grove = TraitDef::new("grove", "Grove")
    .description("Grove units are tougher.")
    .breakpoint(2, [Bonus::members().mods([Armor.flat(15), MagicResist.flat(15)])])
    .breakpoint(4, [Bonus::members().mods([Armor.flat(40), MagicResist.flat(40)])]);

let tide = TraitDef::new("tide", "Tide")
    .description("Tide units restore mana over time.")
    .breakpoint(
        2,
        [Bonus::members().passive(
            Passive::new("Rising Tide", Trigger::Every { interval_ms: 2000 }).effect(Effect::mana(10).to(Selector::Source)),
        )],
    );

let content = ContentSet::builder()
    .trait_defs([grove, tide])
    .unit(UnitDef::new("warden", "Warden").cost(1).traits(["grove"]))
    .unit(UnitDef::new("stalker", "Stalker").cost(1).traits(["grove", "tide"]))
    .build()
    .unwrap();
assert_eq!(content.traits().count(), 2);
```

## How traits count

- A breakpoint is active when at least `count` **different** units with the
  trait are on the board. Two copies of the same unit count once.
- Bench units don't count.
- A unit counts if it has the trait itself or holds an item that grants it.
- Only the **highest** breakpoint reached applies. Each breakpoint lists
  everything it gives, so the higher one doesn't add to the lower one.
- `content.active_traits(&board_units)` reports, for a board, each trait's
  member count, the breakpoint reached (`tier`) and how many members the
  next one needs (`next_at`). Clients show this in the trait list.

## `TraitDef` calls

| Call | What it does |
| --- | --- |
| `TraitDef::new(id, name)` | a trait with no breakpoints |
| `.description(text)` | the tooltip text |
| `.breakpoint(count, [bonuses])` | a tier active from `count` members. Add tiers in increasing order. A trait needs at least one |

## Bonuses

A `Bonus` is what a breakpoint gives. The same type is used for augments
and for `TeamSetup::bonus`.

| Call | What it does |
| --- | --- |
| `Bonus::members()` | applies to the trait's members. In an augment, it applies to every unit |
| `Bonus::team()` | applies to every unit on the board |
| `Bonus::with_trait(id)` | applies to units with another trait |
| `.mods([...])` | stat modifiers each unit in scope gets |
| `.mods_per_member([...])` | stat modifiers multiplied by a count. In a trait, it is the trait's distinct units on the board. In an augment, it is the named trait's units for `with_trait`, and otherwise the distinct units on the board |
| `.passive(Passive)` | a passive each unit in scope gets |

A breakpoint can hold several bonuses with different scopes:

```rust
use autoengine::content::{Bonus, TraitDef};
use autoengine::effect::{Effect, Passive, Selector, Trigger};
use autoengine::stats::Stat::*;

let bulwark = TraitDef::new("bulwark", "Bulwark")
    .description("The whole team starts with a shield; Bulwark units get a bigger one.")
    .breakpoint(
        1,
        [
            Bonus::team().passive(
                Passive::new("Cover", Trigger::CombatStart).effect(Effect::shield_for(60, 8000).to(Selector::Source)),
            ),
            Bonus::members().passive(
                Passive::new("Stand", Trigger::CombatStart).effect(Effect::shield_for(140, 8000).to(Selector::Source)),
            ),
        ],
    );

// +3% attack speed for each distinct Swarm unit, to every Swarm unit.
let swarm = TraitDef::new("swarm", "Swarm")
    .breakpoint(3, [Bonus::members().mods_per_member([AttackSpeed.percent(30)])]);

// A trait that buffs a different trait's units.
let banner = TraitDef::new("banner", "Banner")
    .breakpoint(2, [Bonus::with_trait("knight").mods([AttackDamage.flat(20)])]);

assert_eq!(bulwark.breakpoints[0].bonuses.len(), 2);
assert_eq!(swarm.breakpoints[0].count, 3);
assert_eq!(banner.id, "banner");
```

## Patterns

**A trait that triggers when its members die:**

```rust
use autoengine::content::{Bonus, TraitDef};
use autoengine::effect::{Effect, Passive, Selector, Trigger};
use autoengine::stats::Stat::*;

let hollow = TraitDef::new("hollow", "Hollow")
    .description("When a Hollow unit dies, the whole team hits harder.")
    .breakpoint(
        2,
        [Bonus::members().passive(
            Passive::new("Vengeance", Trigger::OnDeath).effect(Effect::modify_stats([DamageAmp.flat(80)]).to(Selector::AllAllies)),
        )],
    );
assert_eq!(hollow.breakpoints.len(), 1);
```

**A trait that grows stronger the more members are alive during the fight**,
using a scaling amount that counts tagged allies (traits are tags):

```rust
use autoengine::content::{Bonus, TraitDef};
use autoengine::effect::{Amount, Effect, Passive, ScaleBy, Selector, Trigger, Who};

let pack = TraitDef::new("pack", "Pack")
    .description("Every 3 seconds, Pack units heal 20 for each living Pack unit.")
    .breakpoint(
        2,
        [Bonus::members().passive(
            Passive::new("Pack Bond", Trigger::Every { interval_ms: 3000 }).effect(
                Effect::heal(Amount::flat(0).plus(Who::Source, ScaleBy::AlliesWithTag("pack".into()), 20_000))
                    .to(Selector::Source),
            ),
        )],
    );
assert_eq!(pack.id, "pack");
```

`cargo run -- check` warns about breakpoints no team can reach (more
members needed than units with the trait exist) and traits no shop unit has.
`cargo run -- sim` reports how often each trait wins.
