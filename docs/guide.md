# Making a game, step by step

A game built on AutoEngine is a Rust crate. Its content (units, traits,
items, augments, the round schedule and the tests that pin down how fights
should go) is Rust code built with `ContentSet::builder()`, and its `main`
hands that content to the engine's command line, which provides every tool
below. This walks through them in the order you will use them.

In this repository the sample game works the same way: its content is in
`src/sample.rs`, and `cargo run -- COMMAND` runs the tools on it.

## 1. Start from the starter game

```
cargo run -- new ../mygame
cd ../mygame
```

This writes a small, complete game crate: three units and a neutral, two
traits, three items, three augments, six rounds, one script and two content
tests, all in `src/lib.rs`. It depends on this engine by path.
`README.md` lists things to try.

## 2. Check it

```
cargo run -- check
```

Mistakes that stop the content building (a unit naming a trait that doesn't
exist, a recipe with a missing component, a mistyped id) are reported with a
suggestion for the closest valid id. `check` also warns about things that
build but are probably wrong: trait breakpoints no team can reach, traits no
shop unit has, units whose cost never appears in a shop, model files that are
missing, and rounds that use a kind nothing defines.

## 3. Watch a fight

```
cargo run -- fight            # the game's sample_match()
cargo run -- fight 7          # the same fight with other dice
```

This prints every move, hit, cast and death with its time. The same seed
always gives exactly the same fight.

## 4. Pin down what should happen

`tests()` in `src/lib.rs` holds fights with expected results:

```rust
Scenario::new(
    "two guards beat one",
    [
        TeamSetup::new().unit(BoardUnit::new("shield_bearer", (3, 4))).unit(BoardUnit::new("field_medic", (3, 6))),
        TeamSetup::new().unit(BoardUnit::new("shield_bearer", (3, 4))),
    ],
)
.runs(5)
.expect([Expect::Winner(0), Expect::Survives(0, "shield_bearer".into()), Expect::LastsAtMost { seconds: 40 }]),
```

```
cargo test                          # runs them with the rest of the crate's tests
cargo run -- test                   # runs them with a report per test
cargo run -- test --filter guards
```

A failing test says what was expected, what happened and with which seed, so
you can watch that exact fight:

```
cargo run -- fight 3 --test guards
```

| Expectation | Meaning |
| --- | --- |
| `Expect::Winner(0)` | team 0 wins every run |
| `Expect::WinsAtLeast { team: 0, percent: 60 }` | team 0 wins at least that share of the runs |
| `Expect::Draw` | nobody wins |
| `Expect::LastsAtMost { seconds: 30 }`, `Expect::LastsAtLeast { seconds: 5 }` | how long the fight takes |
| `Expect::Survives(0, "unit_id".into())`, `Expect::Dies(1, ...)` | what becomes of a unit |
| `Expect::Casts(0, "unit_id".into())` | the unit gets to use its ability |

## 5. Play whole games

```
cargo run -- lobby                 # bots play one game; prints every round
cargo run -- sim --games 50        # bots play many; prints a balance table
cargo run -- sim --json            # the same numbers for a spreadsheet or script
```

`sim` shows, for each unit and trait, how often it fought, how often it won
and where boards that used it placed.

## 6. Add rules

A round type or a rule can be written in Rust (`RoundType` and `GameHook`,
see [programming.md](programming.md)) or as a Rhai script.
`scripts/bonus_gold.rhai` is a scripted round type: copy it, add the copy
with `.script("my_round", include_str!("../scripts/my_round.rhai"))`, and use
`RoundKindDef::custom("my_round")` in the schedule. A script can also define
`on_event(game, event)` to react to anything that happens.

```
cargo run -- api        # everything scripts can call, and every event
```

## 7. See it

```
cargo run -- serve --dev
autoengine-client --assets assets
```

The client gets the game's content from the server when it joins, so the same
client binary plays any game. `--dev` turns on the cheat console (F1 in the
client): `gold 50`, `unit shield_bearer`, `item iron_ingot`, `skip`. Add
models, icons and sounds to `assets/` when you have them: see
[art.md](art.md).

## 8. Keep it fast and stable

```
cargo run --release -- bench        # fights and games per second
cargo run -- serve --record replays/
cargo run -- replay replays/*.json  # do saved fights still play out the same?
```

## Content reference

Everything is defined with constructors and chainable setters, and checked
when `build()` runs. `src/sample.rs` in this repository is a complete,
original example:

| Function | Contents |
| --- | --- |
| `units()` | `UnitDef`: cost, traits, stats, ability, passives, 3D model |
| `traits()` | `TraitDef`: the bonuses each breakpoint grants |
| `items()` | `ItemDef`: stat bonuses, passives, granted traits, recipes |
| `augments()` | `AugmentDef`: lasting team-wide modifiers |
| `schedule()` | `Schedule` of `RoundDef`s: Pvp, Pve, Draft, Choice and custom rounds |
| `GameRules` | board size, item limit, star levels, economy, levels and shop odds (all have defaults) |
| `sample_match()` | two boards to fight each other |
| `tests()` | fights with expected results |

The sample's models, icons and scripts live in `content/`.

Abilities, trait bonuses and item effects are all written with the same
building blocks: an effect (damage, heal, shield, stun, stat change, summon,
...), a selector for who it hits, and for passives a trigger for when it fires.

### Conditions, stacks and movement

Beyond stat bonuses, content can express more complex rules:

- **Conditions** gate a whole passive (`Passive::when`) or one effect per
  target (`EffectSpec::when`): `HealthBelow`, `HealthAbove`,
  `Stunned`, `Shielded`, `HasTag`, `StacksAtLeast`, `AlliesWithTagAtLeast`,
  a seeded `Chance`, and `Not` / `All` / `Any` to combine them.
- **Stacks** are named counters on a unit. `AddStacks` can be capped with
  `max`, and each stack can grant stats with `mods_per_stack`. `ClearStacks`
  resets the counter and removes those stats. Amounts can scale with a stack
  count (`ScaleBy::Stacks("frenzy")`) or with living allies carrying a tag
  (`ScaleBy::AlliesWithTag("grove")`).
- **Per-member trait bonuses**: `Bonus::mods_per_member` is multiplied by the
  number of distinct units with the trait.
- **Movement**: `Effect::knockback(n)` pushes a unit straight away from the
  source, and `Effect::leap()` jumps the source next to its target.

```rust
use autoengine::effect::{Condition, Effect, Passive, Selector, Trigger, Who};

UnitDef::new("frenzied_brawler", "Frenzied Brawler")
    .cost(2)
    .passive(
        Passive::new("Frenzy", Trigger::OnAttack).effect(
            Effect::add_stacks("frenzy", 1).capped_at(10).per_stack([AttackSpeed.percent(50)]).to(Selector::Source),
        ),
    )
    .passive(
        Passive::new("Finisher", Trigger::OnAttack)
            .when(Condition::StacksAtLeast { who: Who::Source, stack: "frenzy".into(), count: 10 })
            .effect(Effect::knockback(1).to(Selector::EventOther))
            .effect(Effect::stun(500).to(Selector::EventOther).when(Condition::Stunned(Who::Target).not()))
            .effect(Effect::clear_stacks("frenzy").to(Selector::Source)),
    )
```

Anything these blocks can't express can be written as a Rust hook. Register
it with `ContentSet::builder().custom_effect(..)` or `.custom_condition(..)`,
then use it as `Effect::custom_with("name", [args])` or
`Condition::custom("name")`. See [programming.md](programming.md#content-in-rust).

