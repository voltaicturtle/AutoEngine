# A new AutoEngine game

This crate is a complete, small game, written in Rust. The content (units,
traits, items, augments, rounds and content tests) is in `src/lib.rs`.

```
cargo run -- check                 # look for mistakes
cargo test                         # run the content tests
cargo run -- fight                 # watch one fight as text
cargo run -- lobby                 # bots play a whole game
cargo run -- sim                   # balance table from many bot games
cargo run -- serve --dev           # host it for clients
cargo run -- help                  # every command
```

Clients get the content from the server when they join, so
`autoengine-client` can play this game without being rebuilt.

## Things to try, in order

1. Change a number in `units()` and run `cargo test`.
2. Copy a unit, give it a new id, and add it to a fight in `tests()`.
3. Add a breakpoint to a trait in `traits()`. `cargo run -- check` tells
   you if no team could ever reach it.
4. Add a round to `schedule()`.
5. Change `scripts/bonus_gold.rhai`. `cargo run -- api` lists what scripts
   can do.
6. Write a round type or rule in Rust: see `RoundType` and `GameHook` in
   the engine's documentation (`cargo doc --open`).

Put models in `assets/models/` and point units at them with
`.model("models/knight.glb", 100)`, and add `assets/icons/<item id>.png`
and `assets/sounds/<name>.wav` when you have art. Until then the client draws
stand-ins.
