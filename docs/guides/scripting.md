# Scripting

Rules can be written in [Rhai](https://rhai.rs), a small scripting language
with Rust-like syntax. Scripts are part of the content, so they reach every
server and client without rebuilding anything, and the command line tools
run them.

```rust,ignore
ContentSet::builder()
    .script("payday", include_str!("../content/scripts/payday.rhai"))
    .script("second_wind", include_str!("../content/scripts/second_wind.rhai"))
```

`build()` compiles every script and rejects one that doesn't compile, naming
the script and the line. Scripts are compiled once per game and then run
from the compiled form. Rhai interprets them; nothing is compiled to
machine code.

## What a script can define

| Function | Runs | Makes the script |
| --- | --- | --- |
| `finish(game)` | when a round of this script's kind ends | a round kind named after the script, used as `RoundKindDef::custom("name")` |
| `begin(game)` | when a round of this script's kind starts (optional) | |
| `on_event(game, event)` | for every game event (see [Events](events.md#game-events)) | a rule that listens to the whole game |

A script needs `finish` or `on_event`, and can have both. A script with
only `on_event` is never a round. It just listens.

## Functions on `game`

`p` is a player id: a number from `game.players()` or an event's `player`
field.

**Reading the game**

| Function | Returns |
| --- | --- |
| `game.players()` | ids of the players still in the game |
| `game.all_players()` | ids of every player, including eliminated ones |
| `game.name(p)` | a player's name |
| `game.gold(p)` | a player's gold |
| `game.amount(p, "id")` | how much a player has of a currency. `"gold"` is gold, and one the game does not define reads as 0 |
| `game.health(p)` | a player's health |
| `game.level(p)` | a player's level |
| `game.is_alive(p)` | whether a player is still in the game |
| `game.unit_count(p)` | how many units a player owns, board and bench |
| `game.item_count(p)` | how many unequipped items a player holds |
| `game.streak(p)` | a player's streak: wins are positive, losses negative |
| `game.stage()` | the current stage |
| `game.round()` | the current round, counting from 0 |
| `game.round_name()` | the current round's name from the schedule |
| `game.param("name")` | a number from the round's `.param(name, value)` in the schedule, or 0 |

**Remembering and dice**

| Function | Does |
| --- | --- |
| `game.get("key")` | a number saved with `set`, or 0 |
| `game.set("key", n)` | saves a number for the rest of the game. Every script shares the same saved numbers |
| `game.random(n)` | a number from 0 to n − 1, from the game's seed |

**Acting**

| Function | Does |
| --- | --- |
| `game.give_gold(p, n)` | gives gold. A negative `n` takes it away, but never below zero |
| `game.give(p, "id", n)` | gives some of a currency other than gold. A negative `n` takes it away, but never below zero |
| `game.take(p, "id", n)` | takes `n` of a currency if the player has that much, and says whether it did. Use it to charge for something |
| `game.give_xp(p, n)` | gives experience |
| `game.give_item(p, "id")` | gives an item |
| `game.give_random_item(p)` | gives a random component item |
| `game.give_unit(p, "id")` | puts a one-star unit on the bench, taking a copy from the pool if one is left |
| `game.give_augment(p, "id")` | gives an augment |
| `game.damage(p, n)` | takes health |
| `game.heal(p, n)` | gives health back, up to what players start with |
| `game.log("text")` | shows a line in every client's log |
| `game.fight_all()` | pairs everyone up to fight, as a PvP round does |

Actions happen when the function returns, in the order they were asked
for. `gold` and `health` reflect a script's own `give_gold` and `damage`
straight away, so a script can read back what it just did. An action that
names something that doesn't exist (an unknown item, a player that isn't
there) is skipped and reported as a `Script` event.

`cargo run -- api` prints this list and every event. A test keeps that
output in step with the engine.

## Events

`event.kind` is the event's name, and the event's fields are beside it:
`event.player`, `event.gold` and so on. The full list is in
[Events](events.md#game-events). Players are numbers, and absent values are
`()`.

## Examples

**A round where the player with the least health gets help:**

```rhai
fn finish(game) {
    let weakest = -1;
    for p in game.players() {
        if weakest < 0 || game.health(p) < game.health(weakest) {
            weakest = p;
        }
    }
    if weakest >= 0 {
        game.give_gold(weakest, 5 + game.param("bonus"));
        game.give_random_item(weakest);
        game.log(`${game.name(weakest)} receives charity`);
    }
}
```

**A lottery round: one random player wins a unit:**

```rhai
fn begin(game) {
    game.log("The lottery is drawn at the end of this round.");
}

fn finish(game) {
    let players = game.players();
    let winner = players[game.random(players.len())];
    game.give_unit(winner, "ashen_hound");
    game.log(`${game.name(winner)} wins the lottery!`);
}
```

**A rule: three losses in a row earn an item, once per game:**

```rhai
fn on_event(game, event) {
    if event.kind == "PlayerDamaged" {
        let p = event.player;
        if game.streak(p) <= -3 && game.get(`consoled_${p}`) == 0 {
            game.set(`consoled_${p}`, 1);
            game.give_random_item(p);
            game.log(`${game.name(p)} gets a consolation item`);
        }
    }
}
```

**A rule that taxes the richest player at the start of each stage:**

```rhai
fn on_event(game, event) {
    if event.kind != "RoundStarted" || event.stage == game.get("taxed_stage") {
        return;
    }
    game.set("taxed_stage", event.stage);
    let richest = -1;
    for p in game.players() {
        if richest < 0 || game.gold(p) > game.gold(richest) {
            richest = p;
        }
    }
    if richest >= 0 && game.gold(richest) >= 20 {
        game.give_gold(richest, -5);
        game.log(`${game.name(richest)} pays 5 gold in tax`);
    }
}
```

## Limits

- Scripts can't touch files, the network or the clock.
- Numbers are whole numbers (no floating point).
- Randomness comes only from `game.random`, which uses the game's seed, so
  a scripted game replays exactly.
- A script that errors or runs too long (2 million operations per call) is
  stopped and reported as a `Script` event. The game carries on.
- Scripts can change the game around fights, but not fights themselves.
  For that, use [effects](effects.md) or a [Rust hook](effects.md#rust-hooks).

Turning off the `scripting` cargo feature builds the engine without the
Rhai interpreter. Content that adds scripts then fails to build.
