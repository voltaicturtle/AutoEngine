# AutoEngine

A Rust engine for building autochess games. The core is a headless,
deterministic simulation library. A headless server runs the game, and
clients contain only graphics and input: they send the player's commands and
draw what the server reports.

![Planning phase in the client](docs/client-planning.png)
![A fight being replayed in the client](docs/client-fight.png)

## Status

All five roadmap steps are in place: the combat core, data-driven content, the game loop, the server and graphical client, and play over the network.

- `hex`: hex board (odd-r offset / axial coordinates), rings, lines, mirroring, deterministic pathfinding.
- `rng`: seedable xoshiro256** generator with integer-only helpers.
- `stats`: integer stats with flat and percentage modifiers.
- `effect`: plain-data building blocks for abilities and passives (effects, target selectors, triggers).
- `combat`: tick-based fights with targeting, movement, attacks, mana and casting, shields, statuses, triggered passives, summons, any number of teams, and a full event log.
- `content`: units, traits with breakpoints, items with recipes, star levels, augments, rules and the round schedule, loaded from RON files and validated on load.
- `game`: the authoritative lobby state. Players send commands (buy, sell, reroll, buy XP, move, equip, pick, ready); the game handles the shared unit pool, shop odds by level, gold with interest and streaks, bench and board limits, star upgrades, item combining, damage and elimination, and reports everything as events.
- `round`: pluggable round types behind one trait. Built in: player fights (with ghost boards for odd lobbies), neutral fights with rewards, a shared draft in health order, and private choice rounds for augments or items. Games can register their own kinds.
- `bot`: simple computer players that use the same commands as people.

- `protocol`: the client and server messages, sent as one line of JSON each.
- `session`: a hosted game independent of any transport: seats, bots for empty or dropped seats, round timers, reconnecting with a seat token, spectators, and per-viewer filtering of private information.
- `net`: the TCP transport for both ends, using only the standard library.
- `client` and `playback`: the display-side model a client builds from server messages, with no rendering in it, so other front ends can reuse it.

Two binaries sit on top: `autoengine-server` (headless) and, in the `client/`
crate, `autoengine-client` (3D, on OpenGL through macroquad, with models
loaded through Assimp). The core library has no graphics dependencies.

## Play

```
cargo run --bin autoengine-server -- --seats 8          # terminal 1
cargo run -p autoengine-client -- --name You            # terminal 2
```

Press "Start game" in the client; empty seats are filled with bots. More
people can join first with their own `--name`, and `--spectate` watches
instead. To play across machines, start the server with `--bind 0.0.0.0` and
point clients at it with `--addr HOST:7777`.

In the client: click shop cards to buy, drag units between the bench and your
half of the board, drag a unit onto the shop to sell it, drag items onto
units, click a name on the scoreboard to look at that player's board, and
press Space to skip a fight replay. Drag with the right mouse button to turn
the camera, scroll to zoom, and press Home to reset it. If the connection drops, the client keeps
retrying and takes its seat back; a bot plays it in the meantime.

Server options: `--port`, `--bind`, `--seats`, `--seed`, `--planning-secs`,
`--content DIR`. `--autoplay` makes a client play by itself, which is handy
for demos.

### 3D models

The board is a 3D scene and every unit is a model. A unit names its model in
`units.ron`:

```ron
model: (file: "models/ironbark_warden.obj", scale_percent: 110, yaw_degrees: 0),
```

The file is looked up in the client's assets folder (`--assets DIR`, default
`content`) and read with [Assimp](https://www.assimp.org/) through the
`russimp` crate, so OBJ, glTF, FBX, PLY, Collada and the other formats Assimp
imports all work. The client uses the material's diffuse or base colour,
vertex colours, and a diffuse texture, either embedded or in a file next to
the model. Models are scaled to a standard height, centred and stood on the
ground, and should face +Z (use `yaw_degrees` for files that do not). A unit
with no model, or whose file cannot be read, gets a plain stand-in figure.

The models in `content/models` are simple original placeholders made by
`tools/make_sample_models.py`; they cover OBJ with materials, OBJ with a
texture, and PLY with vertex colours.

The first build downloads a ready-made Assimp library. `cargo run` finds it
without help. To run the executable directly, copy `libassimp` from
`target/<profile>/build/russimp-sys-*/out/dylib/lib` next to it, or install
Assimp and remove the `prebuilt` feature in `client/Cargo.toml`.

Known limits: connections are plain TCP with no encryption or accounts, so
host it only for people you trust. Models are static: skeletal animation in a
file is not played, and movement, attacks and hits are shown by moving,
turning and flashing the whole model. Lighting is a single fixed light worked
out per vertex. There is no sound yet.

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
| `units.ron` | Units: cost, traits, stats, ability, passives, 3D model |
| `models/` | The model files the units name |
| `traits.ron` | Traits and the bonuses each breakpoint grants |
| `items.ron` | Items: stat bonuses, passives, granted traits, recipes |
| `rules.ron` | Board size, item limit, star-level bonuses (economy, levels and shop odds use engine defaults unless set here) |
| `augments.ron` | Augments: lasting team-wide modifiers |
| `rounds.ron` | The round schedule |
| `sample_match.ron` | Two boards to fight each other |

Abilities, trait bonuses and item effects are all written with the same
building blocks: an effect (damage, heal, shield, stun, stat change, summon,
...), a selector for who it hits, and for passives a trigger for when it fires.

## Headless tools

```
cargo run -- lobby 7              # eight bots play a whole game; prints every round
cargo run -- lobby 7 --players 4 --quiet
cargo run -- fight 7              # run the sample match with seed 7 and print the log
cargo run -- fight 7 --quiet      # just the traits and the result
cargo run -- fight 7 --content DIR --match FILE
cargo test
```

The same seed always produces exactly the same fight, and the same game.
