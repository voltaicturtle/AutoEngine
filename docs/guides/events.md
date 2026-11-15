# Events and commands

Everything that happens is reported as an event. There are two streams:

- **Game events** (`GameEvent`) cover everything outside fights: rounds,
  the shop, gold, units, items, picks and eliminations. Rules react to
  them, through Rhai `on_event` functions or Rust `GameHook`s.
- **Fight events** (`EventKind`, stamped with a tick in `Event`) cover every
  step of a fight. They are kept in each fight's `CombatRecord` so clients
  can play fights back, and they are only read, never reacted to. Reacting
  to something inside a fight is a job for [passives](effects.md#triggers).

Players change the game with **commands**. A command is either carried out
completely or refused with a `CommandError` that changes nothing.

## Reacting to game events

**In Rhai**, any script can define `on_event(game, event)`. `event.kind` is
the event's name, and its fields sit beside it:

```rhai
// The first time a player drops below 40 health, they get 8 gold.
fn on_event(game, event) {
    if event.kind == "PlayerDamaged" && event.health < 40 && event.health > 0 {
        let key = `second_wind_${event.player}`;
        if game.get(key) == 0 {
            game.set(key, 1);
            game.give_gold(event.player, 8);
        }
    }
}
```

Players are numbers (their seat). Fields that are Rust enums arrive as
maps: `UnitMoved`'s `to` is `#{Bench: 2}` or `#{Board: [3, 5]}`, and
`CombatFinished`'s `away` is `"Neutral"`, `#{Player: 1}` or `#{Ghost: 1}`.
An absent value, such as the `winner` of a draw, is `()`.

**In Rust**, implement `GameHook` and add it to the game:

```rust
use autoengine::prelude::*;

/// Every time a unit reaches three stars, its owner gets 5 gold.
struct StarBounty;

impl GameHook for StarBounty {
    fn on_event(&mut self, game: &mut Game, event: &GameEvent) {
        if let GameEvent::UnitUpgraded { player, star: 3, .. } = event {
            game.add_gold(*player, 5);
        }
    }
}

let mut game = Game::new(autoengine::sample::content(), 1, &["Ana", "Ben"]);
game.add_hook(Box::new(StarBounty));
```

Hooks run after the command or round step that caused the event has
finished, so they always see a settled game. Events a hook causes are
delivered too, up to a limit that stops hooks from feeding each other
forever. Scripts don't receive `Script` events, which keeps them from
looping on their own log lines.

To read events without reacting to them, use `game.events()` (all of them)
or `game.drain_events()` (the ones since the last call).

## Game events

| Event | Fields | When |
| --- | --- | --- |
| `RoundStarted` | `index`, `name`, `kind`, `stage` | a round starts. `index` counts from 0, and `kind` is the round type's name (`"pvp"`, `"draft"`, a script's name, ...) |
| `RoundFinished` | `index` | a round has resolved, before eliminations |
| `ShopRolled` | `player`, `offers` | a player's shop got new offers: at the start of a round, or after a reroll. `offers` holds unit ids, with `None` (`()` in Rhai) for empty slots |
| `GoldChanged` | `player`, `gold` | a player's gold changed. `gold` is the new total |
| `CurrencyChanged` | `player`, `currency`, `amount` | a player's balance of a currency other than gold changed. `amount` is the new total |
| `XpChanged` | `player`, `level`, `xp` | a player gained experience. These are the new level and experience |
| `UnitGained` | `player`, `uid`, `unit` | a player got a unit: bought, drafted, granted or duplicated. `uid` identifies this copy for the rest of the game |
| `UnitSold` | `player`, `uid`, `unit` | a unit was sold |
| `UnitUpgraded` | `player`, `uid`, `unit`, `star` | copies merged. `uid` is the unit that was kept, and `star` its new star level |
| `UnitMoved` | `player`, `uid`, `to` | a unit moved to a bench slot or a board place |
| `ItemGained` | `player`, `item` | an item went into a player's inventory |
| `ItemEquipped` | `player`, `uid`, `item` | an item was given to a unit |
| `ItemCombined` | `player`, `uid`, `item` | two items on a unit combined into `item` |
| `ItemUsed` | `player`, `uid`, `item` | a single-use item was used on a unit |
| `ChoiceOffered` | `player`, `options` | a round offered options: to one player, or to everyone when `player` is `None` (drafts) |
| `ChoiceMade` | `player`, `option`, `choice` | a player picked, or was given, option number `option`, which is `choice` |
| `AugmentGained` | `player`, `augment` | a player got an augment |
| `LobbyModifier` | `augment` | at the start of the game: the augment every player got (see [Augments](augments.md)) |
| `PlayerReady` | `player` | a player said they are done planning |
| `CombatFinished` | `combat`, `home`, `away`, `winner` | a fight finished. `combat` indexes `game.combats()` for the full record, and `winner` is `0` (home), `1` (away) or `None` for a draw |
| `PlayerDamaged` | `player`, `amount`, `health` | a player lost health. `health` is what is left |
| `PlayerForfeited` | `player` | a player gave up. They leave when the round ends |
| `PlayerEliminated` | `player`, `placement` | a player is out, finishing in `placement` (1 is the winner) |
| `GameOver` | `winner` | the game ended |
| `Script` | `script`, `message` | a script logged a line with `game.log`, or hit an error. Clients show these in the log |

## Fight events

Each `Event` has a `tick` and a `kind`. The time in milliseconds is
`tick × CombatRecord::tick_ms`. Units are `UnitId`s, numbered in the order
they appeared. `Spawned` says which unit each one is.

