# Rounds

A game is a series of rounds. Each round has a planning phase, when
players shop, place units, equip items and make any picks the round
offers, and then the round resolves: fights run, rewards are handed out,
and players who ran out of health are out.

The **schedule** in the content says what each round is. The engine has
five built-in kinds, and new kinds can be written as Rhai scripts or in
Rust.

## The schedule

```rust
use autoengine::content::{BoardUnit, ChoiceOffer, RoundDef, RoundKindDef, Schedule};

let golem = |pos| BoardUnit::new("moss_golem", pos);
let schedule = Schedule::new([
    RoundDef::new("1-1", 1, RoundKindDef::pve([golem((3, 4))]).items(1)),
    RoundDef::new("1-2", 1, RoundKindDef::pve([golem((2, 4)), golem((4, 4))]).gold(1).items(1)),
    RoundDef::new("1-3", 1, RoundKindDef::pvp()),
    RoundDef::new("2-1", 2, RoundKindDef::choice(3, ChoiceOffer::Augments)),
    RoundDef::new("2-2", 2, RoundKindDef::pvp()),
    RoundDef::new("2-3", 2, RoundKindDef::draft(1).max_cost(2)),
    RoundDef::new("2-4", 2, RoundKindDef::pvp()),
    RoundDef::new("2-5", 2, RoundKindDef::custom("payday").param("gold", 2)),
])
.repeat_from(4); // after 2-5, go back to 2-2, one stage higher each time
assert_eq!(schedule.rounds.len(), 8);
```

| Call | What it does |
| --- | --- |
| `Schedule::new([rounds])` | the rounds in order |
| `.repeat_from(index)` | when the list runs out, play again from this index, one stage higher each time. The default, 0, loops from the start |
| `RoundDef::new(name, stage, kind)` | one round. The name is shown to players. The stage scales how much a lost fight costs |

Without a schedule, a game is one PvP round, repeated. Add the schedule
with `ContentSet::builder().schedule(schedule)`, or replace one with
`content.with_schedule(schedule)`.

**Damage for a lost fight** is `GameRules::base_damage_by_stage` for the
round's stage (2, 3, 5, 8, 12 and 17 for stages 1 to 6, then one more for
each stage after) plus `damage_per_survivor` (1) for each enemy unit left
standing.

## Built-in round kinds

