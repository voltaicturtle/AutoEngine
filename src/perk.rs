//! Perks, rule changes and loot: what augments, traits and items do to the
//! game around the fights.
//!
//! A [`Bonus`](crate::content::Bonus) changes how a player's units fight.
//! The three things here change everything else:
//!
//! - a [`Perk`] hands the player something when an occasion comes up: gold
//!   after a lost fight, an item on reaching a level, a roll on a loot
//!   table every round;
//! - a [`RuleMod`] changes one of the rules for that player: one more unit
//!   on the board, cheaper rerolls, more interest;
//! - a [`LootTable`] is a weighted list of things to hand out, rolled with
//!   [`Grant::Loot`].
//!
//! Augments have them for as long as the player holds the augment, a trait
//! tier while it is active on the board, and an item while a unit on the
//! board holds it.
//!
//! ```
//! use autoengine::content::{AugmentDef, Bonus, ItemDef, TraitDef};
//! use autoengine::game::Grant;
//! use autoengine::perk::{LootTable, Perk, Rule, When};
//!
//! // A trait that pays out when its owner loses, and more at a higher tier.
//! let gamblers = TraitDef::new("gamblers", "Gamblers")
//!     .breakpoint(2, [Bonus::members()])
//!     .perk(Perk::on(When::Lost).gives(Grant::Gold(1)))
//!     .breakpoint(4, [Bonus::members()])
//!     .perk(Perk::on(When::Lost).gives(Grant::Loot("spoils".into())));
//! // What a roll on the table can give.
//! let spoils = LootTable::new("spoils").entry(3, [Grant::Gold(3)]).entry(1, [Grant::Component]);
//! // An augment that makes rerolls cheaper and the team bigger.
//! let plan = AugmentDef::new("big_plans", "Big Plans").rule(Rule::RerollCost.add(-1)).rule(Rule::TeamSize.add(1));
//! // An item whose holder lets the team field one more unit.
//! let banner = ItemDef::new("banner", "War Banner").rule(Rule::TeamSize.add(1));
//! assert_eq!((gamblers.breakpoints[1].perks.len(), spoils.entries.len(), plan.rules.len(), banner.rules.len()), (1, 2, 2, 1));
//! ```

use serde::{Deserialize, Serialize};

use crate::game::Grant;

/// An occasion a [`Perk`] waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum When {
    /// The player picks up the augment the perk belongs to. Never happens
    /// for a perk on a trait or an item.
    Gained,
    /// A round starts, from the second round on.
    RoundStart,
    /// The player won a fight, against a player or a neutral board.
    Won,
    /// The player lost or drew a fight.
    Lost,
    /// The player's fight ended, however it went.
    Fought,
    /// The player reached a new level.
    LevelUp,
    /// The player rerolled the shop.
    Reroll,
    /// The player bought experience.
    BuyXp,
    /// The player sold a unit.
    UnitSold,
    /// One of the player's units reached a higher star level.
    StarUp,
}

/// Something handed to a player when an occasion comes up; see the
/// [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Perk {
    pub when: When,
    pub gives: Vec<Grant>,
    /// How often it happens when the occasion comes up, out of 1000.
    #[serde(default = "always")]
    pub chance_permille: u32,
}

fn always() -> u32 {
    1000
}

impl Perk {
    /// A perk for this occasion that gives nothing yet.
    pub fn on(when: When) -> Self {
        Perk { when, gives: Vec::new(), chance_permille: 1000 }
    }

    /// Adds something it hands out. Can be repeated.
    pub fn gives(mut self, grant: Grant) -> Self {
        self.gives.push(grant);
        self
    }

    /// Makes it happen only some of the time: `chance(250)` is one time in
    /// four.
    pub fn chance(mut self, permille: u32) -> Self {
        self.chance_permille = permille;
        self
    }
}

/// A rule of the game that can differ from player to player.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Rule {
    /// How many team slots the board has (`LevelRule::board_size`).
    TeamSize,
    /// The price of a reroll (`EconomyRules::reroll_cost`). Never below 0.
    RerollCost,
    /// The price of buying experience (`EconomyRules::xp_cost`). Never
    /// below 0.
    XpCost,
    /// Experience per purchase (`EconomyRules::xp_per_purchase`).
    XpPerPurchase,
    /// Experience at the start of each round (`EconomyRules::xp_per_round`).
    XpPerRound,
    /// Gold at the start of each round (`EconomyRules::round_income`).
    RoundIncome,
    /// The most interest a round pays (`EconomyRules::interest_cap`).
    InterestCap,
    /// Gold for winning a fight against a player (`EconomyRules::win_bonus`).
    WinBonus,
    /// Added to every bit of damage the player takes; negative shields
    /// them. Damage never goes below 0.
    DamageTaken,
    /// The shop rolls as if the player were this many levels higher (or
    /// lower), within the levels the rules have.
    ShopLevel,
}

impl Rule {
    /// A change to this rule: `Rule::TeamSize.add(1)`.
    pub fn add(self, amount: i64) -> RuleMod {
        RuleMod { rule: self, add: amount }
    }
}

/// A change to one [`Rule`] for one player; see the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleMod {
    pub rule: Rule,
    pub add: i64,
}

/// One thing a [`LootTable`] can come up with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LootEntry {
    /// How likely it is, against the other entries' weights.
    pub weight: u32,
    /// Everything the player gets when it comes up.
    pub gives: Vec<Grant>,
}

/// A weighted list of things to hand out. [`Grant::Loot`] rolls on it once
/// and gives the player everything in the entry that comes up.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LootTable {
    /// What content calls it: `"spoils"`.
    pub id: String,
    pub entries: Vec<LootEntry>,
}

impl LootTable {
    pub fn new(id: impl Into<String>) -> Self {
        LootTable { id: id.into(), entries: Vec::new() }
    }

    /// Adds an outcome with a weight. An entry may hold another
    /// `Grant::Loot`, to roll again on this or another table.
    pub fn entry(mut self, weight: u32, gives: impl IntoIterator<Item = Grant>) -> Self {
        self.entries.push(LootEntry { weight, gives: gives.into_iter().collect() });
        self
    }

    /// Adds an outcome that gives nothing.
    pub fn nothing(self, weight: u32) -> Self {
        self.entry(weight, [])
    }
}
