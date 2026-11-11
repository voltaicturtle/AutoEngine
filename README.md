# AutoEngine

A Rust engine for building autochess games. The core is a headless,
deterministic simulation library; a server runs it and GUI clients render the
event stream it produces.

## Status

Steps 1 to 3 of the roadmap are in place: the combat core, data-driven content, and the game loop.

- `hex`: hex board (odd-r offset / axial coordinates), rings, lines, mirroring, deterministic pathfinding.
- `rng`: seedable xoshiro256** generator with integer-only helpers.
- `stats`: integer stats with flat and percentage modifiers.
- `effect`: plain-data building blocks for abilities and passives (effects, target selectors, triggers).
- `combat`: tick-based fights with targeting, movement, attacks, mana and casting, shields, statuses, triggered passives, summons, any number of teams, and a full event log.
- `content`: units, traits with breakpoints, items with recipes, star levels, augments, rules and the round schedule, loaded from RON files and validated on load.
- `game`: the authoritative lobby state. Players send commands (buy, sell, reroll, buy XP, move, equip, pick, ready); the game handles the shared unit pool, shop odds by level, gold with interest and streaks, bench and board limits, star upgrades, item combining, damage and elimination, and reports everything as events.
- `round`: pluggable round types behind one trait. Built in: player fights (with ghost boards for odd lobbies), neutral fights with rewards, a shared draft in health order, and private choice rounds for augments or items. Games can register their own kinds.
- `bot`: simple computer players that use the same commands as people.

Still to come: the server binary and protocol, and the OpenGL client.

## Game loop

```text
begin_round()   income, shop refresh, the round type starts
apply(cmd)...   players act; invalid commands are rejected and change nothing
finish_round()  the round resolves, fights run, players without health are out
```

The schedule in `rounds.ron` decides which kind each round is, so a game can
mix fights with drafts, choices and its own custom rounds freely.

## Content

Game content lives in data files, not code. The `content/` folder holds a small
original sample set:

| File | Contents |
| --- | --- |
| `units.ron` | Units: cost, traits, stats, ability, passives |
| `traits.ron` | Traits and the bonuses each breakpoint grants |
| `items.ron` | Items: stat bonuses, passives, granted traits, recipes |
| `rules.ron` | Board size, item limit, star-level bonuses (economy, levels and shop odds use engine defaults unless set here) |
| `augments.ron` | Augments: lasting team-wide modifiers |
| `rounds.ron` | The round schedule |
| `sample_match.ron` | Two boards to fight each other |

Abilities, trait bonuses and item effects are all written with the same
building blocks: an effect (damage, heal, shield, stun, stat change, summon,
...), a selector for who it hits, and for passives a trigger for when it fires.

## Try it

```
cargo run -- lobby 7              # eight bots play a whole game; prints every round
cargo run -- lobby 7 --players 4 --quiet
cargo run -- fight 7              # run the sample match with seed 7 and print the log
cargo run -- fight 7 --quiet      # just the traits and the result
cargo run -- fight 7 --content DIR --match FILE
cargo test
```

The same seed always produces exactly the same fight, and the same game.
