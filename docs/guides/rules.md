# Rules

`GameRules` holds everything about a game that isn't a definition: the
board, the shop, the economy, levels, damage and how fights run. Every field
has a default, so set only what you change:

```rust
use autoengine::content::{ContentSet, EconomyRules, GameRules, UnitDef};

let rules = GameRules {
    starting_health: 80,
    bench_size: 8,
    economy: EconomyRules { starting_gold: 6, reroll_cost: 1, ..EconomyRules::default() },
    ..GameRules::default()
}
.board(8, 8)
.max_items_per_unit(2);

let content = ContentSet::builder().rules(rules).unit(UnitDef::new("grunt", "Grunt").cost(1)).build().unwrap();
assert_eq!(content.rules().starting_health, 80);
```

`ContentSet::builder().rules(rules)` sets them while building, and
`content.with_game_rules(rules)` swaps them on finished content. Both
check the rules.

## `GameRules`

| Field | Default | Meaning |
| --- | --- | --- |
| `board_cols`, `board_rows` | 7, 8 | board size, set with `.board(cols, rows)`. Each player owns the half with the higher row numbers |
| `max_items_per_unit` | 3 | items one unit can hold |
| `star_levels` | see below | stat modifiers for each star level, set with `.star_levels([...])`. Its length is the highest star level |
| `copies_to_upgrade` | 3 | copies of a unit at one star level that merge into the next. At least 2 |
| `shop_size` | 5 | offers in each shop |
| `bench_size` | 9 | bench slots |
| `starting_health` | 100 | each player's health at the start |
| `economy` | see below | gold and experience |
| `currencies` | empty | what players hold besides gold, added with `.currency(CurrencyDef)`; see [Currencies](#currencies) |
| `actions` | empty | things players can do during planning besides the built-in commands, added with `.action(ActionDef)`; see [Actions](#actions) |
| `sell_refund_permille` | 1000 | the share of a unit's price that selling it gives back, in thousandths. 1000 is all of it, 500 is half |
| `levels` | see below | one entry per player level |
| `pool_sizes` | 1: 29, 2: 22, 3: 18, 4: 12, 5: 10 | copies of each unit in the shared pool, by cost |
| `default_pool_size` | 10 | pool size for costs not in `pool_sizes` |
| `base_damage_by_stage` | 2, 3, 5, 8, 12, 17 | damage for losing a fight, by stage. Later stages add one per stage to the last entry |
| `damage_per_survivor` | 1 | extra damage for each enemy unit left standing |
| `avoid_rematch_rounds` | 2 | PvP pairings avoid anyone a player fought in their last this-many fights, when another pairing works. 0 turns it off |
| `auto_fill_board` | true | before fights, empty board places (up to the level's limit) are filled from the bench, strongest units first |
| `combat` | see below | how fights run (`CombatConfig`) |
| `planning_ms_by_kind` | empty | planning time for rounds of a kind, such as `("draft", 15000)`. Other rounds use the time the server is set to |
| `choice_rerolls` | 1 | times each player may reroll a choice round's options |
| `lobby_modifiers` | empty | augment ids. Each game, one picked at random goes to every player |

**Star levels.** The default is three levels. One star has no change. Two
stars gives +80% `MaxHealth` and +50% `AttackDamage` and `AbilityPower`.
Three stars gives +224% `MaxHealth` and +125% `AttackDamage` and
`AbilityPower`.

```rust
use autoengine::content::GameRules;
use autoengine::stats::Stat::*;

// Four star levels, each a flat +50% health and damage over the last.
let rules = GameRules::default().star_levels([
    vec![],
    vec![MaxHealth.percent(500), AttackDamage.percent(500)],
    vec![MaxHealth.percent(1000), AttackDamage.percent(1000)],
    vec![MaxHealth.percent(1500), AttackDamage.percent(1500)],
]);
assert_eq!(rules.star_levels.len(), 4);
```

## `EconomyRules`

| Field | Default | Meaning |
| --- | --- | --- |
| `starting_gold` | 4 | gold at the start |
| `round_income` | 5 | gold every round after the first |
| `interest_per`, `interest_cap` | 10, 5 | one extra gold per `interest_per` held, at most `interest_cap` |
| `win_bonus` | 1 | gold for winning a fight against another player |
| `streak_bonuses` | (2, 1), (4, 2), (6, 3) | `(streak length, gold)`: the longest streak reached pays each round. Win and loss streaks both count |
| `reroll_cost` | 2 | gold per shop reroll |
| `xp_cost`, `xp_per_purchase` | 4, 4 | price and amount of experience bought with `BuyXp` |
| `xp_per_round` | 2 | experience every round after the first |

## Currencies

Gold is built in: it buys units, rerolls and experience. A game can give
players other things to hold as well, such as souls, tokens or favor. The
engine keeps each player's balance, pays it out and shows it to clients.
What it buys is up to the game: a round type, a hook or a script charges
for it.

```rust
use std::sync::Arc;
use autoengine::content::{AugmentDef, ContentSet, CurrencyDef, RoundGrant};
use autoengine::game::{CommandError, Game, PlayerId};

let content = ContentSet::builder()
    // One soul to start with and two more every round.
    .currency(CurrencyDef::new("souls", "Souls").starting(1).per_round(2).color([150, 90, 200]))
    // An augment can pay a currency too.
    .augment(AugmentDef::new("reaper", "Reaper").per_round(RoundGrant::default().currency("souls", 3)))
    .build()
    .unwrap();

let mut game = Game::new(Arc::new(content), 1, &["a", "b"]);
let me = PlayerId(0);
assert_eq!(game.currency(me, "souls"), 1);
game.add_currency(me, "souls", 4);
// Spending takes nothing unless the player has enough.
assert_eq!(game.spend_currency(me, "souls", 9), Err(CommandError::NotEnough { currency: "souls".into(), need: 9, have: 5 }));
game.spend_currency(me, "souls", 5).unwrap();
assert_eq!(game.currency(me, "souls"), 0);
```

| `CurrencyDef` call | Meaning |
| --- | --- |
| `CurrencyDef::new(id, name)` | a currency nobody is given yet. The id `gold` is taken |
| `.starting(n)` | what every player starts with |
| `.per_round(n)` | what every player gets each round after the first, with their gold |
| `.color([r, g, b])` | the color front ends show it in |

| On `Game` | Does |
| --- | --- |
| `currency(player, id)` | how much the player has |
| `add_currency(player, id, n)` | gives some, or takes it away when negative, never below zero |
| `spend_currency(player, id, cost)` | takes `cost` if the player has it, and fails with `NotEnough` otherwise |

Every change sends a `CurrencyChanged` event, so hooks and scripts can
react. Scripts use `game.amount`, `game.give` and `game.take` (see
[Scripting](scripting.md)), `Grant::Currency` hands some out from outside
the rules, and the cheat console has `give CURRENCY N`. A player's balances
are in `Player::wallet`, and the client lists them under the gold.

These three calls take `"gold"` as well, so code that charges a price does
not need a special case for it.

## Actions

Buying, selling, rerolling and the rest are built in. An action is
something else a player can do during planning, defined by the game: trade
a currency for experience, pay for a blessing, sacrifice a unit. The client
shows every action as a button with its price, and bots use the ones that
only cost a currency other than gold.

```rust
use std::sync::Arc;
use autoengine::content::{ActionDef, ContentSet, CurrencyDef};
use autoengine::game::{Command, CommandError, Game, Grant, PlayerId};

let content = ContentSet::builder()
    .currency(CurrencyDef::new("favor", "Favor").starting(3))
    // Data only: pay the price, get what it gives.
    .action(ActionDef::new("petition", "Petition").description("Trade 3 favor for 4 experience.").costs("favor", 3).gives(Grant::Xp(4)))
    // With code: the player drops a unit on the button, and the handler
    // decides what happens.
    .action(ActionDef::new("offering", "Offering").description("Give up a unit for favor.").on_unit().per_round(1))
    .action_handler("offering", |game, player, uid| {
        let unit = game.remove_unit(player, uid as u32).ok_or(CommandError::UnknownUnit)?;
        let worth = game.unit_value(&unit);
        game.add_currency(player, "favor", worth);
        Ok(())
    })
    .build()
    .unwrap();

let mut game = Game::new(Arc::new(content), 1, &["a", "b"]);
game.begin_round().unwrap();
let me = PlayerId(0);
game.apply(me, Command::Action { id: "petition".into(), arg: 0 }).unwrap();
assert_eq!(game.currency(me, "favor"), 0);
// Not enough favor for a second one, so nothing happens.
assert!(game.apply(me, Command::Action { id: "petition".into(), arg: 0 }).is_err());
```

| `ActionDef` call | Meaning |
| --- | --- |
| `ActionDef::new(id, name)` | a free action that does nothing yet |
| `.description(text)` | what clients show when the mouse is over it |
| `.costs(currency, n)` | adds to the price. `"gold"` or a currency's id |
| `.gives(Grant)` | something the player gets each time; see [Grants](events.md#grants) |
| `.per_round(n)` | how often one player may use it in a round |
| `.on_unit()` | the action needs one of the player's own units. `arg` is the unit's `uid`, and the engine refuses a unit the player does not own |

When a player uses an action the engine checks the limit, the target and
the price, runs the handler if there is one, takes the price, hands over
what the action gives and sends an `ActionUsed` event. So an action can
work in three ways, alone or together:

- **Data.** `.gives(...)` needs no code.
- **A Rust handler**, added with `.action_handler(id, |game, player, arg| ...)`.
  It can refuse by returning an error, usually
  `CommandError::CannotUse("why".into())`, and then the player is not
  charged. A handler should refuse before it changes anything.
- **Hooks and scripts** reacting to `ActionUsed` in `on_event`. They run
  after the price is paid and cannot refuse.

`arg` is a number that travels with the command. For an action on a unit
it is the unit's `uid`. For other actions the bundled client sends 0, and a
game with its own client can use it for anything.

For a client, `Player::can_use(&action)` says whether a player can use an
action right now as far as the limit and the price go, and
`Player::actions_used` holds how often each was used this round.

## Helpers for game code

Round types, hooks and action handlers change the game through `Game`.
Next to `add_gold`, `add_xp`, `give_item`, `add_unit` and `damage_player`:

| On `Game` | Does |
| --- | --- |
| `heal_player(player, n)` | gives health back, up to `starting_health` |
| `remove_unit(player, uid)` | takes a unit away without paying for it. Its items go back to the inventory and its copies to the pool. Returns the unit |
| `unit_value(&unit)` | what a unit is worth: its price times the copies merged into it |
| `give(player, &Grant)` | hands over what a [grant](events.md#grants) describes, without recording it as an outside input |

## Levels

`levels` has one `LevelRule` per player level, starting at level 1:

| Field | Meaning |
| --- | --- |
| `xp_to_next` | experience to reach the next level. Ignored on the last level |
| `board_size` | units the player may field |
| `shop_odds` | relative chances of each cost in the shop, starting at cost 1. They don't need to add up to 100 |

| Level | XP to next | Board | Shop odds (cost 1 / 2 / 3 / 4 / 5) |
| --- | --- | --- | --- |
| 1 | 2 | 1 | 100 / 0 / 0 / 0 / 0 |
| 2 | 2 | 2 | 100 / 0 / 0 / 0 / 0 |
| 3 | 6 | 3 | 75 / 25 / 0 / 0 / 0 |
| 4 | 10 | 4 | 55 / 30 / 15 / 0 / 0 |
| 5 | 20 | 5 | 45 / 33 / 20 / 2 / 0 |
| 6 | 36 | 6 | 30 / 40 / 25 / 5 / 0 |
| 7 | 56 | 7 | 19 / 30 / 35 / 15 / 1 |
| 8 | | 8 | 15 / 20 / 35 / 25 / 5 |

```rust
use autoengine::content::{GameRules, LevelRule};

// A short game with three levels and three unit costs.
let level = |xp_to_next, board_size, odds: [u32; 3]| LevelRule { xp_to_next, board_size, shop_odds: odds.to_vec() };
let rules = GameRules {
    levels: vec![level(4, 2, [80, 20, 0]), level(8, 4, [50, 40, 10]), level(0, 6, [30, 40, 30])],
    ..GameRules::default()
};
assert_eq!(rules.levels.len(), 3);
```

## The shop and the pool

Every shop unit has a limited number of copies (`pool_sizes`) shared by all
players. Buying takes a copy out, and selling or being eliminated puts
copies back. Offers in a shop or a draft are held out of the pool until they
are bought or replaced. A shop never offers a unit with no copies left,
and clients show how many copies are left on each card.

## Fights: `CombatConfig`

| Field | Default | Meaning |
| --- | --- | --- |
| `tick_ms` | 50 | length of one simulation step |
| `max_duration_ms` | 60000 | a fight still going after this long is a draw |
| `resources` | mana | what units build up and spend to cast. Set with the builder's `.resource(...)`; see [Resources](resources.md) |
| `damage_types` | physical, magic, true | how each kind of damage is mitigated. Set with the builder's `.damage_type(...)`; see [Damage types](damage.md) |
| `resist_constant` | 100 | damage after armor or magic resist is `damage × k / (k + resist)` |
| `max_damage_reduction` | 900 | cap on the `DamageReduction` stat |
| `overtime_after_ms` | 30000 | from this point, all damage grows every second. 0 turns overtime off |
| `overtime_damage_per_second` | 250 | thousandths of extra damage for each second of overtime |

These travel with the content, so clients replay fights with the same
rules. [Fights](fights.md) explains how they are used.

## Translations

Names and descriptions can be given in other languages:

```rust
use autoengine::content::{ContentSet, TraitDef, UnitDef, Bonus};
use autoengine::stats::Stat::*;

let content = ContentSet::builder()
    .trait_def(TraitDef::new("ember", "Ember").description("Ember units attack faster.").breakpoint(2, [Bonus::members().mods([AttackSpeed.percent(200)])]))
    .unit(UnitDef::new("ashen_hound", "Ashen Hound").cost(1).traits(["ember"]))
    .translate("fr", "unit.ashen_hound.name", "Chien de cendre")
    .translations("fr", [("trait.ember.name", "Braise"), ("trait.ember.description", "Les unités Braise attaquent plus vite.")])
    .build()
    .unwrap();
assert_eq!(content.name_in(Some("fr"), "ashen_hound").as_deref(), Some("Chien de cendre"));
```

| Key | Text |
| --- | --- |
| `unit.ID.name` | a unit's name |
| `unit.ID.ability` | its ability's name |
| `unit.ID.ability_description` | its ability's description |
| `unit.ID.ability_2`, `unit.ID.ability_2_description` | the same for a second ability, and so on |
| `resource.ID.name` | a [resource](resources.md)'s name |
| `currency.ID.name` | a [currency](#currencies)'s name |
| `action.ID.name`, `action.ID.description` | an [action](#actions)'s name and description |
| `tile.ID.name`, `tile.ID.description` | a [tile](tiles.md)'s name and description |
| `item.ID.name`, `item.ID.description` | an item's name and description |
| `trait.ID.name`, `trait.ID.description` | a trait's name and description |
| `augment.ID.name`, `augment.ID.description` | an augment's name and description |

A key that names nothing fails the build, with a suggestion for typos.
Missing texts show in the original language, and `cargo run -- check` lists
them. Players choose a language in the client's settings file
(`"language": "fr"`). Translations change no rules, so saved matches still
replay after translating.
