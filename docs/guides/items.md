# Items

Players collect items and give them to units. An item can add stats and
passives, make its holder count as having a trait, combine with another item
into something stronger, or, for single-use items, do something once and
disappear.

```rust
use autoengine::content::{Consumable, ContentSet, ItemDef};
use autoengine::effect::{DamageKind, Effect, Passive, Selector, Trigger};
use autoengine::stats::Stat::*;

let content = ContentSet::builder()
    // Components: items with no recipe.
    .item(ItemDef::new("whetstone", "Whetstone").description("+15 attack damage.").mods([AttackDamage.flat(15)]))
    .item(ItemDef::new("ember_core", "Ember Core").description("+10% attack speed.").mods([AttackSpeed.percent(100)]))
    // A finished item, made from two components.
    .item(
        ItemDef::new("cinder_blade", "Cinder Blade")
            .description("Attacks burn for 20 true damage a second for 2 seconds.")
            .recipe("whetstone", "ember_core")
            .mods([AttackDamage.flat(25), AttackSpeed.percent(150)])
            .passive(
                Passive::new("Burn", Trigger::OnAttack)
                    .effect(Effect::damage_over_time(20, DamageKind::True, 2000).to(Selector::EventOther)),
            ),
    )
    // A single-use item.
    .item(
        ItemDef::new("lodestone", "Lodestone")
            .description("Give it to a unit to take back all the items it holds.")
            .consumable(Consumable::RemoveItems),
    )
    .build()
    .unwrap();
assert_eq!(content.combine_items("ember_core", "whetstone").unwrap().id, "cinder_blade");
```

## `ItemDef` calls

| Call | What it does |
| --- | --- |
| `ItemDef::new(id, name)` | an item that does nothing yet |
| `.description(text)` | the tooltip text |
| `.mods([...])` | stat modifiers for the holder |
| `.passive(Passive)` | a passive the holder gets. Can be called more than once |
| `.grants_trait(id)` | the holder counts as having this trait, for trait counts, `AlliesWithTag`, `HasTag` and the like |
| `.recipe(a, b)` | the two items that combine into this one. Order doesn't matter, and `a` and `b` may be the same item |
| `.unique()` | a unit can hold only one of it |
| `.consumable(Consumable)` | makes the item single-use (see below) |

## Components and recipes

An item with no recipe and no `consumable` is a **component**. Components are
what PvE rounds, `RoundGrant::components` and `give_random_item` hand out.

When a unit holds two items that make a recipe, they combine straight away
into the finished item. Giving a component to a unit that is already full
(`GameRules::max_items_per_unit`, 3 by default) is allowed if it combines
with something the unit holds. The client's item tooltips list the
recipes.

What happens to items when units change:

- Selling a unit returns its items to the player.
- When copies merge into a higher star level, the kept unit takes all their
  items, and any that make a recipe combine.

## Single-use items

Giving a single-use item to a unit uses it up instead of equipping it:

| `Consumable` | Effect |
| --- | --- |
| `RemoveItems` | takes every item off the unit and back to the player's items |
| `Duplicate` | gives the player another one-star copy of the unit, from the pool |
| `Reforge` | swaps each of the unit's items for a random other item of the same kind: components for components, finished items for finished items |
| `Gold(n)` | gives `n` gold. Which unit it is given to doesn't matter |
| `Give(Grant)` | hands the player anything a [grant](events.md#grants) can: a currency, experience, health, an item, an augment. Which unit it is given to doesn't matter |

If it can't do anything, the item is kept and the command fails with
`CommandError::CannotUse`, saying why (for example "that unit holds no
items"). Single-use items are never handed out as random components and
can't be part of a recipe.

## Where items come from

- PvE rounds: `RoundKindDef::pve(...).items(n)` gives `n` random components
  for a win.
- Choice rounds: `RoundKindDef::choice(3, ChoiceOffer::Items)` offers items
  to pick from.
- Draft rounds: each unit on offer carries a random component.
- Augments: `RoundGrant { components: 1, .. }` gives a component every round.
- Scripts and Rust: `game.give_item(p, "id")` and
  `game.give_random_item(p)` in Rhai, or `game.give_item(id, "id")` and
  `game.give_random_component(id)` in Rust.

## Examples

**An item that makes its holder a member of a trait**, so a player can reach
a breakpoint with one unit fewer:

```rust
use autoengine::content::ItemDef;
use autoengine::stats::Stat::*;

let sigil = ItemDef::new("grove_sigil", "Grove Sigil")
    .description("The holder counts as Grove.")
    .recipe("oak_charm", "whetstone")
    .mods([MaxHealth.flat(150), AttackDamage.flat(15)])
    .grants_trait("grove");
assert_eq!(sigil.grants_traits, ["grove"]);
```

**An item that shields once when its holder gets low:**

```rust
use autoengine::content::ItemDef;
use autoengine::effect::{Amount, Effect, Passive, ScaleBy, Selector, Trigger, Who};
use autoengine::stats::Stat::*;

let aegis = ItemDef::new("last_aegis", "Last Aegis")
    .description("Below 35% health, gain a shield worth 40% of maximum health for 4 seconds.")
    .recipe("oak_charm", "oak_charm")
    .mods([MaxHealth.flat(250)])
    .passive(
        Passive::new("Last Aegis", Trigger::HealthBelow { permille: 350 }).effect(
            Effect::shield_for(Amount::flat(0).plus(Who::Source, ScaleBy::Stat(MaxHealth), 400), 4000).to(Selector::Source),
        ),
    );
assert!(aegis.recipe.is_some());
```

**An item that wounds whatever its holder hits**, against healing teams:

```rust
use autoengine::content::ItemDef;
use autoengine::effect::{Effect, Passive, Selector, Trigger};
use autoengine::stats::Stat::*;

let serrated = ItemDef::new("serrated_edge", "Serrated Edge")
    .description("Attacks cut the target's healing by 50% for 3 seconds.")
    .recipe("whetstone", "whetstone")
    .mods([AttackDamage.flat(30)])
    .passive(Passive::new("Serrated", Trigger::OnAttack).effect(Effect::wound(500, 3000).to(Selector::EventOther)));
assert_eq!(serrated.passives.len(), 1);
```

**A single-use item that pays out:**

```rust
use autoengine::content::{Consumable, ItemDef};

let coin = ItemDef::new("lucky_coin", "Lucky Coin").description("Give it to any unit for 6 gold.").consumable(Consumable::Gold(6));
assert!(coin.consumable.is_some());
```
