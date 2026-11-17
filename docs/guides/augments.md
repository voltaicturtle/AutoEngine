# Augments

An augment is something a player picks up that lasts for the rest of the
game: team-wide bonuses, gold up front, or rewards every round. Players get
them from choice rounds, and scripts and Rust code can hand them out.

```rust
use autoengine::content::{AugmentDef, Bonus, RoundGrant};
use autoengine::effect::{Amount, Effect, Passive, ScaleBy, Selector, Trigger, Who};
use autoengine::stats::Stat::*;

let augments = [
    AugmentDef::new("war_chest", "War Chest").description("Gain 12 gold now.").gold(12),
    AugmentDef::new("iron_discipline", "Iron Discipline")
        .description("Your team gains 15 armor and magic resist.")
        .bonus(Bonus::team().mods([Armor.flat(15), MagicResist.flat(15)])),
    AugmentDef::new("tidal_surge", "Tidal Surge")
        .description("Tide units gain 40 ability power.")
        .bonus(Bonus::with_trait("tide").mods([AbilityPower.flat(40)])),
    AugmentDef::new("second_breath", "Second Breath")
        .description("The first time each unit drops below 30% health, it heals 20% of its maximum.")
        .bonus(Bonus::team().passive(
            Passive::new("Second Breath", Trigger::HealthBelow { permille: 300 })
                .effect(Effect::heal(Amount::flat(0).plus(Who::Source, ScaleBy::Stat(MaxHealth), 200)).to(Selector::Source)),
        )),
    AugmentDef::new("trade_route", "Trade Route")
        .description("Gain 2 gold and a free reroll every round.")
        .per_round(RoundGrant { gold: 2, free_rerolls: 1, ..Default::default() }),
];
assert_eq!(augments.len(), 5);
```

## `AugmentDef` calls

| Call | What it does |
| --- | --- |
| `AugmentDef::new(id, name)` | an augment that does nothing yet |
| `.description(text)` | the text shown when it is offered |
| `.bonus(Bonus)` | team-wide stats or passives for every fight from now on. Can be called more than once |
| `.gold(n)` | gold given once, when it is picked |
| `.per_round(RoundGrant)` | something given at the start of every round after it is picked |
| `.tier(n)` | its tier, such as 1, 2 and 3 for weaker and stronger augments. Choice rounds can offer one tier (`ChoiceOffer::AugmentsOfTier`) or roll one (`ChoiceOffer::AugmentTiers`). The default is 1 |
| `.perk(Perk)` | grants handed out when something happens: a level up, a reroll, a lost fight. See [Perks](perks.md#perks) |
| `.rule(RuleMod)` | changes a rule for its holder, such as `Rule::TeamSize.add(1)`. See [Rules a player can bend](perks.md#rules-a-player-can-bend) |

Bonuses work as they do for traits (see [Traits](traits.md#bonuses)), with
one difference: `Bonus::members()` in an augment means every unit, the same
as `Bonus::team()`.

## `RoundGrant`

| Field | Given every round |
| --- | --- |
| `gold` | gold |
| `currencies` | other currencies, by id. Add one with `.currency("souls", 3)`; see [Currencies](rules.md#currencies) |
| `xp` | experience |
| `free_rerolls` | rerolls that cost no gold. They are used before gold, and unused ones carry over |
| `components` | random component items |
| `health` | health back, up to what players start with |

## How players get augments

- **Choice rounds:** `RoundKindDef::choice(3, ChoiceOffer::Augments)` offers
  each player three augments they don't have yet. Players may reroll the
  offer `GameRules::choice_rerolls` times (once by default). Anyone who
  doesn't pick gets one picked for them when the round ends.
- **Lobby modifiers:** `GameRules::lobby_modifiers` lists augments, one of
  which, picked at random, every player gets when the game starts. This is a
  twist that makes each game different. `game.lobby_modifier()` says which
  one, and clients show it.
- **Scripts:** `game.give_augment(p, "id")`.
- **Rust:** `game.give_augment(player, "id")`, or
  `game.grant(player, Grant::Augment("id".into()))` from outside the rules,
  so the game can still be replayed.

```rust
use autoengine::content::{AugmentDef, Bonus, ContentSet, GameRules, UnitDef};
use autoengine::stats::Stat::*;

let rules = GameRules { lobby_modifiers: vec!["haste".into(), "bulwark".into()], ..GameRules::default() };
let content = ContentSet::builder()
    .rules(rules)
    .unit(UnitDef::new("grunt", "Grunt").cost(1))
    .augment(AugmentDef::new("haste", "Haste").description("Everyone attacks 15% faster.").bonus(Bonus::team().mods([AttackSpeed.percent(150)])))
    .augment(AugmentDef::new("bulwark", "Bulwark").description("Everyone has 20 more armor.").bonus(Bonus::team().mods([Armor.flat(20)])))
    .build()
    .unwrap();
assert_eq!(content.rules().lobby_modifiers.len(), 2);
```
