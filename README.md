# AutoEngine

A Rust engine for building autochess games. The core is a headless,
deterministic simulation library. A headless server runs the game, and
clients contain only graphics and input: they send the player's commands and
draw what the server reports.

![Planning phase in the client](docs/client-planning.png)
![A fight being replayed in the client](docs/client-fight.png)

## Quick start

Games are pure Rust: content is defined with `ContentSet::builder()` (see
`src/sample.rs`), and every tool below runs on it.

```
cargo run -- check                      # look for mistakes in the sample game
cargo run -- test                       # run its content tests
cargo run -- lobby                      # bots play a whole game
cargo run -- new ../mygame              # write a starter game crate of your own

cargo run -- serve --dev                         # terminal 1: host the sample game
cargo run -p autoengine-client -- --name You     # terminal 2: play it
```

## Learn

| If you want to | Read |
| --- | --- |
| make a game | [docs/guide.md](docs/guide.md), a walk through every tool |
| write scripts or build on the library in Rust | [docs/programming.md](docs/programming.md) and `examples/` |
| make it look and sound like your game | [docs/art.md](docs/art.md) |
| play, host, or change the client | [docs/client.md](docs/client.md) |
| host games, write a client, or tune the server | [docs/networking.md](docs/networking.md) |
| look up a type or function | `cargo doc --open` |

## Tools

`cargo run -- help` lists these. They come from `autoengine::tools::cli`, so a
game's own crate gets all of them for its content by passing a `GameSpec` to
`cli::run` from its `main` (the starter game does this).

| Command | What it does |
| --- | --- |
| `new DIR` | writes a small, commented starter game crate |
| `check` | finds mistakes in the content |
| `test [--filter TEXT]` | runs the content tests and checks what was expected of them |
| `fight [SEED]` | runs one fight and prints everything that happens |
| `lobby [SEED]` | bots play a whole game |
| `sim [--games N] [--json]` | bots play many games and report win rates per unit and trait |
| `bench` | measures fights and games per second |
| `serve [--dev] [--record DIR] ...` | hosts the game for clients over TCP (`autoengine-server` is the same) |
| `replay FILE...` | fights saved replays again and reports any that changed |
| `api` | prints everything scripts can call |

In the client: a cheat console (F1, on servers started with `--dev`), a
statistics overlay (F3), an asset viewer (`--view FILE`) and live reload of
models and icons.

## What is in the box

- **Fights**: tick-based combat on a hex board with targeting, movement, mana and abilities, shields, statuses, triggered passives, summons, conditions, named stacks, knockback and leaps. Abilities, trait bonuses and item effects are data built from the same blocks, and Rust hooks cover anything the blocks can't express.
- **Content in code**: units, traits, items, augments, rounds and content tests are Rust, built with `ContentSet::builder()` and validated when built, with spelling suggestions for mistyped ids.
- **The game around them**: shared unit pool, shop odds by level, gold with interest and streaks, bench and board, star upgrades, item recipes, augments, elimination and placements.
- **Rounds**: player fights, neutral fights, a shared draft, choice rounds, and your own kinds from scripts or Rust, in whatever order the schedule says.
- **Hosting**: a headless server with bots for empty seats, reconnecting, spectators and per-player hiding of private information. The server decides every result and clients only send intents. Only changes are sent, fights travel as a seed the client replays, and slow, silent or flooding connections are dropped without affecting anyone else.
- **A 3D client**: models through Assimp, skeletal animation, particles, sound and settings. It gets each game's content from the server, so one client plays any game.
- **Determinism**: integer math and seeded dice, so the same seed always gives the same fight and the same game. Replays, content tests and the simulator rest on this.

The library is layered so you can use part of it, and the sample set in
`content/` is original. Run `cargo test --workspace` for the test suite.
