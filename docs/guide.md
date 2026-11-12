# Making a game, step by step

This walks through the tools in the order you will use them. Every command
is `autoengine ...`; from a checkout of this repository write
`cargo run -- ...` instead.

## 1. Start from the starter game

```
autoengine new mygame
```

This writes a small, complete game into `mygame/`: three units, two traits,
three items, three augments, six rounds, one script and two tests. Every file
starts with a comment explaining its format. `mygame/README.md` lists things
to try.

## 2. Check it

```
autoengine check mygame
autoengine check mygame --watch     # check and test again whenever a file is saved
```

Mistakes that stop the content loading are reported with the file and line.
`check` also warns about things that load but are probably wrong: trait
breakpoints no team can reach, traits no shop unit has, units whose cost
never appears in a shop, model files that are missing, and rounds that use a
kind nothing defines.

Leave `--watch` running in a terminal while you edit.

## 3. Watch a fight

```
autoengine fight --content mygame          # the fight in mygame/sample_match.ron
autoengine fight 7 --content mygame        # the same fight with other dice
```

This prints every move, hit, cast and death with its time. The same seed
always gives exactly the same fight.

## 4. Pin down what should happen

`tests.ron` holds fights with expected results:

```ron
(
    name: "two guards beat one",
    runs: 5,
    teams: [
        (units: [(unit: "shield_bearer", pos: (3, 4)), (unit: "field_medic", pos: (3, 6))]),
        (units: [(unit: "shield_bearer", pos: (3, 4))]),
    ],
    expect: [Winner(0), Survives(0, "shield_bearer"), LastsAtMost(seconds: 40)],
),
```

```
autoengine test mygame
autoengine test mygame --filter guards
```

A failing test says what was expected, what happened and with which seed, so
you can watch that exact fight:

```
autoengine fight 3 --content mygame --test guards
```

| Expectation | Meaning |
| --- | --- |
| `Winner(0)` | team 0 wins every run |
| `WinsAtLeast(team: 0, percent: 60)` | team 0 wins at least that share of the runs |
| `Draw` | nobody wins |
| `LastsAtMost(seconds: 30)`, `LastsAtLeast(seconds: 5)` | how long the fight takes |
| `Survives(0, "unit_id")`, `Dies(1, "unit_id")` | what becomes of a unit |
| `Casts(0, "unit_id")` | the unit gets to use its ability |

## 5. Play whole games

```
autoengine lobby --content mygame              # bots play one game; prints every round
autoengine sim --content mygame --games 50     # bots play many; prints a balance table
autoengine sim --content mygame --json         # the same numbers for a spreadsheet or script
```

`sim` shows, for each unit and trait, how often it fought, how often it won
and where boards that used it placed.

## 6. Add rules with a script

`mygame/scripts/bonus_gold.rhai` is a round type. Copy it, change it, and use
the new file's name as a round kind in `rounds.ron`. A script can also define
`on_event(game, event)` to react to anything that happens.

```
autoengine api        # everything scripts can call, and every event
```

More in [programming.md](programming.md).

## 7. See it

```
autoengine-server --content mygame --dev
autoengine-client --assets mygame
```

`--dev` turns on the cheat console (F1 in the client): `gold 50`,
`unit shield_bearer`, `item iron_ingot`, `skip`. Edit `mygame/theme.ron`
while the client runs to restyle it. Add models, icons and sounds when you
have them: see [art.md](art.md).

## 8. Keep it fast and stable

```
autoengine bench --content mygame             # fights and games per second
autoengine-server --content mygame --record replays/
autoengine replay replays/*.json              # do saved fights still play out the same?
```

## Content reference

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
