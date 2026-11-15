# Fights

A fight is a deterministic simulation on a hex board. It moves forward in
fixed steps (ticks, 50 ms by default) using only whole-number math and the
fight's seed, so the same seed and the same boards always give exactly the
same fight on any machine. The server runs each fight once, and clients
replay it from the seed to show it.

## Setting up a fight

Fights in a game are set up by the round types. To run one directly, for a
test, a tool or a preview:

```rust
use autoengine::content::{BoardUnit, TeamSetup};

let content = autoengine::sample::content();
let teams = [
    TeamSetup::new().unit(BoardUnit::new("ironbark_warden", (3, 4)).star(2)).unit(BoardUnit::new("reed_stalker", (3, 7))),
    TeamSetup::new().unit(BoardUnit::new("ashen_hound", (3, 4)).item("whetstone")),
];
let mut combat = content.setup_combat(42, &teams).unwrap();
let outcome = combat.run().clone();
println!("winner: {:?} after {} ms", outcome.winner, outcome.duration_ms);
for event in combat.events() {
    println!("{:>5} ms  {:?}", event.tick * combat.config().tick_ms, event.kind);
}
```

Each team writes its positions from its own side of the board. The first team
is placed as written, and the second is turned half a turn so the two face
each other. `combat.step()` advances one tick at a time instead of
`run()`. `cargo run -- fight` prints a fight the same way.

`TeamSetup::bonus(Bonus)` gives a team extra bonuses, the way augments
do.

## What happens each tick

Units act one at a time, in the order they entered the fight. Each living
unit first counts down its statuses, shields and `Every` passives. Then,
unless it is stunned, it:

1. **Continues a cast**, if it is casting. When the cast time is up, the
   ability's effects happen and `OnCast` passives fire.
2. **Starts a cast** if its mana is full (`MaxMana` above 0), it has an
   ability and it isn't silenced. Mana goes back to zero.
3. Otherwise **picks a target** if it has none: whoever taunted it, or else
   the enemy its [targeting](units.md#targeting) prefers.
4. **Attacks** if the target is within `Range` and it isn't disarmed.
   Attacks come at `AttackSpeed` per second.
5. Otherwise **walks** one hex at a time toward the target at `MoveSpeed`,
   unless rooted. Paths go around other units.

Units summoned during a tick start acting on the next one. Passives fired
by something happening are resolved straight away, in order.

## Attacks

A basic attack:

1. gives the attacker `ManaPerAttack` mana,
2. misses if the target dodges (chance `Dodge` / 1000), which gives a
   `Dodged` event and nothing else,
3. otherwise deals `AttackDamage` physical damage, which can crit, and fires
   the attacker's `OnAttack` passives.

## Damage

Every hit, from an attack, an ability, a passive or damage over time, goes
through the same steps:

1. **Crit:** if the damage can crit (attacks, and damage effects with
   `.with_crit()`), it crits with chance `CritChance` / 1000 and is then
   multiplied by `CritDamage` / 1000.
2. **Amplification:** × (1000 + the source's `DamageAmp`) / 1000.
3. **Overtime:** × (1000 + `overtime_damage_per_second` × seconds into
   overtime) / 1000.
4. **Resistance:** × 100 / (100 + armor) for physical damage, or magic
   resist for magic damage. True damage skips this step. 100 is
   `CombatConfig::resist_constant`.
5. **Reduction:** × (1000 − `DamageReduction`) / 1000, with
   `DamageReduction` capped at 900. True damage skips this step.
6. **Shields** soak what's left, oldest shield first. Then health takes
   the rest.

An invulnerable target takes nothing. After the hit:

- the source heals for `Lifesteal` / 1000 of the damage,
- the target gains 3% of the damage as mana, at most 40 per hit,
- `OnCrit` fires for the source on a crit, and `OnShieldBroken` for the target
  for each shield used up,
- `OnDamaged` fires for the target if the damage came from an attack or an
  ability (not damage over time or a passive),
- `HealthBelow` passives fire when health crosses their threshold,
- at zero health the target dies. `OnDeath` fires for it, `OnKill` for the
  killer and `OnAllyDeath` for its living teammates.

**Example:** 100 physical damage against 50 armor is 100 × 100 / 150 = 66.
With a crit at 1400 critical damage, it is 140 × 100 / 150 = 93.

## Healing and shields

Healing restores up to maximum health, cut by the strongest wound on the
target. Shields absorb damage before health, and a timed shield disappears
when its time runs out.

## Overtime

From `overtime_after_ms` (30 seconds by default) on, all damage grows by
`overtime_damage_per_second` thousandths (250) every second: 25% more in
the first second, double after four seconds. Two boards that can't kill each
other, such as a pair of healers, still produce a winner. An `Overtime`
event marks the moment, and clients show "OVERTIME". A fight still going at
`max_duration_ms` (60 seconds) ends as a draw.

## Who wins

The fight ends when only one team has living units (that team wins), when
no team does (a draw), or at the time limit (a draw). `Outcome` gives the
`winner`, the `duration_ms`, whether it `timed_out` and the `survivors`. In a
game, the losing player takes damage for the stage plus one per surviving
enemy unit (see [Rounds](rounds.md#the-schedule)).

## Reading a fight

| Call | Gives |
| --- | --- |
| `combat.units()`, `combat.unit(id)` | every unit, and one by id (see [Effects](effects.md#rust-hooks) for what a unit can tell you) |
| `combat.unit_at(hex)` | who stands on a hex |
| `combat.targets(id, &selector)` | what a selector would pick for a unit right now |
| `combat.events()`, `combat.drain_events()` | the events so far (listed in [Events](events.md#fight-events)) |
| `combat.outcome()` | the outcome, once the fight is over |
| `combat.time_ms()`, `combat.tick()` | how far the fight has got |

In a game, `game.combats()` holds the last round's fights as
`CombatRecord`s with the teams, seed and every event, enough to replay
them. `Playback` in the `playback` module turns a record into unit
positions, health bars, statuses and effects at any moment, which is what
the client draws.
