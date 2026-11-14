# The client and server

## Playing

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
`--record DIR`, `--dev` (see `cargo run -- help`). The server hosts the content its binary was built
with; a game crate hosts its own with `cargo run -- serve`. `--autoplay` makes a client play by itself, which is handy
for demos.

## 3D models

The board is a 3D scene and every unit is a model. A unit names its model in
its definition:

```rust
UnitDef::new("ironbark_warden", "Ironbark Warden").model("models/ironbark_warden.obj", 110)
```

Set `ModelRef::yaw_degrees` on `unit.model` for files that face another way.

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

## Animation

A model file with animations keeps its skeleton, and the client plays its
clips by name: `idle` while standing, `walk` while moving, `attack` on each
attack and `cast` while casting. Names are matched without regard to case and
may be part of a longer name such as `Armature|Walk`. Missing clips are
simply not played. Skinning uses up to four bones per vertex and runs on the
CPU. Two of the sample units (`emberlash_duelist.gltf`, `reed_stalker.gltf`)
are rigged this way.

## Effects, sound and settings

Fights throw particles for hits, heals, casts and deaths, ranged attacks fire
a bolt, and units cast a soft shadow. Settings (name, server address, window
size, fullscreen, volume, and switches for particles, shadows and animations)
live in `autoengine-client.json` in the folder the client is started from
(`--settings FILE` to use another) and are saved when changed in the menu.

Sound effects are synthesised by the client, so no sound files are needed
(a file in `sounds/` replaces one, see [art.md](art.md)).
Playback is behind a cargo feature because it needs the system audio library
to build (ALSA development files on Linux):

```
cargo run -p autoengine-client --features audio
```

## Replays

`autoengine-server --record DIR` saves every fight as a JSON file holding the
content, both teams, the seed and the events.

```
cargo run -p autoengine-client -- --replay DIR/round-004-you-vs-bot-3.json   # watch it
cargo run -- replay DIR/*.json                                               # re-run and compare
```

The second command fights each recorded battle again and reports any whose
events no longer match, which makes saved replays a regression test for
balance-neutral engine changes.

## Known limits

Connections are plain TCP with no encryption or accounts, so host it only
for people you trust. Lighting is a single light worked out per vertex, and
shadows are simple discs. Animation clips switch without blending. The menu
covers the game while it is open.

## How the client's code is laid out

The client contains no game rules. Each file in `client/src/` has one job:

| File | Job |
| --- | --- |
| `main.rs` | start-up, the main loop, networking, the lobby and the panels around the board |
| `planning.rs` | the planning phase: board, bench, shop, dragging, pick overlays |
| `fight.rs` | playing back a fight in the 3D scene |
| `assets.rs` | loading and reloading models and icons |
| `viewer.rs` | the asset viewer |
| `menu.rs`, `console.rs` | the settings menu, statistics overlay and developer console |
| `scene.rs`, `draw.rs`, `fx.rs` | the camera and ground, flat interface drawing, particles |
| `model.rs` | Assimp import, skinning and animation |
| `theme.rs`, `settings.rs`, `sound.rs` | the theme (colours, light, effects, font), the settings file and key bindings, sound effects |
