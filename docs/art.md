# For artists

Art files are read from the client's assets folder (`--assets DIR`, default
`content`) and reloaded while the client runs. Save the file and the change
shows up within a second. The look itself (colours, light, particle effects,
font) is code: `Theme::default()` in `client/src/theme.rs`.

| File or code | What it controls |
| --- | --- |
| `Theme::default()` in `client/src/theme.rs` | Interface and board colours, the scene light, every particle effect, the font file |
| `models/` | Unit models, in any format Assimp reads |
| `icons/<item id>.png` | Item icons (items without one get a lettered badge) |
| `sounds/<name>.wav` or `.ogg` | Replaces a built-in sound: `hit`, `cast`, `heal`, `death`, `buy`, `victory`, `defeat` |

## Asset viewer

```
cargo run -p autoengine-client -- --view content/models/reed_stalker.gltf
```

Shows one model on a tile with its triangle, vertex, bone and texture counts.
Number keys play its animation clips and letter keys fire each particle effect
from the theme, so a model or an effect can be checked without starting a game.
The model reloads when its file is saved.

## Keys

Keys are set in the client's settings file (`autoengine-client.json`):

```json
"keys": { "console": "Tab", "skip": "Enter" }
```

The actions are `menu`, `stats`, `skip`, `camera_reset` and `console`.

## Models and animation

See [client.md](client.md#3d-models) for how units name their models, which
formats work and how animation clips are picked by name.
