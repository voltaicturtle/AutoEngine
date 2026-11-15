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
| `item.ID.name`, `item.ID.description` | an item's name and description |
| `trait.ID.name`, `trait.ID.description` | a trait's name and description |
| `augment.ID.name`, `augment.ID.description` | an augment's name and description |

A key that names nothing fails the build, with a suggestion for typos.
Missing texts show in the original language, and `cargo run -- check` lists
them. Players choose a language in the client's settings file
(`"language": "fr"`). Translations change no rules, so saved matches still
replay after translating.
