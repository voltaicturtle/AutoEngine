# Perks and loot

Bonuses change how a player's units fight. This guide is about everything
around the fights: what augments, traits and items hand out and when, the
rules they bend, loot, picks a player is offered outside a choice round,
what units earn in fights, team play and special shop offers.

## Perks

A `Perk` hands the player [grants](events.md#grants) when an occasion
comes up. Augments have their perks for as long as the player holds them,
a trait tier while it is active on the board, and an item while a unit on
the board holds it.

```rust
use std::sync::Arc;
use autoengine::content::{AugmentDef, Bonus, ContentSet, CurrencyDef, TraitDef, UnitDef};
use autoengine::game::{Command, Game, Grant, PlayerId};
use autoengine::perk::{Perk, When};

let content = ContentSet::builder()
    .currency(CurrencyDef::new("tokens", "Tokens"))
    .unit(UnitDef::new("scout", "Scout").cost(1).traits(["rogues"]))
    // A trait that pays out when its owner loses: an economy trait.
    .trait_def(
        TraitDef::new("rogues", "Rogues")
            .breakpoint(1, [Bonus::members()])
            .perk(Perk::on(When::Lost).gives(Grant::Gold(2)).gives(Grant::Currency { currency: "tokens".into(), amount: 1 })),
    )
    // An augment that gives experience every time its holder rerolls.
    .augment(AugmentDef::new("study", "Study").perk(Perk::on(When::Reroll).gives(Grant::Xp(1))))
    .build()
    .unwrap();
let mut game = Game::new(Arc::new(content), 1, &["a", "b"]);
game.begin_round().unwrap();
let me = PlayerId(0);
game.give_augment(me, "study");
let xp = game.player(me).xp;
game.add_gold(me, 10);
game.apply(me, Command::Reroll).unwrap();
assert_eq!(game.player(me).xp, xp + 1);
```

| `When` | Happens when |
| --- | --- |
| `Gained` | the player picks up the augment. Only for augments |
| `RoundStart` | a round starts, from the second round on |
| `Won` | the player won a fight, against a player or a neutral board |
| `Lost` | the player lost or drew a fight |
| `Fought` | the player's fight ended, however it went |
| `LevelUp` | the player reached a new level (once per level) |
| `Reroll` | the player rerolled the shop |
| `BuyXp` | the player bought experience |
| `UnitSold` | the player sold a unit |
| `StarUp` | one of the player's units reached a higher star level |

`Perk::on(when).gives(grant)` can be given several grants, and
`.chance(permille)` makes it happen only some of the time. Add perks with
`AugmentDef::perk`, `ItemDef::perk` and `TraitDef::perk` (which adds to the
tier added last). Round types and hooks can set off a player's perks with
`game.fire(player, when)`, and `game.perks_and_rules(player)` lists what a
player has right now.

## Rules a player can bend

A `RuleMod` changes one rule for one player: `Rule::TeamSize.add(1)`. They
come from the same three places as perks, with `.rule(...)`.

| `Rule` | The game's value it changes |
| --- | --- |
| `TeamSize` | how many team slots the board has at the player's level |
| `RerollCost` | the price of a reroll |
| `XpCost` | the price of buying experience |
| `XpPerPurchase` | experience per purchase |
| `XpPerRound` | experience at the start of each round |
| `RoundIncome` | gold at the start of each round |
| `InterestCap` | the most interest a round pays |
| `WinBonus` | gold for beating a player |
| `DamageTaken` | added to each bit of damage the player takes; negative shields them. Never below 0 |
| `ShopLevel` | the shop rolls as if the player were this many levels higher (or lower) |

Prices never go below 0. `game.rule(player, rule)` gives the value for a
player now, and `game.board_limit(player)` is `TeamSize`.

```rust
use std::sync::Arc;
use autoengine::content::{AugmentDef, ContentSet, ItemDef};
use autoengine::game::{Game, PlayerId};
use autoengine::perk::Rule;

let content = ContentSet::builder()
    .augment(AugmentDef::new("big_plans", "Big Plans").rule(Rule::TeamSize.add(1)).rule(Rule::RerollCost.add(-1)))
    .item(ItemDef::new("banner", "War Banner").rule(Rule::TeamSize.add(1)))
    .build()
    .unwrap();
let mut game = Game::new(Arc::new(content), 1, &["a", "b"]);
let me = PlayerId(0);
let (slots, reroll) = (game.board_limit(me), game.rule(me, Rule::RerollCost));
game.give_augment(me, "big_plans");
assert_eq!((game.board_limit(me), game.rule(me, Rule::RerollCost)), (slots + 1, reroll - 1));
```

## Loot tables

A `LootTable` is a weighted list of outcomes. `Grant::Loot(id)` rolls on it
once and gives everything in the outcome that comes up. An outcome may roll
on another table.

```rust
use autoengine::content::ContentSet;
use autoengine::game::Grant;
use autoengine::perk::LootTable;

let content = ContentSet::builder()
    .loot(
        LootTable::new("orb")
            .entry(6, [Grant::Gold(2)])
            .entry(3, [Grant::Component])
            .entry(1, [Grant::Gold(5), Grant::Loot("orb".into())])
            .nothing(2),
    )
    .build()
    .unwrap();
assert_eq!(content.loot("orb").unwrap().entries.len(), 4);
```

Loot that rolls on itself stops after a few rolls, so a table can never
hand out forever. `autoengine check` warns about a table nothing rolls on.

## Picks of a player's own

`Grant::Choice { options, offer }` puts a pick of one out of `options` in
front of one player, outside any choice round: an augment that lets its
holder pick a component, an item that opens a choice of emblems.
`game.awaiting()` lists the player, `game.options(player)` shows the
options, and `Command::Pick` answers the oldest open pick (after any pick
the round itself is waiting for). Picks still open when the round ends are
made at random. `Player::choices` holds them.

| `ChoiceOffer` | Offers |
| --- | --- |
| `Augments` | augments the player does not have yet |
| `AugmentsOfTier(t)` | augments of one [tier](augments.md#augmentdef-calls) |
| `AugmentTiers(vec![(tier, weight), ...])` | augments of one tier, rolled by weight. In a choice round everyone gets the same tier |
| `Items` | any item |
| `ItemsTagged(tag)` | items with a [tag](items.md#itemdef-calls) |
| `Components` | component items |
| `Units { cost }` | units of one cost that the pool still has. The pick comes out of the pool |

The same offers work in choice rounds: `RoundKindDef::choice(3,
ChoiceOffer::AugmentTiers(vec![(1, 2), (2, 1)]))`.

```rust
use autoengine::content::{AugmentDef, ChoiceOffer, ContentSet, ItemDef};
use autoengine::game::Grant;
use autoengine::perk::{Perk, When};

let content = ContentSet::builder()
    .item(ItemDef::new("blade_emblem", "Blade Emblem").tag("emblem"))
    .item(ItemDef::new("shield_emblem", "Shield Emblem").tag("emblem"))
    // Picking the augment opens a pick of one of two emblems.
    .augment(
        AugmentDef::new("tome", "Tome of Kinship")
            .perk(Perk::on(When::Gained).gives(Grant::Choice { options: 2, offer: ChoiceOffer::ItemsTagged("emblem".into()) })),
    )
    .build()
    .unwrap();
assert!(content.augment("tome").is_some());
```

## What units earn in fights

Two effects reach past the end of a fight:

- `Effect::reward(grant)` hands a grant to the player who owns the unit it
  is aimed at, once the fight is over. Aim it with `Selector::Source` in a
  passive of the player's own unit (gold for each kill), or
  `Selector::EventOther` in a monster's `OnDeath` passive (the killer's
  player gets the drop).
- `Effect::grow(mods)` changes the unit's stats at once and for good: the
  player's unit keeps the change in every later fight. Merged copies keep
  what they earned. `game.grow_unit(player, uid, mods)` does the same from
  game code.

```rust
use autoengine::content::UnitDef;
use autoengine::effect::{Effect, Passive, Selector, Trigger};
use autoengine::game::Grant;
use autoengine::stats::Stat;

// A monster that drops a roll on the "orb" table for whoever kills it.
let golem = UnitDef::new("golem", "Golem")
    .summon_only()
    .passive(Passive::new("Hoard", Trigger::OnDeath).effect(Effect::reward(Grant::Loot("orb".into())).to(Selector::EventOther)));
// A unit that earns a gold piece and a little attack damage, for good, per kill.
let reaper = UnitDef::new("reaper", "Reaper").cost(2).passive(
    Passive::new("Harvest", Trigger::OnKill)
        .effect(Effect::reward(Grant::Gold(1)).to(Selector::Source))
        .effect(Effect::grow([Stat::AttackDamage.flat(2)]).to(Selector::Source)),
);
assert_eq!((golem.passives.len(), reaper.passives[0].effects.len()), (1, 2));
```

Nothing is handed out or kept for a side no player owns: a neutral board,
or the copy of a board a player fights when the number of players is odd.
The fight's events say what was earned (`Rewarded`, `Grew`), and a
`BoardUnit` carries the growth (`growth`) and any traits given to that one
unit (`traits`, with `game.add_unit_trait`), so a fight replays the same.

## Teams

`GameRules::team_size` (or `.team_size(n)` on the builder) splits players
into teams in seat order: with 2, seats 0 and 1 are a team, then 2 and 3,
and so on. Teammates:

- share one health total: damage and healing to one apply to all of them;
- are never paired against each other while anyone else is left, and never
  fight a copy of each other's board;
- can send each other units with `Command::Gift { uid, to }`;
- go out together, with the same placement, and win together.

`game.team_of(player)`, `game.teammates(a, b)`, `game.team_members(player)`
and `game.alive_teams()` answer questions about teams.

## Special shop offers

A `ShopSpecial` gives shop offers a chance to come with something extra: a
higher star level, or a trait the unit does not have. The offer costs what
the copies it stands for cost and holds them out of the pool.
`Player::shop_specials` says which offers are special, `game.shop_price`
what one costs, and the client marks them.

```rust
use autoengine::content::{ContentSet, ShopSpecial};

let content = ContentSet::builder()
    // From level 7, one offer in fifty is a two-star unit.
    .shop_special(ShopSpecial::new(20).from_level(7).star(2))
    .build()
    .unwrap();
assert_eq!(content.rules().shop_specials.len(), 1);
```

`ShopSpecial::new(n).with_trait_from(["id", ...])` gives the offer one of
the listed traits that the unit does not have.
