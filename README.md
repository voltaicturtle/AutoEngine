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
- `script`: round types and game rules written as Rhai scripts, loaded with the content.
- `tools`: the content checker and the balance simulator.

- `protocol`: the client and server messages, sent as one line of JSON each.
- `session`: a hosted game independent of any transport: seats, bots for empty or dropped seats, round timers, reconnecting with a seat token, spectators, and per-viewer filtering of private information.
- `net`: the TCP transport for both ends, using only the standard library.
- `replay`: fights saved to files, which can be watched again or re-run to check that the engine still plays them out identically.
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

Escape opens a menu with the settings, and F3 shows frame-rate and scene
statistics.

Server options: `--port`, `--bind`, `--seats`, `--seed`, `--planning-secs`,
`--content DIR`, `--record DIR`. `--autoplay` makes a client play by itself, which is handy
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

### Animation

A model file with animations keeps its skeleton, and the client plays its
clips by name: `idle` while standing, `walk` while moving, `attack` on each
attack and `cast` while casting. Names are matched without regard to case and
may be part of a longer name such as `Armature|Walk`. Missing clips are
simply not played. Skinning uses up to four bones per vertex and runs on the
CPU. Two of the sample units (`emberlash_duelist.gltf`, `reed_stalker.gltf`)
are rigged this way.

### Effects, sound and settings

Fights throw particles for hits, heals, casts and deaths, ranged attacks fire
a bolt, and units cast a soft shadow. Settings (name, server address, window
size, fullscreen, volume, and switches for particles, shadows and animations)
live in `autoengine-client.ron` in the folder the client is started from
(`--settings FILE` to use another) and are saved when changed in the menu.

Sound effects are synthesised by the client, so there are no sound files.
Playback is behind a cargo feature because it needs the system audio library
to build (ALSA development files on Linux):

```
cargo run -p autoengine-client --features audio
```

### Replays

`autoengine-server --record DIR` saves every fight as a JSON file holding the
content, both teams, the seed and the events.

```
cargo run -p autoengine-client -- --replay DIR/round-004-you-vs-bot-3.json   # watch it
cargo run -- replay DIR/*.json                                               # re-run and compare
```

The second command fights each recorded battle again and reports any whose
events no longer match, which makes saved replays a regression test for
balance-neutral engine changes.

Known limits: connections are plain TCP with no encryption or accounts, so
host it only for people you trust. Lighting is a single fixed light worked
out per vertex, and shadows are simple discs. Animation clips switch without
blending. The menu covers the game while it is open, and keys cannot be
rebound yet.

## Game loop

```text
begin_round()   income, shop refresh, the round type starts
apply(cmd)...   players act; invalid commands are rejected and change nothing
finish_round()  the round resolves, fights run, players without health are out
```

The schedule in `rounds.ron` decides which kind each round is, so a game can
mix fights with drafts, choices and its own custom rounds freely.

## For programmers

### Scripting

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

Rust code can do the same through the `GameHook` trait (`Game::add_hook`) and
the `RoundKind` trait for round types.

### Checking and balancing content

```
cargo run -- check                 # look for mistakes in content/ and its assets
cargo run -- sim --games 50        # bots play 50 games; prints a balance table
cargo run -- sim --games 20 --players 4 --seed 3 --content DIR
```

`check` reports things that load but are probably wrong: trait breakpoints no
team can reach, traits no shop unit has, units whose cost never appears in a
shop, model files that are named but missing, and rounds that use a kind
nothing defines. `sim` plays whole games with bots
and prints, for each unit and trait, how often it fought, its win rate and the
average placement of boards that used it.

### Cheats for testing

Start the server with `--dev` and press F1 in the client for a console:
`gold N`, `xp N`, `unit ID`, `item ID`, `augment ID`, `hurt N`, `skip`.
Servers without `--dev` refuse these.

## For artists

Everything below is read from the content folder and reloaded while the client
runs. Save the file and the change shows up within a second.

| File | What it controls |
| --- | --- |
| `theme.ron` | Interface and board colours, the scene light, every particle effect, the font |
| `models/` | Unit models, in any format Assimp reads |
| `icons/<item id>.png` | Item icons (items without one get a lettered badge) |
| `sounds/<name>.wav` or `.ogg` | Replaces a built-in sound: `hit`, `cast`, `heal`, `death`, `buy`, `victory`, `defeat` |

### Asset viewer

```
cargo run -p autoengine-client -- --view content/models/reed_stalker.gltf
```

Shows one model on a tile with its triangle, vertex, bone and texture counts.
Number keys play its animation clips and letter keys fire each particle effect
from `theme.ron`, so a model or an effect can be tuned without starting a game.
The model reloads when its file is saved.

### Keys

Keys are set in the client's settings file (`autoengine-client.ron`):

```ron
keys: { "console": "Tab", "skip": "Enter" }
```

The actions are `menu`, `stats`, `skip`, `camera_reset` and `console`.

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
| `scripts/` | Round types and rules written as scripts |
| `theme.ron`, `icons/`, `sounds/` | The look and sound of the client |
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
