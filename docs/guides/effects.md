# Effects

Abilities, passives, trait bonuses, items and augments are all built from
the same blocks:

- an **effect** says what happens (damage, heal, stun, summon and so on),
- a **selector** says who it happens to,
- a **trigger** says when a passive fires,
- a **condition**, which is optional, decides whether it happens at all.

```rust
use autoengine::effect::{Condition, Effect, Passive, Selector, Trigger, Who};

// When this unit is hit, it burns the attacker, but only while it is above half health.
let thorns = Passive::new("Thorns", Trigger::OnDamaged)
    .when(Condition::HealthAbove { who: Who::Source, permille: 500 })
    .effect(Effect::magic(25).to(Selector::EventOther));
assert_eq!(thorns.effects.len(), 1);
```

`Effect::...()` makes an effect, `.to(selector)` pairs it with who it
hits, which gives an `EffectSpec`, and `.when(condition)` on the spec checks
the condition for each unit it would hit.

## Effects

| Constructor | What it does to each selected unit |
| --- | --- |
| `Effect::physical(amount)` | physical damage, reduced by armor |
| `Effect::magic(amount)` | magic damage, reduced by magic resist |
| `Effect::true_damage(amount)` | damage that ignores armor, magic resist and damage reduction |
| `Effect::damage(amount, DamageKind)` | damage of any kind |
| `.with_crit()` | added to a damage effect, lets it crit using the source's crit stats |
| `Effect::heal(amount)` | restores health, up to the maximum. Wounds cut it |
| `Effect::shield(amount)` | a shield that absorbs damage before health, for the whole fight |
| `Effect::shield_for(amount, ms)` | a shield that disappears after `ms` |
| `Effect::mana(n)` | gives mana. A negative `n` drains it |
| `Effect::resource(id, n)` | the same for any [resource](resources.md) |
| `Effect::stun(ms)` | can't move, attack or cast. A cast already started is paused |
| `Effect::silence(ms)` | can't start casting. Mana and other resources still build up |
| `Effect::disarm(ms)` | can't attack, but can move and cast |
| `Effect::root(ms)` | can't move, but can attack what's in range and cast |
| `Effect::taunt(ms)` | must attack the source while the source lives |
| `Effect::wound(permille, ms)` | healing received is cut by `permille`. Only the strongest wound counts |
| `Effect::invulnerable(ms)` | takes no damage and can't be executed |
| `Effect::unstoppable(ms)` | new debuffs don't take hold |
| `Effect::stat_status(mods, ms)` | stat changes that are undone after `ms` |
| `Effect::damage_over_time(per_second, DamageKind, ms)` | damage every tick, given per second |
| `Effect::modify_stats(mods)` | stat changes for the rest of the fight |
| `Effect::cleanse()` | removes every debuff (see [Statuses](#statuses)) |
| `Effect::execute(permille)` | kills the unit if its health is at or below `permille` of its maximum |
| `Effect::knockback(hexes)` | pushes it straight away from the source, up to `hexes` steps |
| `Effect::pull(hexes)` | drags it up to `hexes` steps toward the source, stopping next to it or at another unit |
| `Effect::leap()` | the *source* jumps to the free hex closest to the selected unit, and targets it if it is an enemy |
| `Effect::revive(amount)` | brings a dead unit back with that much health (see [Bringing units back](#bringing-units-back)) |
| `Effect::tile(id)`, `Effect::tile_for(id, ms)` | lays a [tile](tiles.md) under the unit, for the whole fight or for `ms`. `.spread(radius)` covers the hexes around it too |
| `Effect::summon(unit_id, count)` | puts `count` copies of a unit on the selected unit's team, on the free hexes closest to it |
| `Effect::add_stacks(name, count)` | adds to a named counter on the unit (see [Stacks](#stacks)) |
| `.capped_at(max)` | added to `add_stacks`, caps the counter |
| `.per_stack(mods)` | added to `add_stacks`, gives stats for each stack gained |
| `Effect::clear_stacks(name)` | sets the counter to zero and removes the stats its stacks gave |
| `Effect::mark(name, ms)`, `Effect::harmful_mark(name, ms)` | puts a named mark on the unit for conditions to find (see [Statuses](#statuses)) |
| `Effect::signal(name)` | fires the unit's passives that wait for this [signal](#signals) |
| `Effect::custom(name)` | runs a Rust hook (see [Rust hooks](#rust-hooks)) |
| `Effect::custom_with(name, [args])` | runs a Rust hook with numbers passed to it |

Effects skip units that are already dead, except `summon`, `revive`, `tile`
and custom effects. Stun, silence, disarm, root and taunt last shorter on units
with `Tenacity`. A status applied twice runs twice, each with its own timer.

### Later, and more than once

`.to(selector)` gives an `EffectSpec`, and two calls on it set when the
effect lands:

| Call | Meaning |
| --- | --- |
| `.after(ms)` | the effect lands `ms` later: a fuse, a wind-up, a shot in flight |
| `.repeat(times, every_ms)` | the effect lands `times` times in all, `every_ms` apart: a channel, a burst, a pulse |

```rust
use autoengine::effect::{Ability, Effect, Selector};

let barrage = Ability::new("Barrage")
    // Three volleys half a second apart, the first at once.
    .effect(Effect::physical(40).to(Selector::NearestEnemies(3)).repeat(3, 500))
    // And a charge on the target that goes off two seconds later.
    .effect(Effect::magic(300).to(Selector::CurrentTarget).after(2000));
assert_eq!(barrage.effects[1].delay_ms, 2000);
```

Who receives the effect is decided when the ability or passive resolves,
and conditions are checked then. It lands on those still alive, even if
its source has died in the meantime, and amounts are worked out as it
lands. Times are rounded up to whole ticks.

To pick the receivers at the moment of landing instead, delay a
[signal](#signals) to the source and put the effect in a passive waiting
for it:

```rust
use autoengine::effect::{Ability, Anchor, Effect, Passive, Selector, Trigger};

// Two seconds after the cast, whoever is standing near the caster then.
let cast = Ability::new("Quake").effect(Effect::signal("quake").to(Selector::Source).after(2000));
let quake = Passive::new("Quake lands", Trigger::signal("quake"))
    .effect(Effect::physical(200).to(Selector::EnemiesWithin { anchor: Anchor::Source, radius: 2 }));
assert_eq!(cast.effects.len() + quake.effects.len(), 2);
```

### Bringing units back

`Effect::revive(amount)` stands a dead unit up again with `amount` health
(at least 1, at most its maximum), where it fell or on the nearest free
hex. It comes back without shields, statuses or a cast in progress, and
keeps its stacks and resources. It does nothing to the living.

Three selectors reach the dead: `Source` in an `OnDeath` passive,
`EventOther` in an `OnAllyDeath` passive, and `FallenAllies(n)` anywhere.

```rust
use autoengine::effect::{Ability, Amount, Effect, Passive, ScaleBy, Selector, Trigger, Who};
use autoengine::stats::Stat;

let share = |permille| Amount::default().plus(Who::Target, ScaleBy::Stat(Stat::MaxHealth), permille);
// Once a fight, get back up with 40% health.
let second_wind = Passive::new("Second Wind", Trigger::OnDeath).max_procs(1).effect(Effect::revive(share(400)).to(Selector::Source));
// An ability that raises the ally who fell last, with a quarter of theirs.
let raise = Ability::new("Raise").effect(Effect::revive(share(250)).to(Selector::FallenAllies(1)));
assert_eq!(second_wind.max_procs, Some(1));
assert_eq!(raise.effects.len(), 1);
```

A unit that revives sends a `Revived` fight event. Death passives without
`max_procs` fire again each time the unit dies.

### Stat modifiers

`stat_status`, `modify_stats`, `per_stack` and the bonuses of traits, items
and augments all take stat modifiers:

```rust
use autoengine::stats::Stat::*;

let mods = [Armor.flat(20), AttackSpeed.percent(150), MagicResist.flat(-10)];
assert_eq!(mods[1].permille, 150);
```

`Stat.flat(n)` adds `n` and `Stat.percent(permille)` adds `permille`
thousandths: `percent(150)` is +15%. Negative values lower the stat. The
list of stats is in [Units](units.md#stats).

## Amounts

Damage, heals, shields and damage over time take an `Amount`. A plain number
works as a fixed amount:

```rust
use autoengine::effect::{Amount, Effect, ScaleBy, Who};
use autoengine::stats::Stat::*;

let fixed = Effect::magic(100);
let scaled = Effect::magic(Amount::power(100)); // 100 × ability power / 100
let mixed = Effect::physical(
    Amount::power(80)
        .plus(Who::Source, ScaleBy::Stat(AttackDamage), 1500) // + 150% of the source's attack damage
        .plus(Who::Target, ScaleBy::MissingHealth, 50),       // + 5% of the target's missing health
);
```

An amount is `flat + each scaling term`, and then, for `Amount::power`,
multiplied by the source's `AbilityPower` / 100. Each term is
`value × permille / 1000`.

| `ScaleBy` | Value read from the unit |
| --- | --- |
| `Stat(stat)` | the unit's current value of a stat |
| `CurrentHealth` | its health now |
| `MissingHealth` | maximum health minus health now |
| `Stacks(name)` | its count of a named stack |
| `AlliesWithTag(tag)` | living units on its team, itself included, that carry a tag (traits are tags) |
| `Resource(id)` | how much of a [resource](resources.md) it has now |

`Who::Source` is the unit producing the effect and `Who::Target` is the unit
receiving it.

## Selectors

Selectors pick units relative to the source, the unit producing the effect.
"Allies" always includes the source itself. Results come in a fixed order,
so fights stay repeatable.

| `Selector` | Picks |
| --- | --- |
| `Source` | the source |
| `CurrentTarget` | the source's attack target, or the nearest enemy if it has none |
| `EventOther` | the other unit in the event that fired the passive (see [Triggers](#triggers)). Nothing if there is none |
| `NearestEnemies(n)` | the `n` closest enemies |
| `FarthestEnemies(n)` | the `n` farthest enemies |
| `LowestHealthEnemies(n)` | the `n` enemies with the least health |
| `LowestHealthAllies(n)` | the `n` allies with the least health |
| `HighestStatEnemies { stat, n }` | the `n` enemies with the most of a stat |
| `HighestStatAllies { stat, n }` | the `n` allies with the most of a stat |
| `LowestStatEnemies { stat, n }` | the `n` enemies with the least of a stat |
| `LowestStatAllies { stat, n }` | the `n` allies with the least of a stat |
| `RandomEnemies(n)` | up to `n` enemies picked at random (from the fight's seed) |
| `RandomAllies(n)` | up to `n` allies picked at random, the source included |
| `AllEnemies` | every living enemy |
| `AllAllies` | every living ally |
| `OtherAllies` | every living ally except the source |
| `FallenAllies(n)` | up to `n` dead allies, the most recently fallen first. Only `revive` does anything to them |
| `EnemiesWithin { anchor, radius }` | enemies within `radius` hexes of the anchor |
| `AlliesWithin { anchor, radius }` | allies within `radius` hexes of the anchor |
| `AlliesWithTag(tag)` | allies carrying a tag, such as a trait id |
| `Custom(name)` | whatever a Rust hook returns (see [Rust hooks](#rust-hooks)) |

`anchor` is `Anchor::Source` or `Anchor::CurrentTarget`. Radius 1 means the
anchor and the six hexes next to it.

To see what a selector would pick in a running fight, call
`combat.targets(unit_id, &selector)`.

## Triggers

A passive fires when its trigger happens. Many triggers involve another
unit, which `Selector::EventOther` picks and which a passive's own
condition sees as `Who::Target`.

| `Trigger` | Fires when | Other unit |
| --- | --- | --- |
| `CombatStart` | the fight starts | none |
| `OnAttack` | the owner lands a basic attack (not on a miss) | the unit attacked |
| `OnDamaged` | the owner takes damage from an attack or an ability, but not from damage over time or a passive | the attacker |
| `OnCast` | the owner finishes casting its ability | none |
| `OnCrit` | the owner lands a critical hit | the unit hit |
| `OnKill` | the owner kills a unit | the victim |
| `OnDeath` | the owner dies | the killer |
| `OnAllyDeath` | another unit on the owner's team dies | the fallen ally |
| `OnEnemyDeath` | an enemy dies, whoever killed it | the one that died |
| `OnDodge` | the owner dodges a basic attack | the attacker |
| `OnHealed` | the owner is healed by an ability, but not by lifesteal or a passive | the healer |
| `OnAllyCast` | another unit on the owner's team finishes casting | the caster |
| `Trigger::signal(name)` | an `Effect::signal(name)` reaches the owner (see [Signals](#signals)) | the unit that sent it |
| `OnShieldBroken` | one of the owner's shields is used up by damage | the unit that broke it |
| `Every { interval_ms }` | every `interval_ms` while the owner lives, first after one interval | none |
| `HealthBelow { permille }` | the owner's health drops to or below `permille` of its maximum. Fires once per fight unless `max_procs` says otherwise | the attacker |

Dead units only resolve their `OnDeath` passives. A chain of passives
triggering each other stops after 10,000 in one go, so content that triggers
itself forever can't hang a fight.

### Signals

A signal is an occasion the content makes up. `Effect::signal(name)` sends
it to the selected units, and each of them fires its passives that wait for
`Trigger::signal(name)`. This lets one piece of content set off another
without either knowing what the other does:

```rust
use autoengine::effect::{Ability, Effect, Passive, Selector, Trigger};

// The ability calls "rally". Any unit, item or trait can answer it.
let rally = Ability::new("Rally").effect(Effect::signal("rally").to(Selector::OtherAllies));
let answer = Passive::new("Answer the Call", Trigger::signal("rally")).effect(Effect::shield(80).to(Selector::Source));
assert_eq!(rally.effects.len() + answer.effects.len(), 2);
```

## Conditions

A condition on a passive (`Passive::when`) is checked once when the
trigger fires, with the event's other unit as the target. If it fails,
nothing happens and no use of `max_procs` is spent. A condition on an effect
(`EffectSpec::when`) is checked for each selected unit, with that unit as
the target. A condition about a target that doesn't exist is false.

| `Condition` | True when |
| --- | --- |
| `HealthBelow { who, permille }` | health is at or below `permille` of maximum |
| `HealthAbove { who, permille }` | health is above `permille` of maximum |
| `Stunned(who)` | the unit is stunned |
| `Shielded(who)` | the unit has a shield |
| `HasDebuff(who)` | the unit has at least one debuff |
| `HasTag { who, tag }` | the unit carries a tag: its traits, including traits its items grant |
| `StacksAtLeast { who, stack, count }` | the unit has at least `count` of a stack |
| `AlliesWithTagAtLeast { tag, count }` | at least `count` living units on the source's team carry a tag |
| `OnTile { who, tile }` | the unit stands on a [tile](tiles.md) of this kind |
| `HasStatus { who, status }` | the unit has a status with this name: `"stunned"`, `"rooted"` and so on, or the name of a [mark](#statuses) |
| `ResourceAtLeast { who, resource, permille }` | the unit's bar of a [resource](resources.md) is at least `permille` full |
| `Condition::chance(permille)` | a random roll from the fight's seed |
| `Condition::custom(name)` | a Rust predicate says so (see [Rust hooks](#rust-hooks)) |
| `c.not()`, `a.and(b)`, `a.or(b)` | combine conditions (`Not`, `All(vec)`, `Any(vec)` as values) |

```rust
use autoengine::effect::{Condition, Effect, Selector, Who};

// Execute low-health enemies that aren't shielded; otherwise a 20% chance to stun them.
let execute = Effect::execute(150)
    .to(Selector::CurrentTarget)
    .when(Condition::Shielded(Who::Target).not());
let maybe_stun = Effect::stun(1000)
    .to(Selector::CurrentTarget)
    .when(Condition::chance(200).and(Condition::Stunned(Who::Target).not()));
```

## Statuses

Statuses are what `stun`, `silence`, `stat_status`, `damage_over_time` and
the like put on a unit. Clients show them over units. The **debuffs** are:
stun, silence, disarm, root, taunt, wound, damage over time, and stat
changes that lower any stat. A lower `MaxMana` helps the unit, so it
doesn't count. `cleanse` removes debuffs, `HasDebuff` checks for them and
`unstoppable` blocks new ones. Invulnerable and unstoppable are not debuffs.

A **mark** is a status that does nothing by itself. It is there for
conditions to find with `Condition::HasStatus`, and clients show its name
over the unit like any other status. `Effect::mark(name, ms)` puts one on,
and `Effect::harmful_mark(name, ms)` puts on one that counts as a debuff.

```rust
use autoengine::effect::{Condition, Effect, Passive, Selector, Trigger, Who};

// Attacks deal 40 bonus damage to targets something has marked as hunted.
let finish = Passive::new("Finish", Trigger::OnAttack)
    .when(Condition::HasStatus { who: Who::Target, status: "hunted".into() })
    .effect(Effect::true_damage(40).to(Selector::EventOther));
let mark = Effect::harmful_mark("hunted", 4000).to(Selector::CurrentTarget);
assert!(finish.condition.is_some() && mark.condition.is_none());
```

The built-in statuses are named `stunned`, `silenced`, `disarmed`, `rooted`,
`taunted`, `wounded`, `afflicted` (damage over time), `stat-modified`,
`invulnerable` and `unstoppable`.

## Stacks

Stacks are named counters on a unit that last for the fight. They can give
stats for each stack, and amounts and conditions can read them.

```rust
use autoengine::effect::{Amount, Condition, Effect, Passive, ScaleBy, Selector, Trigger, Who};
use autoengine::stats::Stat::*;

// Every attack adds a stack of "fury", up to 5, each worth +8% attack speed.
let build_up = Passive::new("Fury", Trigger::OnAttack)
    .effect(Effect::add_stacks("fury", 1).capped_at(5).per_stack([AttackSpeed.percent(80)]).to(Selector::Source));

// At 5 stacks, the next attack deals 40 extra damage per stack and spends them.
let release = Passive::new("Release", Trigger::OnAttack)
    .when(Condition::StacksAtLeast { who: Who::Source, stack: "fury".into(), count: 5 })
    .effect(Effect::true_damage(Amount::flat(0).plus(Who::Source, ScaleBy::Stacks("fury".into()), 40_000)).to(Selector::EventOther))
    .effect(Effect::clear_stacks("fury").to(Selector::Source));
```

The scaling term is `stacks × 40000 / 1000`, which is 40 per stack.
Passives fire in the order they were added, so `release` above sees the
fifth stack that `build_up` just added.

## Rust hooks

For anything the blocks above can't express, register a Rust function on
the content builder and refer to it by name. `build()` rejects names that
nobody registered.

```rust
use autoengine::content::{ContentSet, UnitDef};
use autoengine::effect::{Ability, Condition, Effect, Selector};
use autoengine::stats::Stat::*;

let content = ContentSet::builder()
    // Steals `arg(0)` health: true damage to the target, the same amount healed to the source.
    .custom_effect("siphon", |ctx| {
        let amount = ctx.arg(0);
        ctx.apply(ctx.target(), &Effect::true_damage(amount));
        ctx.apply(ctx.source(), &Effect::heal(amount));
    })
    // True when the source's team has fewer living units than the enemy.
    .custom_condition("outnumbered", |ctx| {
        let combat = ctx.combat();
        let team = combat.unit(ctx.source()).team();
        let (mine, theirs) = combat.units().iter().filter(|u| u.is_alive()).fold((0, 0), |(m, t), u| {
            if u.team() == team { (m + 1, t) } else { (m, t + 1) }
        });
        mine < theirs
    })
    .unit(
        UnitDef::new("leech", "Leech").cost(1).stat(MaxMana, 50).ability(
            Ability::new("Siphon")
                .effect(Effect::custom_with("siphon", [120]).to(Selector::CurrentTarget))
                .effect(Effect::mana(25).to(Selector::Source).when(Condition::custom("outnumbered"))),
        ),
    )
    .build()
    .unwrap();
```

A custom effect runs once per selected unit and gets an `EffectContext`:

| Call | Gives |
| --- | --- |
| `ctx.source()`, `ctx.target()` | the unit producing the effect, and the selected unit this call is for |
| `ctx.other()` | the other unit in the triggering event, if any |
| `ctx.args()`, `ctx.arg(i)` | the numbers from `custom_with`. A missing one is 0 |
| `ctx.combat()` | the whole fight, read-only: `unit(id)`, `units()`, `unit_at(hex)`, `targets(id, &selector)`, `time_ms()`, `board()` |
| `ctx.rng()` | the fight's dice, the only randomness a hook may use |
| `ctx.select(&selector)` | units a selector picks for the source |
| `ctx.eval(&amount, target)` | works out an amount |
| `ctx.check(&condition, target)` | checks a condition |
| `ctx.apply(target, &effect)` | applies any effect from the source, so events, triggers and stats stay consistent |
| `ctx.apply_from(source, target, &effect)` | the same, as if another unit produced it |
| `ctx.move_unit(unit, hex)` | moves a living unit to a free hex. Returns whether it moved |

A custom condition gets a `ConditionContext` with `source()`, `target()`
(an `Option`) and `combat()`.

A custom selector is registered with `.custom_selector(name, |ctx| ...)` and
used as `Selector::Custom(name.into())`. It gets a `SelectContext` with
`source()`, `other()`, `combat()` and `rng()` (dice that don't move the
fight's own), and returns the ids of the units it picks. It must return
the same units in the same order every time, so sort by something stable
such as distance and then id. Dead units it returns are skipped.

```rust
use autoengine::content::ContentSet;
use autoengine::effect::{Effect, Selector};

let content = ContentSet::builder()
    // Every living enemy in the same column as the source.
    .custom_selector("my_column", |ctx| {
        let me = ctx.combat().unit(ctx.source());
        let column = me.pos().to_offset().0;
        let enemies = ctx.combat().units().iter().filter(|u| u.is_alive() && u.team() != me.team());
        enemies.filter(|u| u.pos().to_offset().0 == column).map(|u| u.id()).collect()
    })
    .build()
    .unwrap();
let skewer = Effect::physical(90).to(Selector::Custom("my_column".into()));
# let _ = (content, skewer);
```

On a unit in the fight (`ctx.combat().unit(id)`) you can read `health()`,
`mana()`, `resource(id)`, `pools()`, `cooldown_ms(ability)`, `casting()`,
`shield()`,
`stat(stat)`, `pos()`, `team()`, `target()`,
`is_alive()`, `is_casting()`, `is_stunned()`, `has_status(|s| ...)`,
`statuses()`, `stacks(name)`, `has_tag(tag)`, `name()` and `template()`.

Hooks must be deterministic: no clocks, no global state, and only
`ctx.rng()` for dice. They live in the program, not in the content, so
every program that runs the fights must register the same hooks. That
means the server and, for networked games, the clients, which replay
fights themselves.
