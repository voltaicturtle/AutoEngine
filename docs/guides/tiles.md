# Tiles

A tile is an effect that belongs to a hex instead of a unit: burning
ground, a shrine, a patch of healing mist, a boulder in the way. A game
describes each kind with a `TileDef`, and tiles get onto the board from
abilities, from rounds or from Rust code.

## A tile of your own

```rust
use autoengine::content::{ContentSet, RoundDef, RoundKindDef, Schedule, UnitDef};
use autoengine::effect::{Ability, Amount, Effect, Selector};
use autoengine::stats::Stat::*;
use autoengine::tile::TileDef;

let content = ContentSet::builder()
    // Burning ground that only hurts the enemies of whoever lit it.
    .tile(
        TileDef::new("embers", "Embers")
            .description("Burns enemies for 30 magic damage a second.")
            .enemies_only()
            .per_second(Effect::magic(Amount::power(30)))
            .color([235, 110, 45]),
    )
    // A shrine: whoever stands on it attacks faster.
    .tile(TileDef::new("shrine", "Shrine").mods([AttackSpeed.percent(150)]))
    // A boulder: nobody can stand there.
    .tile(TileDef::new("boulder", "Boulder").blocking())
    // An ability lays embers under its target and around it for 3 seconds.
    .unit(UnitDef::new("pyre", "Pyre").cost(1).stat(MaxMana, 60).ability(
        Ability::new("Scorch").effect(Effect::tile_for("embers", 3000).spread(1).to(Selector::CurrentTarget)),
    ))
    // A round whose fights start with a shrine on each side and a boulder
    // in the middle of the far half.
    .schedule(Schedule::new([RoundDef::new("1-1", 1, RoundKindDef::pvp()).tile("shrine", (3, 5)).tile_at("boulder", (3, 3))]))
    .build()
    .unwrap();
assert_eq!(content.tiles().len(), 3);
```

## `TileDef` calls

| Call | What it does |
| --- | --- |
| `TileDef::new(id, name)` | a tile that does nothing yet and affects everyone |
| `.description(text)` | the tooltip text |
| `.on_enter(Effect)` | happens to a unit when it steps onto the tile, or when the tile appears under it |
| `.on_leave(Effect)` | happens to a unit when it steps off the tile |
| `.per_second(Effect)` | happens once a second to the unit standing on it |
| `.mods([...])` | stat changes a unit has for as long as it stands on the tile |
| `.allies_only()` | only affects units on the side of whoever laid it |
| `.enemies_only()` | only affects the enemies of whoever laid it |
| `.blocking()` | nobody can stand on the hex, and units find their way around it |
| `.color([r, g, b])` | the color clients tint the hex with |

The effect calls can be repeated to add several effects. A tile's effects
have no selector: they happen to the unit on the tile. Their source is the
unit that laid the tile, so an amount like `Amount::power(30)` uses the
caster's ability power, and kills count for the caster. A tile nobody laid
(one a round starts with) has no side, so it affects everyone, and the unit
on it is the source of its own effects.

For anything wider than the unit on the tile, give the tile an
[`Effect::signal`](effects.md#signals) and let a passive answer it.

## Getting tiles onto the board

**From an ability or passive.** Any effect list can lay tiles:

| Constructor | What it does |
| --- | --- |
| `Effect::tile(id)` | lays the tile under each selected unit for the rest of the fight |
| `Effect::tile_for(id, ms)` | the same, for `ms` |
| `.spread(radius)` | added to either, also covers every hex within `radius` |

Unlike most effects these also work on a unit that just died, so an
`OnDeath` passive can leave something where its owner fell. Laying a tile
where the same kind already lies keeps the longer of the two times and
hands the tile to the new owner.

**From a round.** Every fight of the round starts with the tiles:

| Call on `RoundDef` | Where the tile goes |
| --- | --- |
| `.tile(id, (col, row))` | on that hex for both sides, as each side sees its own half, so neither has the better ground |
| `.tile_at(id, (col, row))` | on exactly that hex, as the home side sees the board |

`tile_at` suits rounds against a neutral board, where the player is always
the home side. In a fight between two players either may be the home side,
so use `tile` there unless the uneven ground is the point.

The server sends a round's tiles to every client (`GameView::round_tiles`),
and the client shows them while players place their units. A fight
outside a game takes its tiles from `MatchSetup::tile_at`,
`Scenario::tile_at` or `ContentSet::setup_combat_with`.

**From Rust.** A custom effect or a test can call
`combat.place_tile(id, hex, owner, duration_ms)` and
`combat.clear_tile(id, hex)`. `combat.tiles()` lists what is on the board,
`combat.has_tile(hex, id)` asks about one hex, and `combat.can_stand(hex)`
says whether a unit could be there.

## How tiles behave

- **Timing.** A tile's once-a-second effect first happens a second after it
  is laid, and a tile laid for two seconds gets two of them.
- **Standing.** Stat changes follow the unit exactly: they start when it
  steps on, and end when it steps off, dies or the tile runs out.
  `on_leave` only happens when the unit walks or is pushed off, not when
  the tile runs out or the unit dies.
- **Several tiles.** A hex can hold tiles of different kinds, and each
  works on its own.
- **Blocking.** A blocking tile is never laid on a hex a unit is standing
  on, so a round's boulder is left out of a fight where a player put a unit
  there. Units path around blocked hexes, and summons, leaps, knockbacks
  and revives never land on one. A wall with no gap leaves melee units
  with nothing to do until the fight times out, so leave a way through.
  A blocking tile cannot have effects or stat changes.
- **Movement.** Units do not avoid harmful tiles or seek helpful ones.
  They walk the shortest way to their target.

## Asking about tiles

`Condition::OnTile { who, tile }` is true when the unit stands on a tile of
that kind, whether or not the tile affects it:

```rust
use autoengine::effect::{Condition, Effect, Selector, Who};

// Extra damage to a target that is standing in embers.
let stoke = Effect::magic(120).to(Selector::CurrentTarget).when(Condition::OnTile { who: Who::Target, tile: "embers".into() });
assert!(stoke.condition.is_some());
```

## Events and drawing

Fights send `TilePlaced { tile, pos, team, duration_ms }` when a tile is
laid or laid again, and `TileRemoved { tile, pos }` when it goes.
`Playback::tiles()` gives the tiles on the board at the replay's current
time, which is what the client draws: a colored slab on the hex, or a block
for a blocking tile.

Tile names and descriptions can be translated with `tile.ID.name` and
`tile.ID.description`.

## What the checker catches

Building the content fails for an effect, condition or round that names a
tile that does not exist (with a suggestion for typos), a round tile off
the board, and a blocking tile with effects. `autoengine check` warns about
a tile nothing lays.
