# Testing content

Content changes all the time, and a small change can break a fight far
away. The engine has four ways to check it.

## Content tests

A content test is a fight with expected results. Put them in the function
your `GameSpec` lists with `.tests(...)`:

```rust
use autoengine::content::{BoardUnit, TeamSetup};
use autoengine::tools::scenario::{Expect, Scenario, run_all};

let tests = vec![
    Scenario::new(
        "two stars beat one star",
        [
            TeamSetup::new().unit(BoardUnit::new("ironbark_warden", (3, 4)).star(2)),
            TeamSetup::new().unit(BoardUnit::new("ironbark_warden", (3, 4))),
        ],
    )
    .runs(10)
    .expect([Expect::Winner(0), Expect::Survives(0, "ironbark_warden".into())]),
];

let results = run_all(&autoengine::sample::content(), &tests);
assert!(results.iter().all(|r| r.passed()), "{:?}", results);
```

| Call | What it does |
| --- | --- |
| `Scenario::new(name, [team0, team1])` | a fight between teams, numbered from 0 |
| `.runs(n)` | fights it `n` times with different seeds. Expectations must hold in every run unless they say otherwise |
| `.seed(s)` | the first run's seed. Later runs count up from it |
| `.expect([...])` | what should happen |

| `Expect` | Passes when |
| --- | --- |
| `Winner(team)` | that team wins every run |
| `WinsAtLeast { team, percent }` | that team wins at least that share of the runs |
| `Draw` | nobody wins any run |
| `LastsAtMost { seconds }` | every run is over within that time |
| `LastsAtLeast { seconds }` | every run lasts at least that long |
| `Survives(team, unit_id)` | a unit with that id on that team is alive at the end of every run |
| `Dies(team, unit_id)` | every unit with that id on that team dies in every run |
| `Casts(team, unit_id)` | a unit with that id on that team casts its ability in every run |

```sh
cargo run -- test                   # every test, with a report
cargo run -- test --filter stars    # the tests whose name contains "stars"
cargo run -- fight 3 --test stars   # watch one run, with seed 3
```

A failing test says what was expected, what happened and with which seed,
so you can watch that exact fight. `run_scenario` and `run_all` also work
from ordinary `#[test]` functions, so `cargo test` runs content tests with the
rest.

## Checks

`cargo run -- check` builds the content, which runs every check `build()`
makes, and warns about content that builds but is probably wrong:

- trait breakpoints no team can reach, and traits no shop unit has,
- unit costs that never appear in a shop,
- model and icon files that are missing from the assets folder,
- rounds whose kind nothing defines,
- texts a translation is missing.

From code: `autoengine::tools::lint(&content, Some(assets_dir))` returns
the same warnings.

## Whole games

```sh
cargo run -- lobby                     # bots play one game, printing every round
cargo run -- lobby --bots hard --save game.json
cargo run -- sim --games 400           # balance report from many games
cargo run -- sim --games 400 --bots hard --json
```

`sim` reports, for every unit and trait, how often it was fielded, how
often it won and where the boards that used it placed. It plays games on
every CPU core, and the report is the same however many cores there are.
From code: `autoengine::tools::simulate(content, games, players, seed)`.

Bots come in three levels. Hard bots play much better than normal ones, so
run `sim` with `--bots hard` to see how content holds up against strong
play.

## Replays

A saved fight or match must play out the same way after a change. If it
doesn't, something changed behaviour:

```sh
cargo run -- serve --record replays/     # keep every fight a server runs
cargo run -- replay replays/*.json       # do they still play out the same?
cargo run -- match game.json --rounds    # play a saved match back round by round
```

`MatchLog::replay` does the same from code and reports the first input that
no longer works, or that the game ended in a different state.
