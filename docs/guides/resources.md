# Resources

A resource is something a unit builds up and spends to cast: mana, rage,
energy, heat, ammunition. The engine ships with mana, and nothing in a
fight is specific to it. A game can retune mana, add resources of its own
next to it, or drop mana altogether.

Three things work together:

- a **resource** (`ResourceDef`) says how it is gained and lost, for the
  whole game,
- a **stat** holds how much of it each unit can have, so items, traits,
  star levels and statuses can change it like any other stat,
- an ability's **cost** (`Cost`) names the resource it spends.

## A resource of your own

```rust
use autoengine::content::{ContentSet, UnitDef};
use autoengine::effect::{Ability, Cost, Effect, Selector};
use autoengine::resource::ResourceDef;
use autoengine::stats::Stat::{self, *};

// The stat that holds each unit's capacity. Declare it once and use the
// constant everywhere.
const MAX_RAGE: Stat = Stat::custom("max_rage");

let content = ContentSet::builder()
    // Rage comes from fighting and drains away when the fighting stops.
    .resource(
        ResourceDef::new("rage", "Rage", MAX_RAGE)
            .per_attack(5)
            .from_damage_taken(100, Some(25))
            .from_damage_dealt(50, None)
            .per_second(-4)
            .color([220, 70, 50]),
    )
    .unit(
        UnitDef::new("ravager", "Ravager")
            .cost(2)
            .stats([(MaxHealth, 900), (AttackDamage, 70), (MAX_RAGE, 100)])
            .ability(
                Ability::new("Rampage")
                    .cost(Cost::of("rage", 60))
                    .effect(Effect::physical(200).with_crit().to(Selector::CurrentTarget)),
            ),
    )
    .build()
    .expect("the content is valid");
assert_eq!(content.resources().len(), 2); // mana and rage
```

A unit has a resource when its capacity stat is above zero. The ravager
above has rage and no mana. A unit can have several resources at once, and
units without a resource are never given any.

| Call | What it does |
| --- | --- |
| `ResourceDef::new(id, name, max)` | a resource nothing gives yet. `max` is the stat that holds each unit's capacity |
| `ResourceDef::mana()` | the built-in mana, as a starting point for changing it |
| `.start(amount)` | how much a unit starts each fight with, up to its capacity |
| `.starts_full()` | starts each fight full |
| `.per_attack(amount)` | gained on each basic attack, hit or miss |
| `.from_damage_taken(permille, cap)` | gained from damage the unit survives: this many thousandths of it, at most `cap` per hit (`None` is no limit) |
| `.from_damage_dealt(permille, cap)` | the same for damage the unit deals |
| `.per_kill(amount)` | gained when the unit kills an enemy |
| `.per_second(amount)` | gained once a second. Negative drains it |
| `.changes_while_casting()` | keeps it changing while the unit casts. Without this it is frozen for the length of the cast, as mana is |
| `.color([r, g, b])` | the color of its bar in clients |

Amounts are worked out for the unit that owns the resource. They can be
plain numbers, or read the unit's stats, which lets items and traits change
them: `.per_attack(Amount::stat(RAGE_PER_ATTACK))` gives each unit whatever
its `RAGE_PER_ATTACK` stat says.

The builder has three calls for resources:

| Call | What it does |
| --- | --- |
| `.resource(def)` | adds a resource, or replaces the one with the same id |
| `.without_resource(id)` | takes one out, such as `.without_resource("mana")` |
| `content.resources()`, `content.resource(id)` | reads them back |

## Mana

Mana is defined with the same calls:

```rust
use autoengine::effect::Amount;
use autoengine::resource::ResourceDef;
use autoengine::stats::Stat::*;

let mana = ResourceDef::new("mana", "Mana", MaxMana)
    .start(Amount::stat(StartingMana))
    .per_attack(Amount::stat(ManaPerAttack))
    .from_damage_taken(30, Some(40))
    .color([70, 130, 230]);
assert_eq!(mana, ResourceDef::mana());
```

To change how mana works in your game, pass a changed copy to the builder.
This game gives no mana for taking damage and a little every second:

```rust
use autoengine::content::ContentSet;
use autoengine::resource::ResourceDef;

let content = ContentSet::builder()
    .resource(ResourceDef::mana().from_damage_taken(0, None).per_second(5))
    .build()
    .unwrap();
assert_eq!(content.resource("mana").unwrap().from_damage_taken.permille, 0);
```

## Costs and cooldowns

An ability is cast as soon as its unit can pay the cost, the ability is off
cooldown and the unit isn't silenced.

| Cost | The unit casts when | The cast takes |
| --- | --- | --- |
| `Cost::full("mana")` (the default) | the bar is full | everything |
| `Cost::of("energy", 40)` | it has at least 40 | 40, and the rest stays |
| `Cost::Free` | the cooldown is over | nothing |

`.cooldown(ms)` on an ability sets the least time between two casts,
counted from the start of a cast. It combines with any cost. A free ability
must have a cooldown.

```rust
use autoengine::effect::{Ability, Cost, Effect, Selector};

// Every four seconds, whatever else is going on.
let pulse = Ability::new("Pulse").cost(Cost::Free).cooldown(4000).effect(Effect::magic(60).to(Selector::AllEnemies));
assert_eq!(pulse.cooldown_ms, 4000);
```

## Giving, taking and reading resources

Effects, amounts and conditions can all use a resource by id:

| Call | What it does |
| --- | --- |
| `Effect::resource(id, amount)` | gives the selected units some of a resource. A negative amount takes it away. Units without the resource are skipped |
| `Effect::mana(amount)` | short for `Effect::resource("mana", amount)` |
| `ScaleBy::Resource(id)` | how much of it a unit has right now, for an [amount](effects.md#amounts) |
| `Condition::ResourceAtLeast { who, resource, permille }` | the unit's bar is at least this full |

Anything the fixed gains above don't cover is a passive. This one gives
rage whenever an ally dies:

```rust
use autoengine::effect::{Effect, Passive, Selector, Trigger};

let mourn = Passive::new("Mourn", Trigger::OnAllyDeath).effect(Effect::resource("rage", 20).to(Selector::Source));
assert_eq!(mourn.effects.len(), 1);
```

In a Rust hook, `ctx.combat().unit(id)` has `resource(id)` (the amount),
`pool(id)` (the amount, the capacity and the definition) and `pools()`
(every resource the unit has).

## Stats of your own

`Stat::custom(name)` makes a stat the engine gives no meaning to. It is
stored and modified like the built-in stats, so `MAX_RAGE.flat(20)` on an
item or `MAX_RAGE.percent(250)` on a trait bonus work, and
`ScaleBy::Stat`, the `HighestStat` selectors and Rust hooks can read it.
Units have 0 of a custom stat unless given some. The name can't be empty,
longer than 48 bytes or the name of a built-in stat.

Custom stats aren't only for resources. A game with a luck stat, a size
stat or a stat that a Rust hook reads declares them the same way.

## What players see

Every change to a unit's resources is a `ResourceChanged` fight event with
the new amount and the capacity, and `Playback` keeps them as
`UnitView::resources`. The bundled client draws one thin bar per resource
under each unit's health bar, in the resource's color, and shows what an
ability costs in the unit's tooltip. Resource names can be translated with
the key `resource.<id>.name`.

`cargo run -- check` warns about a unit whose ability it can never pay for,
such as one with a mana ability and no `MaxMana`.