| `EventKind` | Fields | When |
| --- | --- | --- |
| `Spawned` | `unit`, `team`, `pos`, `name`, `max_health` | a unit entered the fight: at the start, or summoned |
| `Moved` | `unit`, `from`, `to` | a unit walked one hex |
| `Displaced` | `unit`, `from`, `to` | an effect moved a unit: knockback, pull, leap or a hook |
| `Attacked` | `source`, `target` | a basic attack was made |
| `Dodged` | `source`, `target` | the attack missed because the target dodged |
| `Damaged` | `source`, `target`, `amount`, `absorbed`, `kind`, `crit` | damage landed. `amount` is health lost and `absorbed` is what shields took |
| `Healed` | `source`, `target`, `amount` | health restored. `source` is the healer, or the unit itself for lifesteal |
| `Shielded` | `source`, `target`, `amount` | a shield was given |
| `CastStarted` | `unit`, `ability` | a unit started casting an ability. `ability` is its position among the unit's abilities, starting at 0 |
| `CastFinished` | `unit`, `ability` | the cast finished and its effects happened |
| `PassiveTriggered` | `unit`, `passive` | a passive fired. `passive` is its index in the unit's template |
| `StatusApplied` | `source`, `target`, `status`, `duration_ms` | a status took hold, for this long after tenacity |
| `StatusExpired` | `target`, `status` | a status ran out |
| `StacksChanged` | `unit`, `stack`, `count` | a stack changed. `count` is the new count |
| `ResourceChanged` | `unit`, `resource`, `amount`, `max` | a unit now has `amount` of a [resource](resources.md) out of `max`. Sent when either changes, and for each resource a unit has when it enters the fight |
| `Overtime` | | overtime began, and damage grows from now on (see [Fights](fights.md#overtime)) |
| `Died` | `unit`, `killer` | a unit died |
| `Ended` | `winner`, `timed_out` | the fight ended. `winner` is the team left standing, or `None` for a draw |

`playback::fight_stats(&record.events)` adds these up into each unit's
damage dealt and taken, healing, shielding, kills, casts and dodges, which
is what damage meters show.

## Commands

Players send commands with `game.apply(player, command)`. Bots and
networked clients go through the same call, so every rule is enforced in
one place.

| `Command` | What it does |
| --- | --- |
| `BuyUnit { slot }` | buys the unit in a shop slot (0-based) |
| `SellUnit { uid }` | sells a unit for its cost times the number of one-star copies in it (1, 3 or 9), and returns its items to the player |
| `Reroll` | new shop offers, for `reroll_cost` gold or a free reroll |
| `BuyXp` | `xp_per_purchase` experience for `xp_cost` gold |
| `SetShopLock { locked }` | keeps the current offers for the next round |
| `MoveUnit { uid, to }` | moves a unit to `Slot::Bench(i)` or `Slot::Board(col, row)`. If the place is taken, the two units swap |
| `EquipItem { item, uid }` | gives item number `item` from the player's inventory to a unit, or uses it if it is single-use |
| `Pick { option }` | picks an option the round offers |
| `RerollPick` | swaps the round's options for new ones, where the round allows it |
| `Ready` | done planning. A hosted game ends planning early when everyone is ready |
| `Forfeit` | gives up. The player leaves when the round ends, placed below everyone else who goes out that round. This can't be undone |

| `CommandError` | Meaning |
| --- | --- |
| `NotPlanning` | no round is in progress |
| `UnknownPlayer` | no such player |
| `Eliminated` | the player is out |
| `Forfeited` | the player has forfeited and is only waiting for the round to end |
| `GameOver` | the game is over |
| `NotEnoughGold { need, have }` | can't afford it |
| `NotEnough { currency, need, have }` | not enough of a currency other than gold (from `spend_currency`) |
| `UnknownCurrency` | the game has no currency with that id |
| `EmptyShopSlot` | nothing in that shop slot |
| `SoldOut` | no copies of that unit are left in the pool |
| `BenchFull` | no room on the bench |
| `BoardFull` | the board is full for the player's level |
| `UnknownUnit` | the player owns no unit with that `uid` |
| `InvalidSlot` | not a valid place, such as the enemy's half of the board |
| `UnknownItem` | no item with that number in the inventory |
| `ItemSlotsFull` | the unit can't hold more items |
| `CannotUse(reason)` | a single-use item can't do anything to that unit, for the reason given |
| `MaxLevel` | already at the highest level |
| `NothingToPick` | the round has nothing to pick |
| `NotYourTurn` | in a draft, it isn't this player's turn |
| `InvalidOption` | that option doesn't exist or is taken |
| `NoRerollsLeft` | no rerolls of the options are left |
| `UnknownRoundKind(kind)` | the schedule names a round kind nothing registered (from `begin_round`) |

## Grants

To hand a player something from outside the rules, such as an admin tool, a
tournament reward or a cheat, use `game.grant(player, Grant::...)`.
Grants are recorded with the game's inputs, so a saved match that used them
still replays. Round types and hooks can call `add_gold` and the like
directly, because they run again on replay.

| `Grant` | Gives |
| --- | --- |
| `Gold(n)` | gold |
| `Currency { currency, amount }` | some of a currency other than gold. A negative amount takes it away |
| `Xp(n)` | experience |
| `Unit(id)` | a one-star unit, without taking a copy from the pool |
| `Item(id)` | an item |
| `Augment(id)` | an augment |
| `Damage(n)` | takes health |

Every input that moves a game along (`Input::BeginRound`, `FinishRound`,
accepted commands and grants) is recorded in `game.inputs()`.
`MatchLog::of(&game)` saves them with the seed and player names, which is
enough to rebuild the game exactly.
