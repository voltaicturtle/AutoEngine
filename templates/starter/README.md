# A new AutoEngine game

This folder is a complete, small game. Everything in it is data you can
change without rebuilding the engine. From the folder that contains it
(replace `mygame` with this folder's name):

```
autoengine check mygame            # look for mistakes
autoengine test mygame             # run the fights in tests.ron
autoengine fight --content mygame  # watch one fight as text
autoengine lobby --content mygame  # bots play a whole game
autoengine sim --content mygame    # balance table from many bot games
autoengine-server --content mygame --dev
autoengine-client --assets mygame
```

## Things to try, in order

1. Change a number in `units.ron` and run `autoengine test mygame`.
2. Copy a unit, give it a new `id`, and add it to a fight in `tests.ron`.
3. Add a breakpoint to a trait in `traits.ron`. `autoengine check` tells
   you if no team could ever reach it.
4. Add a round to `rounds.ron`.
5. Change `scripts/bonus_gold.rhai`. `autoengine api` lists what scripts
   can do.
6. Change a colour in `theme.ron` while the client is running.

## Files

| File | Contents |
| --- | --- |
| `units.ron` | Units: cost, traits, stats, ability |
| `traits.ron` | Traits and what each breakpoint grants |
| `items.ron` | Items and recipes |
| `augments.ron` | Augments offered in Choice rounds |
| `rules.ron` | Board size, item limit, star levels |
| `rounds.ron` | The order of rounds |
| `scripts/` | Round types and rules written as scripts |
| `tests.ron` | Fights with expected results |
| `theme.ron` | Colours, light and particle effects for the client |

Add `models/`, `icons/<item id>.png` and `sounds/<name>.wav` when you have
art. Until then the client draws stand-ins.
