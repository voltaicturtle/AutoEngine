# Content guides

These guides explain how to make every part of a game on AutoEngine, with
examples. Every Rust example in them is compiled and run by `cargo test`,
and every Rhai example is compiled, so they stay correct as the engine
changes.

| Guide | Covers |
| --- | --- |
| [Units](units.md) | `UnitDef`: cost, traits, stats, abilities, passives, targeting, summons, models, star levels |
| [Effects](effects.md) | what abilities, passives, traits and items can do: effects, amounts, selectors, triggers, conditions, statuses, stacks, Rust hooks |
| [Resources](resources.md) | mana and resources of your own (rage, energy and so on), ability costs and cooldowns, custom stats |
| [Damage types](damage.md) | physical, magic and true damage, and damage types of your own with their own resistances |
| [Traits](traits.md) | `TraitDef`: breakpoints and bonuses |
| [Items](items.md) | `ItemDef`: stats, passives, recipes, granted traits, single-use items |
| [Augments](augments.md) | `AugmentDef`: lasting bonuses, gold up front, rewards every round, lobby modifiers |
| [Rounds](rounds.md) | the schedule, the built-in round kinds, and new round types in Rhai or Rust |
| [Events](events.md) | every game event, fight event, command and command error, and how to react to events |
| [Scripting](scripting.md) | every function Rhai scripts can call |
| [Rules](rules.md) | `GameRules`: board, economy, levels, shop odds, pool sizes, damage, fight settings, translations |
| [Fights](fights.md) | how a fight runs: ticks, targeting, movement, attacks, casting, the damage formula, overtime |
| [Testing](testing.md) | content tests, the simulator and the other tools for checking content |

[guide.md](../guide.md) is a walk through the command line tools, and
[programming.md](../programming.md) covers using the engine as a library.

## The basics

A game's content is Rust code. One builder collects every definition, and
`build()` checks it:

```rust
use autoengine::content::{Bonus, ContentSet, TraitDef, UnitDef};
use autoengine::stats::Stat::*;

let content = ContentSet::builder()
    .trait_def(TraitDef::new("grove", "Grove").breakpoint(2, [Bonus::members().mods([Armor.flat(15)])]))
    .unit(UnitDef::new("warden", "Warden").cost(2).traits(["grove"]).stat(MaxHealth, 900))
    .unit(UnitDef::new("mender", "Mender").cost(1).traits(["grove"]).stat(Range, 3))
    .build()
    .expect("the content is valid");
assert_eq!(content.units().count(), 2);
```

Definitions can be added in any order. `build()` rejects anything that
can't work, such as a unit naming a trait that doesn't exist, a recipe with
a missing part or a duplicate id, and suggests the closest id for typos.
`cargo run -- check` runs the same checks and also warns about content that
builds but is probably a mistake.

Some things hold everywhere:

- **Ids** are short strings such as `"ashen_hound"`. Definitions refer to
  each other by id. Names are what players see.
- **Numbers are whole numbers.** Fractions use fixed units: *permille* means
  thousandths (`250` is 25%), times are in milliseconds (`3000` is three
  seconds), and speeds are in thousandths per second (`AttackSpeed` `750`
  is 0.75 attacks a second).
- **Everything is deterministic.** Randomness comes from a seed, so the same
  seed and the same inputs always play out the same way. This is what makes
  replays, content tests and the simulator work, and it is why content
  never reads a clock or any other outside state.
- **Board positions** are `(column, row)` from the owning player's point of
  view. On the default 7 × 8 board a player's half is rows 4 to 7, and row 4
  is the front line.

`src/sample.rs` is a complete small game written this way, and
`cargo run -- new ../mygame` writes a starter game to build from.