| Kind | Made with | What happens |
| --- | --- | --- |
| `pvp` | `RoundKindDef::pvp()` | players are paired up and fight. The loser takes damage, and a draw hurts both. With an odd number of players, the one left over fights a copy (a "ghost") of another player's board, and the copied player isn't affected. Pairings avoid recent opponents (`GameRules::avoid_rematch_rounds`) |
| `pve` | `RoundKindDef::pve([units]).gold(n).items(n).reward(Grant)` | everyone fights the same neutral board. Winners get the gold, `n` random components and every [grant](events.md#grants) added with `.reward(...)` (a currency, experience, health, a named item or augment), and losers take damage |
| `draft` | `RoundKindDef::draft(extra).max_cost(c)` | one shared set of units, each carrying a random component, is offered to everyone: one per player plus `extra`, at most cost `c`. Players pick one each in turn, lowest health first. Units nobody picks go back to the pool |
| `choice` | `RoundKindDef::choice(n, ChoiceOffer::Augments)` or `ChoiceOffer::Items` | each player privately picks one of `n` augments (ones they don't have) or items. Players can reroll the options `GameRules::choice_rerolls` times |
| custom | `RoundKindDef::custom("kind").param("name", value)` | a round type the game defines, as a script or in Rust (below). `param` passes numbers to it |

In every kind, anyone who hasn't made a pick when the round ends gets a
random one. Shopping and arranging the board work the same in every round.

**Every round starts the same way.** From the second round on, each player
receives `round_income` gold, interest (1 gold per `interest_per` gold
held, at most `interest_cap`), a streak bonus, `xp_per_round` experience
and whatever their augments pay every round. Shops refresh unless locked,
and then the round type's `begin` runs. Before a round resolves, empty board
places are filled from the bench if `GameRules::auto_fill_board` is on.

## A round type as a script

A script with a `finish(game)` function is a round type named after the
script. `begin(game)` is optional and runs when the round starts.

```rhai
// content/scripts/payday.rhai: everyone gets paid; whoever is worst off also finds an item.
fn begin(game) {
    game.log("Payday! Gold for everyone when the round ends.");
}

fn finish(game) {
    let poorest = -1;
    for p in game.players() {
        game.give_gold(p, game.param("gold") + game.stage());
        if poorest < 0 || game.health(p) < game.health(poorest) {
            poorest = p;
        }
    }
    if poorest >= 0 {
        game.give_random_item(poorest);
    }
}
```

Add it to the content and use it in the schedule:

```rust,ignore
ContentSet::builder()
    .script("payday", include_str!("../content/scripts/payday.rhai"))
    .schedule(Schedule::new([RoundDef::new("Payday", 3, RoundKindDef::custom("payday").param("gold", 2))]))
```

A script round can also run fights with `game.fight_all()`, which pairs
everyone up just as a PvP round does:

```rhai
// "bounty": a PvP round where the winner of each fight also gets 3 gold.
fn finish(game) {
    game.set("bounty", 1);
    game.fight_all();
}

fn on_event(game, event) {
    if event.kind == "RoundStarted" {
        game.set("bounty", 0);
    }
    if event.kind != "CombatFinished" || game.get("bounty") != 1 {
        return;
    }
    // `winner` is 0 when the home player won, 1 when the away side did, and () for a draw.
    if event.winner == 0 {
        game.give_gold(event.home, 3);
    } else if event.winner == 1 && type_of(event.away) == "map" && "Player" in event.away {
        game.give_gold(event.away.Player, 3);
    }
}
```

Every function scripts can call is in [Scripting](scripting.md).

## A round type in Rust

Use Rust when a round needs something scripts can't do, such as offering
players options to pick from. Implement `RoundType` and register a factory
for it on each game. Only `kind` and `finish` are required:

| Method | Called |
| --- | --- |
| `kind(&self) -> &str` | the kind's name, shown to clients |
| `begin(&mut self, game)` | once, when the round starts (after income and shops) |
| `awaiting(&self, game) -> Vec<PlayerId>` | which players still need to pick. The round isn't ready while anyone is listed |
| `options(&self, game, player) -> Vec<Option<String>>` | what a player is offered. `None` marks an option already taken |
| `pick(&mut self, game, player, option)` | handles `Command::Pick`. Return a `CommandError` to refuse |
| `reroll(&mut self, game, player)` | handles `Command::RerollPick` |
| `rerolls_left(&self, game, player) -> u32` | how many rerolls a player has left |
| `finish(&mut self, game)` | once, when the round ends. It must settle everything, including picking for anyone who didn't |

```rust
use std::collections::BTreeMap;

use autoengine::content::{RoundDef, RoundKindDef, Schedule};
use autoengine::prelude::*;

/// Each player chooses between gold now and a random item.
struct Spoils {
    gold: i64,
    waiting: BTreeMap<PlayerId, ()>,
}

impl RoundType for Spoils {
    fn kind(&self) -> &str {
        "spoils"
    }

    fn begin(&mut self, game: &mut Game) {
        self.waiting = game.alive().into_iter().map(|p| (p, ())).collect();
    }

    fn awaiting(&self, _game: &Game) -> Vec<PlayerId> {
        self.waiting.keys().copied().collect()
    }

    fn options(&self, _game: &Game, player: PlayerId) -> Vec<Option<String>> {
        if self.waiting.contains_key(&player) {
            vec![Some(format!("{} gold", self.gold)), Some("a random item".into())]
        } else {
            Vec::new()
        }
    }

    fn pick(&mut self, game: &mut Game, player: PlayerId, option: usize) -> Result<(), CommandError> {
        if self.waiting.remove(&player).is_none() {
            return Err(CommandError::NothingToPick);
        }
        match option {
            0 => game.add_gold(player, self.gold),
            1 => game.give_random_component(player),
            _ => {
                self.waiting.insert(player, ());
                return Err(CommandError::InvalidOption);
            }
        }
        Ok(())
    }

    fn finish(&mut self, game: &mut Game) {
        // Players who didn't choose take the gold.
        for player in std::mem::take(&mut self.waiting).into_keys() {
            game.add_gold(player, self.gold);
        }
    }
}

let schedule = Schedule::new([
    RoundDef::new("fight", 1, RoundKindDef::pvp()),
    RoundDef::new("spoils", 1, RoundKindDef::custom("spoils").param("gold", 4)),
]);
let content = autoengine::sample::content().with_schedule(schedule).unwrap();
let mut game = Game::new(content, 7, &["Ana", "Ben"]);
// The factory turns the round's params into a fresh round each time.
game.register_round_type(
    "spoils",
    Box::new(|params| Box::new(Spoils { gold: params.get("gold").copied().unwrap_or(3), waiting: BTreeMap::new() })),
);

game.begin_round().unwrap(); // the fight
game.finish_round().unwrap();
game.begin_round().unwrap(); // spoils
assert_eq!(game.options(PlayerId(0)).len(), 2);
let gold = game.player(PlayerId(0)).gold;
game.apply(PlayerId(0), Command::Pick { option: 0 }).unwrap();
assert_eq!(game.player(PlayerId(0)).gold, gold + 4);
game.finish_round().unwrap();
```

What a round type can do through `game`:

| Call | What it does |
| --- | --- |
| `alive()`, `players()`, `player(id)` | who is playing, and each player's gold, health, level, board, bench, items, augments and streak |
| `stage()`, `round_index()`, `round_name()` | where the game is |
| `rng()` | the game's dice: `below(n)`, `chance(a, b)`, `range_inclusive(lo, hi)`, `shuffle(&mut v)`, `weighted_index(&weights)`. Round types must use these for randomness so games can be replayed |
| `add_gold(id, n)`, `add_xp(id, n)` | gold (negative takes it away) and experience |
| `give_item(id, "item")`, `give_random_component(id)`, `component_items()` | items |
| `give_augment(id, "augment")` | an augment |
| `take_from_pool("unit")`, `add_unit(id, "unit", items)`, `put_in_pool("unit")`, `available_units(max_cost)` | units, and the shared pool they come from |
| `fight(home, Opponent::Player(b) / Ghost(b) / Neutral, &neutral_units)` | runs and records one fight, returning who won and how many units survived |
| `loss_damage(survivors)`, `damage_player(id, n)`, `record_result(id, won)` | damage for a lost fight, taking health, and updating streaks and the win bonus |
| `emit(GameEvent)` | reports something to clients and hooks |
| `round::pair_up(game, &players)` | pairs players as PvP rounds do, avoiding rematches |

## Where Rust round types must be registered

A Rust round type lives in the program, not in the content, so it has to
be registered on every `Game` that uses it:

- Your own code: `game.register_round_type("spoils", factory)` after
  `Game::new`.
- A hosted game: `session.on_start(|game| game.register_round_type(...))`.
- Replaying a saved match: register it in the `setup` closure that
  `MatchLog::replay` takes.

The `autoengine` command line tools (`lobby`, `sim`, `serve`) only know the
content, so they can't run a schedule that uses a Rust round type. Scripts
travel with the content and work everywhere. `cargo run -- check` warns
about rounds whose kind nothing defines.
