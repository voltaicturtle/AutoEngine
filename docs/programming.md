# For programmers

There are three ways to build on the engine, from least to most code:

1. **Data.** Units, traits, items, augments and the round schedule are RON
   files. See [guide.md](guide.md).
2. **Scripts.** Round types and rules in `.rhai` files, loaded with the data.
3. **Rust.** The engine is a library. Use all of it, or only some layers.

## Scripting

Game rules can be written as [Rhai](https://rhai.rs) scripts in
`content/scripts/`, with no engine rebuild. A script can be a round type, a
rule that listens to events, or both.

```rhai
// content/scripts/payday.rhai: the file name is the round kind.
fn begin(game)  { game.log("Payday!"); }
fn finish(game) {
    for p in game.players() { game.give_gold(p, game.param("gold") + game.stage()); }
}

// content/scripts/second_wind.rhai: called for everything that happens.
fn on_event(game, event) {
    if event.kind == "PlayerDamaged" && event.health < 40 { game.give_gold(event.player, 8); }
}
```

A scripted round is used from `rounds.ron` as
`Custom(kind: "payday", params: {"gold": 2})`. Through `game` a script can read
players (`players`, `name`, `gold`, `health`, `level`, `unit_count`,
`item_count`, `streak`, `stage`, `round`, `param`), keep its own numbers
(`get`, `set`), roll dice (`random`), and act (`give_gold`, `give_xp`,
`give_item`, `give_random_item`, `give_unit`, `give_augment`, `damage`, `log`,
`fight_all`). Scripts use whole numbers and the game's seeded dice, so games
stay repeatable, and a runaway script is stopped rather than hanging the
server.

`autoengine api` prints every function and event. A test keeps that list in
step with the engine.


## Extending in Rust

| To add | Use | Example |
| --- | --- | --- |
| a round type | `RoundType` and `Game::register_round_type` | `examples/custom_rules.rs` |
| a rule that reacts to events | `GameHook` and `Game::add_hook` | `examples/custom_rules.rs` |
| either of those on a hosted game | `Session::on_start` | `examples/host_a_game.rs` |
| another transport than TCP | drive `Session` yourself | `examples/host_a_game.rs` |
| another front end | `ClientState` and `Playback` | the `client/` crate |

Run an example with `cargo run --example first_fight`. They are built by
`cargo test`, so they keep working. `use autoengine::prelude::*;` imports the
types most programs need, and `cargo doc --open` has the full reference.

## The game loop

```text
begin_round()   income, shop refresh, the round type starts
apply(cmd)...   players act; invalid commands are rejected and change nothing
finish_round()  the round resolves, fights run, players without health are out
```

The schedule in `rounds.ron` decides which kind each round is, so a game can
mix fights with drafts, choices and its own custom rounds freely.

## Layers

Each layer only uses the ones above it, so a project can take the fight
simulation alone, or everything up to the lobby, without the rest.

| Layer | Modules | What it gives you |
| --- | --- | --- |
| Foundations | `rng`, `hex`, `stats` | seeded dice, board geometry and pathfinding, integer stats |
| Fights | `effect`, `combat` | abilities as data and the tick-based fight simulation |
| Content | `content` | everything loaded from data files and validated |
| Game | `game`, `round`, `bot`, `script` | shop, economy, round loop, round types, scripts, computer players |
| Hosting | `protocol`, `session`, `net`, `replay` | messages, a hosted game with seats and timers, TCP, saved fights |
| Display | `client`, `playback` | what a front end needs to show a game, with no rendering in it |
| Tools | `tools` | the checker, content tests, simulator, starter game and benchmarks |

The graphical client is a separate crate (`client/`), so the library has no
graphics dependencies. Scripting is a cargo feature that is on by default:

```toml
autoengine = { package = "AutoEngine", path = "...", default-features = false }  # no script interpreter
```

Everything is deterministic: integer math and seeded dice, no clocks. The
same seed always gives the same fight and the same game, which is what makes
replays, content tests and the simulator possible.

## Cheats for testing

Start the server with `--dev` and press F1 in the client for a console:
`gold N`, `xp N`, `unit ID`, `item ID`, `augment ID`, `hurt N`, `skip`.
Servers without `--dev` refuse these.
