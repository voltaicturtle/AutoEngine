# AutoEngine

A Rust engine for building autochess games. The core is a headless,
deterministic simulation library; a server runs it and GUI clients render the
event stream it produces.

## Status

Steps 1 and 2 of the roadmap are in place: the combat core and data-driven content.

- `hex`: hex board (odd-r offset / axial coordinates), rings, lines, mirroring, deterministic pathfinding.
- `rng`: seedable xoshiro256** generator with integer-only helpers.
- `stats`: integer stats with flat and percentage modifiers.
- `effect`: plain-data building blocks for abilities and passives (effects, target selectors, triggers).
- `combat`: tick-based fights with targeting, movement, attacks, mana and casting, shields, statuses, triggered passives, summons, any number of teams, and a full event log.
- `content`: units, traits with breakpoints, items with recipes, star levels and team-wide bonuses, loaded from RON files and validated on load.

Still to come: the game loop (shop, economy, pluggable round types, bots),
the server, and the OpenGL client.

## Content

Game content lives in data files, not code. The `content/` folder holds a small
original sample set:

| File | Contents |
| --- | --- |
| `units.ron` | Units: cost, traits, stats, ability, passives |
| `traits.ron` | Traits and the bonuses each breakpoint grants |
| `items.ron` | Items: stat bonuses, passives, granted traits, recipes |
| `rules.ron` | Board size, item limit, star-level bonuses |
| `sample_match.ron` | Two boards to fight each other |

Abilities, trait bonuses and item effects are all written with the same
building blocks: an effect (damage, heal, shield, stun, stat change, summon,
...), a selector for who it hits, and for passives a trigger for when it fires.

## Try it

```
cargo run -- 7                    # run the sample match with seed 7 and print the log
cargo run -- 7 --quiet            # just the traits and the result
cargo run -- 7 --content DIR --match FILE
cargo test
```

The same seed always produces exactly the same fight.
