# AutoEngine

A Rust engine for building autochess games. The core is a headless,
deterministic simulation library; a server runs it and GUI clients render the
event stream it produces.

## Status

Step 1 of the roadmap, the combat core, is in place:

- `hex`: hex board (odd-r offset / axial coordinates), rings, lines, mirroring, deterministic pathfinding.
- `rng`: seedable xoshiro256** generator with integer-only helpers.
- `stats`: integer stats with flat and percentage modifiers.
- `effect`: plain-data building blocks for abilities and passives (effects, target selectors, triggers).
- `combat`: tick-based fights with targeting, movement, attacks, mana and casting, shields, statuses, triggered passives, summons, any number of teams, and a full event log.

Still to come: content loaded from data files, the game loop (shop, economy,
pluggable round types, bots), the server, and the OpenGL client.

## Try it

```
cargo run -- 7          # run the sample fight with seed 7 and print the log
cargo run -- 7 --quiet  # just the result
cargo test
```

The same seed always produces exactly the same fight.
