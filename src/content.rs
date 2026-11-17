//! Game content: the units, traits, items, augments, rounds and rules that
//! make up a game.
//!
//! A [`ContentSet`] holds everything a game built on the engine defines, and
//! a game defines it in Rust with [`ContentSet::builder`]. Every definition
//! type has a constructor and chainable setters, and the builder can also
//! register Rust hooks for effects and conditions the built-in blocks cannot
//! express. The set is validated when it is built, so mistakes such as a
//! unit naming a trait that does not exist are reported up front, not in the
//! middle of a match. [`crate::sample`] is a complete example.
//!
//! ```
//! use autoengine::content::{Bonus, ContentSet, ItemDef, TraitDef, UnitDef};
//! use autoengine::effect::{Ability, Effect, Selector};
//! use autoengine::stats::Stat;
//!
//! let game = ContentSet::builder()
//!     .board(7, 8)
//!     .trait_def(TraitDef::new("grove", "Grove").breakpoint(2, [Bonus::members().mods([Stat::Armor.flat(15)])]))
//!     .unit(
//!         UnitDef::new("warden", "Warden")
//!             .cost(2)
//!             .traits(["grove"])
//!             .stat(Stat::MaxHealth, 900)
//!             .ability(Ability::new("Rootwall").effect(Effect::shield_for(250, 4000).to(Selector::Source))),
//!     )
//!     .item(ItemDef::new("whetstone", "Whetstone").mods([Stat::AttackDamage.flat(15)]))
//!     .build()
//!     .unwrap();
//! assert_eq!(game.unit("warden").unwrap().cost, 2);
//! ```
//!
//! The content layer sits on top of combat without combat knowing about it:
//! [`ContentSet::build_team`] turns "these units, at these star levels, with
//! these items, on these hexes" into plain [`UnitTemplate`]s with every trait
//! and item bonus already folded in.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::combat::{
    Combat, ConditionContext, CustomConditionFn, CustomEffectFn, CustomSelectorFn, EffectContext, SelectContext, Team, UnitId,
};
use crate::damage::DamageType;
use crate::game::{CommandError, Game, Grant, PlayerId};
use crate::effect::{
    Ability, Amount, Condition, Cost, DamageKind, Effect, EffectSpec, Passive, ScaleBy, Selector, StatusKind, UnitTemplate,
};
use crate::resource::ResourceDef;
use crate::hex::{Hex, HexBoard};
use crate::stats::{Stat, StatBlock, StatMod};
use crate::perk::{LootTable, Perk, RuleMod};
use crate::tile::{TileDef, TilePlacement};

fn default_true() -> bool {
    true
}

/// A unit definition. Build one with [`UnitDef::new`] and its setters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitDef {
    pub id: String,
    pub name: String,
    /// Shop cost, which also serves as the unit's tier.
    #[serde(default)]
    pub cost: u32,
    /// Ids of the traits this unit belongs to.
    #[serde(default)]
    pub traits: Vec<String>,
    /// Stats that differ from [`StatBlock::base`].
    #[serde(default)]
    pub stats: BTreeMap<Stat, i64>,
    /// The abilities it casts. Each tick it casts the first one it can
    /// pay for, so the ones that should win go first.
    #[serde(default)]
    pub abilities: Vec<Ability>,
    #[serde(default)]
    pub passives: Vec<Passive>,
    /// Whether the unit can appear in shops. Summoned-only units set this
    /// to `false`.
    #[serde(default = "default_true")]
    pub in_shop: bool,
    /// How graphical clients should draw the unit. The engine itself never
    /// looks at this.
    #[serde(default)]
    pub model: Option<ModelRef>,
    /// Which enemy the unit goes after.
    #[serde(default)]
    pub targeting: crate::effect::Targeting,
    /// The type of damage its basic attacks deal.
    #[serde(default = "physical")]
    pub attack_kind: DamageKind,
    /// How many copies the shared pool holds, instead of the number
    /// `GameRules::pool_sizes` gives units of its cost.
    #[serde(default)]
    pub pool_size: Option<u32>,
    /// How many of the board's team slots it takes. Most units take 1.
    #[serde(default = "one")]
    pub team_slots: u32,
}

fn one() -> u32 {
    1
}

fn physical() -> DamageKind {
    DamageKind::Physical
}

/// A 3D model for a unit: display-only data that travels with the content
/// so every client draws the same thing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    /// Model file, relative to the client's assets directory. Any format
    /// the client's importer understands (OBJ, glTF, FBX, PLY, ...).
    pub file: String,
    /// Height on the board as a percentage of the standard unit height.
    #[serde(default = "default_hundred")]
    pub scale_percent: u32,
    /// Turn applied to the model so that it faces forward, for files
    /// authored facing another way.
    #[serde(default)]
    pub yaw_degrees: i32,
}

fn default_hundred() -> u32 {
    100
}

impl UnitDef {
    /// A unit with base stats, no traits, no ability, that appears in shops.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        UnitDef {
            id: id.into(),
            name: name.into(),
            cost: 0,
            traits: Vec::new(),
            stats: BTreeMap::new(),
            abilities: Vec::new(),
            passives: Vec::new(),
            in_shop: true,
            model: None,
            targeting: crate::effect::Targeting::Nearest,
            attack_kind: DamageKind::Physical,
            pool_size: None,
            team_slots: 1,
        }
    }

    /// The type of damage its basic attacks deal, instead of physical.
    pub fn attack_kind(mut self, kind: DamageKind) -> Self {
        self.attack_kind = kind;
        self
    }

    /// Which enemy the unit goes after, such as
    /// `Targeting::Farthest` for an assassin.
    pub fn targeting(mut self, targeting: crate::effect::Targeting) -> Self {
        self.targeting = targeting;
        self
    }

    pub fn cost(mut self, cost: u32) -> Self {
        self.cost = cost;
        self
    }

    /// Adds traits by id.
    pub fn traits<S: Into<String>>(mut self, ids: impl IntoIterator<Item = S>) -> Self {
        self.traits.extend(ids.into_iter().map(Into::into));
        self
    }

    /// Overrides one base stat.
    pub fn stat(mut self, stat: Stat, value: i64) -> Self {
        self.stats.insert(stat, value);
        self
    }

    /// Overrides several base stats.
    pub fn stats(mut self, stats: impl IntoIterator<Item = (Stat, i64)>) -> Self {
        self.stats.extend(stats);
        self
    }

    /// Adds an ability. Most units have one; call this again to give a
    /// unit several, which works best when they have different
    /// [costs](crate::effect::Cost) or cooldowns. Each tick the unit casts
    /// the first one it can pay for, in the order they were added.
    pub fn ability(mut self, ability: Ability) -> Self {
        self.abilities.push(ability);
        self
    }

    pub fn passive(mut self, passive: Passive) -> Self {
        self.passives.push(passive);
        self
    }

    /// Sets how many copies of the unit the shared pool holds, whatever
    /// its cost: `.pool_size(1)` for a unit only one player can own.
    pub fn pool_size(mut self, copies: u32) -> Self {
        self.pool_size = Some(copies);
        self
    }

    /// Keeps the unit out of shops, e.g. for units that are only summoned.
    pub fn summon_only(mut self) -> Self {
        self.in_shop = false;
        self
    }

    /// Makes the unit take this many of the board's team slots: a huge
    /// unit that counts as two.
    pub fn team_slots(mut self, slots: u32) -> Self {
        self.team_slots = slots;
        self
    }

    /// The model graphical clients draw for this unit.
    pub fn model(mut self, file: impl Into<String>, scale_percent: u32) -> Self {
        self.model = Some(ModelRef { file: file.into(), scale_percent, yaw_degrees: 0 });
        self
    }
}

/// Who receives a bonus.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Scope {
    /// Units that have the trait granting the bonus. For team-wide bonuses
    /// that do not come from a trait, this means every unit.
    #[default]
    Members,
    /// Every unit on the team.
    Team,
    /// Units that have the named trait.
    WithTrait(String),
}

/// Stat changes and passives granted to a set of units.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Bonus {
    pub scope: Scope,
    pub mods: Vec<StatMod>,
    /// Modifiers multiplied by a member count: for a trait bonus, the number
    /// of distinct units with that trait; for a team bonus scoped
    /// [`Scope::WithTrait`], the units with the named trait; for any other
    /// team bonus, the distinct units on the board.
    pub mods_per_member: Vec<StatMod>,
    pub passives: Vec<Passive>,
}

impl Bonus {
    /// A bonus for the members of the trait granting it (or every unit, for
    /// a team bonus that does not come from a trait).
    pub fn members() -> Self {
        Bonus::default()
    }

    /// A bonus for every unit on the team.
    pub fn team() -> Self {
        Bonus { scope: Scope::Team, ..Bonus::default() }
    }

    /// A bonus for units that have the given trait.
    pub fn with_trait(id: impl Into<String>) -> Self {
        Bonus { scope: Scope::WithTrait(id.into()), ..Bonus::default() }
    }

    pub fn mods(mut self, mods: impl IntoIterator<Item = StatMod>) -> Self {
        self.mods.extend(mods);
        self
    }

    /// Modifiers that scale with the member count; see
    /// [`Bonus::mods_per_member`](field@Bonus::mods_per_member).
    pub fn mods_per_member(mut self, mods: impl IntoIterator<Item = StatMod>) -> Self {
        self.mods_per_member.extend(mods);
        self
    }

    pub fn passive(mut self, passive: Passive) -> Self {
        self.passives.push(passive);
        self
    }
}

/// One tier of a trait: active when at least `count` distinct members are on
/// the board. Only the highest tier reached applies.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Breakpoint {
    pub count: u32,
    pub bonuses: Vec<Bonus>,
    /// Active only with exactly `count` members, not with more: a trait
    /// that rewards fielding a single member.
    #[serde(default)]
    pub exactly: bool,
    /// Things the player gets on occasions while the tier is active; see
    /// [`crate::perk`].
    #[serde(default)]
    pub perks: Vec<Perk>,
    /// Rules that change for the player while the tier is active.
    #[serde(default)]
    pub rules: Vec<RuleMod>,
}

impl Breakpoint {
    /// Whether this tier is the one a board with `count` members has, given
    /// the trait's tiers in order.
    fn reached(&self, count: u32) -> bool {
        if self.exactly { self.count == count } else { self.count <= count }
    }
}

/// A trait (synergy) definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub breakpoints: Vec<Breakpoint>,
}

impl TraitDef {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        TraitDef { id: id.into(), name: name.into(), description: String::new(), breakpoints: Vec::new() }
    }

    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = text.into();
        self
    }

    /// Adds a tier that is active from `count` distinct members. Tiers must
    /// be added in increasing order.
    pub fn breakpoint(mut self, count: u32, bonuses: impl IntoIterator<Item = Bonus>) -> Self {
        self.breakpoints.push(Breakpoint { count, bonuses: bonuses.into_iter().collect(), ..Breakpoint::default() });
        self
    }

    /// Adds a tier that is active with exactly `count` members and no
    /// more.
    pub fn breakpoint_exactly(mut self, count: u32, bonuses: impl IntoIterator<Item = Bonus>) -> Self {
        self.breakpoints.push(Breakpoint { count, bonuses: bonuses.into_iter().collect(), exactly: true, ..Breakpoint::default() });
        self
    }

    /// Adds a [perk](crate::perk) to the tier added last: something the
    /// player gets on an occasion while that tier is active.
    pub fn perk(mut self, perk: Perk) -> Self {
        self.breakpoints.last_mut().expect("add a breakpoint before its perks").perks.push(perk);
        self
    }

    /// Changes a rule for the player while the tier added last is active.
    pub fn rule(mut self, rule: RuleMod) -> Self {
        self.breakpoints.last_mut().expect("add a breakpoint before its rules").rules.push(rule);
        self
    }

    /// The tier a board with `count` members has, if any: the last one
    /// reached.
    pub fn tier(&self, count: u32) -> Option<usize> {
        self.breakpoints.iter().rposition(|b| b.reached(count))
    }
}

/// An item definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDef {
    pub id: String,
    pub name: String,
    /// What it does, for players to read.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub mods: Vec<StatMod>,
    #[serde(default)]
    pub passives: Vec<Passive>,
    /// Traits the holder counts as having while it carries this item.
    #[serde(default)]
    pub grants_traits: Vec<String>,
    /// The two component items that combine into this one, if any.
    #[serde(default)]
    pub recipe: Option<(String, String)>,
    /// Makes the item single-use: giving it to a unit does this instead of
    /// equipping it, and the item is gone.
    #[serde(default)]
    pub consumable: Option<Consumable>,
    /// A unit can hold only one of it.
    #[serde(default)]
    pub unique: bool,
    /// Labels a game sorts its items by: `"artifact"`, `"emblem"`. Choices
    /// and grants can ask for items with a tag.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Things the player gets on occasions while a unit on the board holds
    /// the item; see [`crate::perk`].
    #[serde(default)]
    pub perks: Vec<Perk>,
    /// Rules that change for the player while a unit on the board holds
    /// the item.
    #[serde(default)]
    pub rules: Vec<RuleMod>,
    /// The item a [`Consumable::Upgrade`] turns this one into.
    #[serde(default)]
    pub upgrades_to: Option<String>,
}

/// What a single-use item does when it is given to a unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Consumable {
    /// Takes every item off the unit, back to the player's items.
    RemoveItems,
    /// Gives the player another one-star copy of the unit, from the pool.
    Duplicate,
    /// Swaps each item on the unit for a random other one of the same kind:
    /// components for components, finished items for finished items.
    Reforge,
    /// Gold for the player; the unit does not matter.
    Gold(i64),
    /// Anything a [`Grant`] can hand the player: a currency, experience,
    /// health, an item, an augment. The unit does not matter.
    Give(Grant),
    /// Turns one item the unit holds into its better version
    /// ([`ItemDef::upgrades_to`]): the first one that has one.
    Upgrade,
}

impl ItemDef {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        ItemDef {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            mods: Vec::new(),
            passives: Vec::new(),
            grants_traits: Vec::new(),
            recipe: None,
            consumable: None,
            unique: false,
            tags: Vec::new(),
            perks: Vec::new(),
            rules: Vec::new(),
            upgrades_to: None,
        }
    }

    /// What it does, for players to read.
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = text.into();
        self
    }

    /// Lets a unit hold only one of this item.
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Adds a label choices and grants can ask for.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Adds a [perk](crate::perk) the player has while a unit on the board
    /// holds the item.
    pub fn perk(mut self, perk: Perk) -> Self {
        self.perks.push(perk);
        self
    }

    /// Changes a rule for the player while a unit on the board holds the
    /// item.
    pub fn rule(mut self, rule: RuleMod) -> Self {
        self.rules.push(rule);
        self
    }

    /// Names the better version a [`Consumable::Upgrade`] turns this item
    /// into.
    pub fn upgrades_to(mut self, id: impl Into<String>) -> Self {
        self.upgrades_to = Some(id.into());
        self
    }

    /// Makes the item single-use (see [`Consumable`]).
    pub fn consumable(mut self, consumable: Consumable) -> Self {
        self.consumable = Some(consumable);
        self
    }

    pub fn mods(mut self, mods: impl IntoIterator<Item = StatMod>) -> Self {
        self.mods.extend(mods);
        self
    }

    pub fn passive(mut self, passive: Passive) -> Self {
        self.passives.push(passive);
        self
    }

    /// The holder counts as having this trait.
    pub fn grants_trait(mut self, id: impl Into<String>) -> Self {
        self.grants_traits.push(id.into());
        self
    }

    /// The two component items that combine into this one.
    pub fn recipe(mut self, a: impl Into<String>, b: impl Into<String>) -> Self {
        self.recipe = Some((a.into(), b.into()));
        self
    }
}

/// Gold and experience rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EconomyRules {
    pub starting_gold: i64,
    /// Gold every player receives at the start of each round after the first.
    pub round_income: i64,
    /// One extra gold per this much gold held...
    pub interest_per: i64,
    /// ...up to this much.
    pub interest_cap: i64,
    /// Gold for winning a fight against another player.
    pub win_bonus: i64,
    /// `(streak length, gold)` pairs; the longest streak reached applies to
    /// both win and loss streaks.
    pub streak_bonuses: Vec<(u32, i64)>,
    pub reroll_cost: i64,
    pub xp_cost: i64,
    pub xp_per_purchase: u32,
    /// Experience every player gains each round.
    pub xp_per_round: u32,
}

impl Default for EconomyRules {
    fn default() -> Self {
        EconomyRules {
            starting_gold: 4,
            round_income: 5,
            interest_per: 10,
            interest_cap: 5,
            win_bonus: 1,
            streak_bonuses: vec![(2, 1), (4, 2), (6, 3)],
            reroll_cost: 2,
            xp_cost: 4,
            xp_per_purchase: 4,
            xp_per_round: 2,
        }
    }
}

/// What something costs: an amount of gold, or of a currency the game
/// defines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Price {
    /// `"gold"` or a currency's id.
    pub currency: String,
    pub amount: i64,
}

impl Price {
    pub fn gold(amount: i64) -> Self {
        Price { currency: "gold".into(), amount }
    }

    pub fn of(currency: impl Into<String>, amount: i64) -> Self {
        Price { currency: currency.into(), amount }
    }
}

/// What a player has to point an action at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionTarget {
    /// Nothing: the action is a plain button.
    #[default]
    None,
    /// One of the player's own units. The command's `arg` is its `uid`, and
    /// the engine refuses a unit the player does not own.
    Unit,
}

/// Something a player can do during planning that the game itself defines:
/// trade souls for experience, pay for a blessing, sacrifice a unit.
/// Clients show actions as buttons and send
/// [`Command::Action`](crate::game::Command::Action).
///
/// The engine checks the price and the limit, and takes the price once the
/// action goes through. What the action does is any mix of:
///
/// - [`gives`](Self::gives): things handed over, with no code at all;
/// - a Rust handler added with
///   [`GameBuilder::action_handler`], which can also refuse;
/// - hooks and scripts reacting to
///   [`GameEvent::ActionUsed`](crate::game::GameEvent::ActionUsed).
///
/// ```
/// use autoengine::content::{ActionDef, ContentSet, CurrencyDef};
/// use autoengine::game::Grant;
///
/// let game = ContentSet::builder()
///     .currency(CurrencyDef::new("souls", "Souls").per_round(1))
///     .action(ActionDef::new("study", "Study").description("Trade 2 souls for 6 experience.").costs("souls", 2).gives(Grant::Xp(6)).per_round(1))
///     .build()
///     .unwrap();
/// assert_eq!(game.action("study").unwrap().prices[0].amount, 2);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// What using it costs. Empty is free.
    #[serde(default)]
    pub prices: Vec<Price>,
    /// What the player gets each time.
    #[serde(default)]
    pub gives: Vec<Grant>,
    /// How many times a player may use it in one round. `None` is no limit.
    #[serde(default)]
    pub per_round: Option<u32>,
    #[serde(default)]
    pub target: ActionTarget,
}

impl ActionDef {
    /// A free action that does nothing yet.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        ActionDef {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            prices: Vec::new(),
            gives: Vec::new(),
            per_round: None,
            target: ActionTarget::None,
        }
    }

    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = text.into();
        self
    }

    /// Adds to the price: `.costs("gold", 2)` or `.costs("souls", 1)`.
    pub fn costs(mut self, currency: impl Into<String>, amount: i64) -> Self {
        self.prices.push(Price::of(currency, amount));
        self
    }

    /// Adds something the player gets each time they use the action.
    pub fn gives(mut self, grant: Grant) -> Self {
        self.gives.push(grant);
        self
    }

    /// Limits how often a player may use it in one round.
    pub fn per_round(mut self, times: u32) -> Self {
        self.per_round = Some(times);
        self
    }

    /// Makes the action need one of the player's own units.
    pub fn on_unit(mut self) -> Self {
        self.target = ActionTarget::Unit;
        self
    }
}

/// A Rust handler for an [`ActionDef`]: gets the game, the player and the
/// command's `arg`. Returning an error refuses the action, and nothing is
/// charged; a handler must not change the game before it refuses.
pub type ActionFn = dyn Fn(&mut Game, PlayerId, i64) -> Result<(), CommandError> + Send + Sync;

/// A currency players hold besides gold: tokens, souls, favor. The engine
/// keeps the balance, pays it out and shows it; what it buys is up to the
/// game's round types, scripts and hooks (see
/// [`Game::spend_currency`](crate::game::Game::spend_currency)).
///
/// ```
/// use autoengine::content::{ContentSet, CurrencyDef};
///
/// let game = ContentSet::builder().currency(CurrencyDef::new("souls", "Souls").starting(1).per_round(2)).build().unwrap();
/// assert_eq!(game.currency("souls").unwrap().per_round, 2);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrencyDef {
    /// What content calls it: `"souls"`.
    pub id: String,
    /// What players call it.
    pub name: String,
    /// How much every player starts the game with.
    #[serde(default)]
    pub starting: i64,
    /// How much every player receives at the start of each round after the
    /// first.
    #[serde(default)]
    pub per_round: i64,
    /// The color clients show it in, as red, green and blue.
    #[serde(default)]
    pub color: Option<[u8; 3]>,
}

impl CurrencyDef {
    /// A currency nobody is given yet.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        CurrencyDef { id: id.into(), name: name.into(), starting: 0, per_round: 0, color: None }
    }

    pub fn starting(mut self, amount: i64) -> Self {
        self.starting = amount;
        self
    }

    pub fn per_round(mut self, amount: i64) -> Self {
        self.per_round = amount;
        self
    }

    pub fn color(mut self, rgb: [u8; 3]) -> Self {
        self.color = Some(rgb);
        self
    }
}

/// What one player level means.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelRule {
    /// Experience needed to reach the next level. Ignored on the last level.
    pub xp_to_next: u32,
    /// How many units the player may field.
    pub board_size: usize,
    /// Relative chance of each unit cost appearing in the shop, starting at
    /// cost 1.
    pub shop_odds: Vec<u32>,
}

/// Global rules. Every field has a default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameRules {
    pub board_cols: u32,
    pub board_rows: u32,
    pub max_items_per_unit: usize,
    /// Stat modifiers for each star level; the first entry is one star.
    pub star_levels: Vec<Vec<StatMod>>,
    /// Copies of a unit at the same star level that merge into the next.
    pub copies_to_upgrade: u32,
    pub shop_size: usize,
    pub bench_size: usize,
    pub starting_health: i64,
    pub economy: EconomyRules,
    /// One entry per player level, starting at level 1.
    pub levels: Vec<LevelRule>,
    /// Copies of each unit in the shared pool, by unit cost.
    pub pool_sizes: BTreeMap<u32, u32>,
    /// Pool size for costs not listed in `pool_sizes`.
    pub default_pool_size: u32,
    /// Damage a player takes for losing a fight, by stage (starting at
    /// stage 1). Later stages reuse the last entry plus one per extra stage.
    pub base_damage_by_stage: Vec<i64>,
    /// Extra damage per enemy unit left standing.
    pub damage_per_survivor: i64,
    /// Player fights avoid pairing someone with anyone they fought in
    /// their last this-many player fights, when another pairing is possible.
    pub avoid_rematch_rounds: u32,
    /// Before the fights, empty places on a player's board (up to their
    /// level's limit) are filled from the bench, strongest units first, so
    /// forgetting to place a unit does not throw a round away.
    pub auto_fill_board: bool,
    /// How fights run: tick length, time limit, overtime and so on.
    pub combat: crate::combat::CombatConfig,
    /// Planning time for rounds of a kind (such as `"draft"`), in
    /// milliseconds, instead of the time the server is set to.
    pub planning_ms_by_kind: BTreeMap<String, u64>,
    /// Times each player may swap the options a choice round offers them
    /// for new ones, per round.
    pub choice_rerolls: u32,
    /// Augments one of which, picked at random, every player gets when the
    /// game starts: a twist that makes each game a little different. Empty
    /// for none.
    pub lobby_modifiers: Vec<String>,
    /// Currencies players hold besides gold.
    pub currencies: Vec<CurrencyDef>,
    /// The share of what a unit is worth that selling it gives back, in
    /// thousandths. 1000 is a full refund.
    pub sell_refund_permille: u32,
    /// Things players can do during planning besides the built-in commands.
    pub actions: Vec<ActionDef>,
    /// Weighted lists of things to hand out; see [`crate::perk::LootTable`].
    pub loot: Vec<LootTable>,
    /// Players per team. With more than 1, seats are split into teams in
    /// order (seats 0 and 1 together, then 2 and 3, and so on): teammates
    /// share their health, never fight each other, can send each other
    /// units with `Command::Gift`, and place together.
    pub team_size: u32,
    /// Shop offers that come with something extra; see [`ShopSpecial`].
    pub shop_specials: Vec<ShopSpecial>,
}

/// A chance for a shop offer to come with something extra: a higher star
/// level, or a trait the unit does not have. It costs what the copies it
/// stands for cost, and takes them out of the pool.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ShopSpecial {
    /// How often an offer becomes this special, out of 1000.
    pub chance_permille: u32,
    /// The player's lowest level for it to show up.
    pub min_level: u32,
    /// The star level of the offer. 0 and 1 leave it at 1 star.
    pub star: u8,
    /// Ids of traits one of which the offer gets, picked at random among
    /// those the unit does not already have. Empty for none.
    pub traits: Vec<String>,
}

impl ShopSpecial {
    /// A special that shows up `chance_permille` times out of 1000.
    pub fn new(chance_permille: u32) -> Self {
        ShopSpecial { chance_permille, ..ShopSpecial::default() }
    }

    /// Only from this player level on.
    pub fn from_level(mut self, level: u32) -> Self {
        self.min_level = level;
        self
    }

    /// Offers the unit at a higher star level.
    pub fn star(mut self, star: u8) -> Self {
        self.star = star;
        self
    }

    /// Gives the offer one of these traits.
    pub fn with_trait_from<S: Into<String>>(mut self, traits: impl IntoIterator<Item = S>) -> Self {
        self.traits.extend(traits.into_iter().map(Into::into));
        self
    }
}

impl Default for GameRules {
    fn default() -> Self {
        let level = |xp_to_next, board_size, shop_odds: [u32; 5]| LevelRule {
            xp_to_next,
            board_size,
            shop_odds: shop_odds.to_vec(),
        };
        GameRules {
            board_cols: 7,
            board_rows: 8,
            max_items_per_unit: 3,
            star_levels: vec![
                vec![],
                vec![
                    StatMod::percent(Stat::MaxHealth, 800),
                    StatMod::percent(Stat::AttackDamage, 500),
                    StatMod::percent(Stat::AbilityPower, 500),
                ],
                vec![
                    StatMod::percent(Stat::MaxHealth, 2240),
                    StatMod::percent(Stat::AttackDamage, 1250),
                    StatMod::percent(Stat::AbilityPower, 1250),
                ],
            ],
            copies_to_upgrade: 3,
            shop_size: 5,
            bench_size: 9,
            starting_health: 100,
            economy: EconomyRules::default(),
            levels: vec![
                level(2, 1, [100, 0, 0, 0, 0]),
                level(2, 2, [100, 0, 0, 0, 0]),
                level(6, 3, [75, 25, 0, 0, 0]),
                level(10, 4, [55, 30, 15, 0, 0]),
                level(20, 5, [45, 33, 20, 2, 0]),
                level(36, 6, [30, 40, 25, 5, 0]),
                level(56, 7, [19, 30, 35, 15, 1]),
                level(0, 8, [15, 20, 35, 25, 5]),
            ],
            pool_sizes: BTreeMap::from([(1, 29), (2, 22), (3, 18), (4, 12), (5, 10)]),
            default_pool_size: 10,
            base_damage_by_stage: vec![2, 3, 5, 8, 12, 17],
            damage_per_survivor: 1,
            avoid_rematch_rounds: 2,
            auto_fill_board: true,
            combat: crate::combat::CombatConfig::default(),
            planning_ms_by_kind: BTreeMap::new(),
            choice_rerolls: 1,
            lobby_modifiers: Vec::new(),
            currencies: Vec::new(),
            sell_refund_permille: 1000,
            actions: Vec::new(),
            loot: Vec::new(),
            team_size: 1,
            shop_specials: Vec::new(),
        }
    }
}

impl GameRules {
    pub fn board(mut self, cols: u32, rows: u32) -> Self {
        self.board_cols = cols;
        self.board_rows = rows;
        self
    }

    pub fn max_items_per_unit(mut self, n: usize) -> Self {
        self.max_items_per_unit = n;
        self
    }

    /// Stat modifiers for each star level, starting at one star.
    pub fn star_levels<L: IntoIterator<Item = StatMod>>(mut self, levels: impl IntoIterator<Item = L>) -> Self {
        self.star_levels = levels.into_iter().map(|l| l.into_iter().collect()).collect();
        self
    }
}

/// An augment-style modifier a player can pick up: team-wide bonuses for the
/// rest of the game, optionally with some gold up front.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AugmentDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub bonuses: Vec<Bonus>,
    #[serde(default)]
    pub gold: i64,
    /// Given to the holder at the start of every round after it is picked.
    #[serde(default)]
    pub per_round: RoundGrant,
    /// How strong it is, for choices that offer one tier at a time. 0 for
    /// games that do not sort their augments.
    #[serde(default)]
    pub tier: u32,
    /// Things the holder gets on occasions; see [`crate::perk`].
    #[serde(default)]
    pub perks: Vec<Perk>,
    /// Rules that change for the holder.
    #[serde(default)]
    pub rules: Vec<RuleMod>,
}

/// Things an augment hands out every round.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoundGrant {
    pub gold: i64,
    pub xp: u32,
    /// Rerolls that cost nothing, used before any gold is spent. Unused ones
    /// carry over.
    pub free_rerolls: u32,
    /// Random component items.
    pub components: u32,
    /// Other currencies, by id.
    pub currencies: BTreeMap<String, i64>,
    /// Health back, up to what players start with.
    pub health: i64,
}

impl RoundGrant {
    /// Adds some of a currency other than gold to the grant.
    pub fn currency(mut self, id: impl Into<String>, amount: i64) -> Self {
        self.currencies.insert(id.into(), amount);
        self
    }
}

/// What a choice round offers.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ChoiceOffer {
    /// Augments the player does not have yet, of any tier.
    #[default]
    Augments,
    /// Any item.
    Items,
    /// Augments of one [tier](AugmentDef::tier).
    AugmentsOfTier(u32),
    /// Augments of one tier, rolled once for everyone: `(tier, weight)`
    /// pairs. Every player of a round is offered the same tier.
    AugmentTiers(Vec<(u32, u32)>),
    /// Items with a [tag](ItemDef::tag).
    ItemsTagged(String),
    /// Component items: the ones not made from others and not used up.
    Components,
    /// Units of one cost that the shared pool still has. The pick comes
    /// out of the pool.
    Units { cost: u32 },
}

/// The kind of a round and its settings. The built-in kinds cover fights and
/// non-combat rounds; `Custom` names a round type registered by the game
/// built on the engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundKindDef {
    /// Players are paired off and fight each other.
    Pvp,
    /// Every player fights the same neutral board. Winning grants the rewards.
    Pve {
        enemies: Vec<BoardUnit>,
        #[serde(default)]
        gold: i64,
        /// Number of random component items.
        #[serde(default)]
        items: u32,
        /// Anything else winning gives: a currency, experience, health, a
        /// named item or augment.
        #[serde(default)]
        rewards: Vec<Grant>,
    },
    /// A shared draft: one pool of units is offered to everyone and players
    /// pick in turn, lowest health first.
    Draft {
        /// Offers beyond one per player.
        #[serde(default)]
        extra: u32,
        #[serde(default)]
        max_cost: Option<u32>,
    },
    /// Each player privately picks one of several options. No combat.
    Choice {
        options: u32,
        #[serde(default)]
        offer: ChoiceOffer,
    },
    /// A round type supplied by the game, looked up by name.
    Custom {
        kind: String,
        #[serde(default)]
        params: BTreeMap<String, i64>,
    },
}

fn default_stage() -> u32 {
    1
}

/// One entry of the round schedule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundDef {
    pub name: String,
    #[serde(default = "default_stage")]
    pub stage: u32,
    pub kind: RoundKindDef,
    /// [Tiles](crate::tile) every fight of the round starts with.
    #[serde(default)]
    pub tiles: Vec<TilePlacement>,
}

/// The order of rounds in a game. When the list
/// runs out it loops back to `repeat_from`, one stage higher each time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub rounds: Vec<RoundDef>,
    #[serde(default)]
    pub repeat_from: usize,
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule { rounds: vec![RoundDef { name: "Fight".into(), stage: 1, kind: RoundKindDef::Pvp, tiles: Vec::new() }], repeat_from: 0 }
    }
}

impl Schedule {
    /// The round at a position in the game, with its effective stage.
    pub fn round(&self, index: usize) -> (&RoundDef, u32) {
        if index < self.rounds.len() {
            let def = &self.rounds[index];
            return (def, def.stage);
        }
        let cycle = self.rounds.len() - self.repeat_from;
        let past = index - self.rounds.len();
        let def = &self.rounds[self.repeat_from + past % cycle];
        (def, def.stage + 1 + (past / cycle) as u32)
    }
}

fn default_star() -> u8 {
    1
}

impl AugmentDef {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        AugmentDef {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            bonuses: Vec::new(),
            gold: 0,
            per_round: RoundGrant::default(),
            tier: 0,
            perks: Vec::new(),
            rules: Vec::new(),
        }
    }

    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = text.into();
        self
    }

    pub fn bonus(mut self, bonus: Bonus) -> Self {
        self.bonuses.push(bonus);
        self
    }

    /// Gold given once, when the augment is picked.
    pub fn gold(mut self, gold: i64) -> Self {
        self.gold = gold;
        self
    }

    /// Something given every round from then on, such as
    /// `RoundGrant { gold: 2, ..Default::default() }`.
    pub fn per_round(mut self, grant: RoundGrant) -> Self {
        self.per_round = grant;
        self
    }

    /// Sorts it into a tier, for [`ChoiceOffer::AugmentsOfTier`].
    pub fn tier(mut self, tier: u32) -> Self {
        self.tier = tier;
        self
    }

    /// Adds a [perk](crate::perk): something the holder gets on an
    /// occasion. `Perk::on(When::Gained)` happens once, when it is picked.
    pub fn perk(mut self, perk: Perk) -> Self {
        self.perks.push(perk);
        self
    }

    /// Changes a rule for the holder: `Rule::TeamSize.add(1)`.
    pub fn rule(mut self, rule: RuleMod) -> Self {
        self.rules.push(rule);
        self
    }
}

impl RoundKindDef {
    pub fn pvp() -> Self {
        RoundKindDef::Pvp
    }

    /// Everyone fights this neutral board. Add rewards with
    /// [`gold`](Self::gold) and [`items`](Self::items).
    pub fn pve(enemies: impl IntoIterator<Item = BoardUnit>) -> Self {
        RoundKindDef::Pve { enemies: enemies.into_iter().collect(), gold: 0, items: 0, rewards: Vec::new() }
    }

    /// Gold for winning a [`RoundKindDef::Pve`] round. Other kinds are
    /// returned unchanged.
    pub fn gold(mut self, amount: i64) -> Self {
        if let RoundKindDef::Pve { gold, .. } = &mut self {
            *gold = amount;
        }
        self
    }

    /// Random component items for winning a [`RoundKindDef::Pve`] round.
    pub fn items(mut self, count: u32) -> Self {
        if let RoundKindDef::Pve { items, .. } = &mut self {
            *items = count;
        }
        self
    }

    /// Something else for winning a [`RoundKindDef::Pve`] round:
    /// `.reward(Grant::Xp(4))`, `.reward(Grant::Heal(5))`, a currency, a
    /// named item or an augment. Call it once per reward.
    pub fn reward(mut self, grant: Grant) -> Self {
        if let RoundKindDef::Pve { rewards, .. } = &mut self {
            rewards.push(grant);
        }
        self
    }

    /// A shared draft with `extra` offers beyond one per player.
    pub fn draft(extra: u32) -> Self {
        RoundKindDef::Draft { extra, max_cost: None }
    }

    /// Limits a [`RoundKindDef::Draft`] to units up to this cost.
    pub fn max_cost(mut self, cost: u32) -> Self {
        if let RoundKindDef::Draft { max_cost, .. } = &mut self {
            *max_cost = Some(cost);
        }
        self
    }

    pub fn choice(options: u32, offer: ChoiceOffer) -> Self {
        RoundKindDef::Choice { options, offer }
    }

    /// A round type registered by the game or defined by a script.
    pub fn custom(kind: impl Into<String>) -> Self {
        RoundKindDef::Custom { kind: kind.into(), params: BTreeMap::new() }
    }

    /// A named number for a [`RoundKindDef::Custom`] round.
    pub fn param(mut self, name: impl Into<String>, value: i64) -> Self {
        if let RoundKindDef::Custom { params, .. } = &mut self {
            params.insert(name.into(), value);
        }
        self
    }
}

impl RoundDef {
    pub fn new(name: impl Into<String>, stage: u32, kind: RoundKindDef) -> Self {
        RoundDef { name: name.into(), stage, kind, tiles: Vec::new() }
    }

    /// Starts every fight of the round with a tile on this hex, for both
    /// sides: `pos` is `(column, row)` as each side sees its own half, so
    /// neither has the better ground. A blocking tile is left out of a
    /// fight where a unit stands on its hex.
    pub fn tile(mut self, tile: impl Into<String>, pos: (i32, i32)) -> Self {
        self.tiles.push(TilePlacement { tile: tile.into(), pos, mirrored: true });
        self
    }

    /// Starts every fight of the round with a tile on exactly this hex, as
    /// the home side sees the board.
    pub fn tile_at(mut self, tile: impl Into<String>, pos: (i32, i32)) -> Self {
        self.tiles.push(TilePlacement { tile: tile.into(), pos, mirrored: false });
        self
    }
}

impl Schedule {
    /// A schedule that loops back to the first round when it runs out; see
    /// [`repeat_from`](Self::repeat_from).
    pub fn new(rounds: impl IntoIterator<Item = RoundDef>) -> Self {
        Schedule { rounds: rounds.into_iter().collect(), repeat_from: 0 }
    }

    /// Where the schedule loops back to (one stage higher each time).
    pub fn repeat_from(mut self, index: usize) -> Self {
        self.repeat_from = index;
        self
    }
}

/// A unit a player has placed on their board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardUnit {
    /// Id of the [`UnitDef`].
    pub unit: String,
    #[serde(default = "default_star")]
    pub star: u8,
    #[serde(default)]
    pub items: Vec<String>,
    /// `(column, row)` on the board, from the owning player's point of view.
    pub pos: (i32, i32),
    /// Stat changes this one unit has earned for good (see
    /// [`Effect::grow`](crate::effect::Effect::grow)).
    #[serde(default)]
    pub growth: Vec<StatMod>,
    /// Traits this one unit counts as having besides its own.
    #[serde(default)]
    pub traits: Vec<String>,
}

impl BoardUnit {
    /// A one-star unit with no items at `(column, row)`.
    pub fn new(unit: impl Into<String>, pos: (i32, i32)) -> Self {
        BoardUnit { unit: unit.into(), star: 1, items: Vec::new(), pos, growth: Vec::new(), traits: Vec::new() }
    }

    /// Adds stat changes the unit has earned for good.
    pub fn growth(mut self, mods: impl IntoIterator<Item = StatMod>) -> Self {
        self.growth.extend(mods);
        self
    }

    /// Makes this one unit count as having another trait.
    pub fn with_trait(mut self, id: impl Into<String>) -> Self {
        self.traits.push(id.into());
        self
    }

    pub fn star(mut self, star: u8) -> Self {
        self.star = star;
        self
    }

    pub fn item(mut self, id: impl Into<String>) -> Self {
        self.items.push(id.into());
        self
    }
}

/// One side of a fight.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TeamSetup {
    pub units: Vec<BoardUnit>,
    /// Extra team-wide bonuses that do not come from traits or items, such
    /// as augment-style modifiers a player has picked up.
    #[serde(default)]
    pub bonuses: Vec<Bonus>,
}

impl TeamSetup {
    pub fn new() -> Self {
        TeamSetup::default()
    }

    pub fn unit(mut self, unit: BoardUnit) -> Self {
        self.units.push(unit);
        self
    }

    pub fn bonus(mut self, bonus: Bonus) -> Self {
        self.bonuses.push(bonus);
        self
    }
}

/// A whole fight described in data: the teams and where they stand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchSetup {
    pub teams: Vec<TeamSetup>,
    /// [Tiles](crate::tile) the fight starts with.
    #[serde(default)]
    pub tiles: Vec<TilePlacement>,
}

impl MatchSetup {
    pub fn new(teams: impl IntoIterator<Item = TeamSetup>) -> Self {
        MatchSetup { teams: teams.into_iter().collect(), tiles: Vec::new() }
    }

    /// Starts the fight with a tile on exactly this hex, as the first team
    /// sees the board.
    pub fn tile_at(mut self, tile: impl Into<String>, pos: (i32, i32)) -> Self {
        self.tiles.push(TilePlacement { tile: tile.into(), pos, mirrored: false });
        self
    }
}

/// The state of one trait for a given board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveTrait {
    pub id: String,
    /// Distinct units on the board with this trait.
    pub count: u32,
    /// Index of the breakpoint reached, if any.
    pub tier: Option<usize>,
    /// Members needed for the next breakpoint, if there is one.
    pub next_at: Option<u32>,
}

/// A combat-ready unit and where it stands.
#[derive(Clone, Debug)]
pub struct Placement {
    pub template: Arc<UnitTemplate>,
    pub pos: Hex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentError {
    /// A script that does not compile, or scripts in a build without the
    /// `scripting` feature.
    Script { name: String, message: String },
    /// Content that breaks a rule: a dangling reference, a duplicate id, a
    /// unit that cannot be placed, and so on.
    Invalid(String),
}

impl std::fmt::Display for ContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentError::Script { name, message } => write!(f, "script `{name}`: {message}"),
            ContentError::Invalid(message) => write!(f, "invalid content: {message}"),
        }
    }
}

impl std::error::Error for ContentError {}

/// " (did you mean `x`?)" for the known id closest to a mistyped one, or
/// nothing if none is close.
fn suggestion<'a>(wrong: &str, known: impl Iterator<Item = &'a String>) -> String {
    fn distance(a: &str, b: &str) -> usize {
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for i in 1..=a.len() {
            let mut previous = row[0];
            row[0] = i;
            for j in 1..=b.len() {
                let swap = previous + usize::from(a[i - 1] != b[j - 1]);
                previous = row[j];
                row[j] = swap.min(row[j] + 1).min(row[j - 1] + 1);
            }
        }
        row[b.len()]
    }
    let limit = (wrong.chars().count() / 3).max(1);
    known
        .map(|id| (distance(wrong, id), id))
        .filter(|(d, _)| *d <= limit)
        .min()
        .map_or(String::new(), |(_, id)| format!(" (did you mean `{id}`?)"))
}

fn invalid<T>(message: String) -> Result<T, ContentError> {
    Err(ContentError::Invalid(message))
}

fn index<T>(
    kind: &str,
    defs: Vec<T>,
    id: impl Fn(&T) -> &str,
) -> Result<BTreeMap<String, T>, ContentError> {
    let mut map = BTreeMap::new();
    for def in defs {
        let key = id(&def).to_string();
        if key.is_empty() {
            return invalid(format!("a {kind} has an empty id"));
        }
        if map.insert(key.clone(), def).is_some() {
            return invalid(format!("duplicate {kind} id `{key}`"));
        }
    }
    Ok(map)
}

/// The Rust hooks a game registers, by name.
#[derive(Clone, Default)]
struct Hooks {
    effects: BTreeMap<String, Arc<CustomEffectFn>>,
    conditions: BTreeMap<String, Arc<CustomConditionFn>>,
    selectors: BTreeMap<String, Arc<CustomSelectorFn>>,
    actions: BTreeMap<String, Arc<ActionFn>>,
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hooks")
            .field("effects", &self.effects.keys().collect::<Vec<_>>())
            .field("conditions", &self.conditions.keys().collect::<Vec<_>>())
            .field("selectors", &self.selectors.keys().collect::<Vec<_>>())
            .field("actions", &self.actions.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Hooks are closures, so two sets are equal when they register the same names.
impl PartialEq for Hooks {
    fn eq(&self, other: &Self) -> bool {
        self.effects.keys().eq(other.effects.keys())
            && self.conditions.keys().eq(other.conditions.keys())
            && self.selectors.keys().eq(other.selectors.keys())
            && self.actions.keys().eq(other.actions.keys())
    }
}

impl Eq for Hooks {}

/// Collects definitions for a [`ContentSet`]; see the [module docs](self).
/// Definitions can be added in any order since references are only checked
/// by [`GameBuilder::build`].
#[derive(Clone, Debug, Default)]
pub struct GameBuilder {
    units: Vec<UnitDef>,
    traits: Vec<TraitDef>,
    items: Vec<ItemDef>,
    augments: Vec<AugmentDef>,
    schedule: Option<Schedule>,
    rules: GameRules,
    scripts: BTreeMap<String, String>,
    hooks: Hooks,
    translations: Translations,
}

/// Text in other languages: language code (such as `"fr"`) to key to text.
/// Keys name a definition and one of its texts, `"unit.ashen_hound.name"`;
/// see [`text_key`].
pub type Translations = BTreeMap<String, BTreeMap<String, String>>;

/// The kinds of definition that have translatable text.
pub const TEXT_KINDS: [&str; 8] = ["unit", "trait", "item", "augment", "resource", "currency", "action", "tile"];

/// The text fields of a unit's ability, by position: `"ability"` and
/// `"ability_description"` for the first, `"ability_2"` and
/// `"ability_2_description"` for the second, and so on.
pub fn ability_fields(index: usize) -> (String, String) {
    match index {
        0 => ("ability".into(), "ability_description".into()),
        i => (format!("ability_{}", i + 1), format!("ability_{}_description", i + 1)),
    }
}

/// The ability a text field of a unit belongs to, and whether the field is
/// its description.
fn ability_of_field(field: &str) -> Option<(usize, bool)> {
    let (rest, description) = match field.strip_suffix("_description") {
        Some(rest) => (rest, true),
        None => (field, false),
    };
    let index = match rest.strip_prefix("ability")? {
        "" => 0,
        number => number.strip_prefix('_')?.parse::<usize>().ok().filter(|n| *n >= 2)? - 1,
    };
    Some((index, description))
}

/// The translation key for one text of a definition: `kind` is one of
/// [`TEXT_KINDS`] and `field` is `"name"` or `"description"` (resources only
/// have a name), or for units also the fields [`ability_fields`] gives:
/// `"ability"` (the ability's name) or `"ability_description"`.
pub fn text_key(kind: &str, id: &str, field: &str) -> String {
    format!("{kind}.{id}.{field}")
}

impl GameBuilder {
    pub fn new() -> Self {
        GameBuilder::default()
    }

    /// Replaces all rules.
    pub fn rules(mut self, rules: GameRules) -> Self {
        self.rules = rules;
        self
    }

    pub fn board(mut self, cols: u32, rows: u32) -> Self {
        self.rules = self.rules.board(cols, rows);
        self
    }

    pub fn max_items_per_unit(mut self, n: usize) -> Self {
        self.rules = self.rules.max_items_per_unit(n);
        self
    }

    /// Adds a chance for shop offers to come with something extra (see
    /// [`ShopSpecial`]).
    pub fn shop_special(mut self, special: ShopSpecial) -> Self {
        self.rules.shop_specials.push(special);
        self
    }

    /// Splits players into teams of this many (see
    /// [`GameRules::team_size`]).
    pub fn team_size(mut self, players: u32) -> Self {
        self.rules.team_size = players;
        self
    }

    /// Stat modifiers for each star level, starting at one star.
    pub fn star_levels<L: IntoIterator<Item = StatMod>>(mut self, levels: impl IntoIterator<Item = L>) -> Self {
        self.rules = self.rules.star_levels(levels);
        self
    }

    /// Adds a resource units can build up and spend (see the
    /// [`resource`](crate::resource) module), or replaces the one with the
    /// same id: pass a changed [`ResourceDef::mana`] to retune mana.
    ///
    /// ```
    /// use autoengine::content::{ContentSet, UnitDef};
    /// use autoengine::effect::{Ability, Cost, Effect, Selector};
    /// use autoengine::resource::ResourceDef;
    /// use autoengine::stats::Stat;
    ///
    /// const MAX_ENERGY: Stat = Stat::custom("max_energy");
    ///
    /// let game = ContentSet::builder()
    ///     .resource(ResourceDef::new("energy", "Energy", MAX_ENERGY).starts_full().per_second(10))
    ///     .unit(UnitDef::new("duelist", "Duelist").stat(MAX_ENERGY, 100).ability(
    ///         Ability::new("Lunge")
    ///             .cost(Cost::of("energy", 40))
    ///             .effect(Effect::physical(120).to(Selector::CurrentTarget)),
    ///     ))
    ///     .build()
    ///     .unwrap();
    /// assert_eq!(game.resources().len(), 2); // mana is still there
    /// ```
    pub fn resource(mut self, def: ResourceDef) -> Self {
        let resources = &mut self.rules.combat.resources;
        match resources.iter_mut().find(|r| r.id == def.id) {
            Some(existing) => *existing = def,
            None => resources.push(def),
        }
        self
    }

    /// Adds a currency players hold besides gold, or replaces the one with
    /// the same id.
    pub fn currency(mut self, def: CurrencyDef) -> Self {
        let currencies = &mut self.rules.currencies;
        match currencies.iter_mut().find(|c| c.id == def.id) {
            Some(existing) => *existing = def,
            None => currencies.push(def),
        }
        self
    }

    /// Adds a kind of [tile](crate::tile), or replaces the one with the
    /// same id.
    ///
    /// ```
    /// use autoengine::content::{ContentSet, UnitDef};
    /// use autoengine::effect::{Ability, Effect, Selector};
    /// use autoengine::stats::Stat;
    /// use autoengine::tile::TileDef;
    ///
    /// let game = ContentSet::builder()
    ///     .tile(TileDef::new("embers", "Embers").enemies_only().per_second(Effect::magic(40)))
    ///     .unit(UnitDef::new("pyre", "Pyre").stat(Stat::MaxMana, 60).ability(
    ///         Ability::new("Scorch").effect(Effect::tile_for("embers", 3000).spread(1).to(Selector::CurrentTarget)),
    ///     ))
    ///     .build()
    ///     .unwrap();
    /// assert!(game.tile("embers").is_some());
    /// ```
    pub fn tile(mut self, def: TileDef) -> Self {
        let tiles = &mut self.rules.combat.tiles;
        match tiles.iter_mut().find(|t| t.id == def.id) {
            Some(existing) => *existing = def,
            None => tiles.push(def),
        }
        self
    }

    /// Adds several kinds of tile; see [`GameBuilder::tile`].
    pub fn tiles(mut self, defs: impl IntoIterator<Item = TileDef>) -> Self {
        for def in defs {
            self = self.tile(def);
        }
        self
    }

    /// Adds a [loot table](LootTable), or replaces the one with the same
    /// id.
    pub fn loot(mut self, table: LootTable) -> Self {
        let tables = &mut self.rules.loot;
        match tables.iter_mut().find(|t| t.id == table.id) {
            Some(existing) => *existing = table,
            None => tables.push(table),
        }
        self
    }

    /// Adds something players can do during planning (see [`ActionDef`]),
    /// or replaces the action with the same id.
    pub fn action(mut self, def: ActionDef) -> Self {
        let actions = &mut self.rules.actions;
        match actions.iter_mut().find(|a| a.id == def.id) {
            Some(existing) => *existing = def,
            None => actions.push(def),
        }
        self
    }

    /// Gives an action Rust code to run. The handler can refuse by
    /// returning an error (usually [`CommandError::CannotUse`] with the
    /// reason), and then the player is not charged.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use autoengine::content::{ActionDef, ContentSet};
    /// use autoengine::game::{Command, CommandError, Game, PlayerId};
    ///
    /// let content = ContentSet::builder()
    ///     .action(ActionDef::new("bloodprice", "Blood Price").description("Lose 5 health, gain 4 gold."))
    ///     .action_handler("bloodprice", |game, player, _arg| {
    ///         if game.player(player).health <= 5 {
    ///             return Err(CommandError::CannotUse("not enough health".into()));
    ///         }
    ///         game.damage_player(player, 5);
    ///         game.add_gold(player, 4);
    ///         Ok(())
    ///     })
    ///     .build()
    ///     .unwrap();
    /// let mut game = Game::new(Arc::new(content), 1, &["a", "b"]);
    /// game.begin_round().unwrap();
    /// let gold = game.player(PlayerId(0)).gold;
    /// game.apply(PlayerId(0), Command::Action { id: "bloodprice".into(), arg: 0 }).unwrap();
    /// assert_eq!(game.player(PlayerId(0)).gold, gold + 4);
    /// ```
    pub fn action_handler(
        mut self,
        id: impl Into<String>,
        handler: impl Fn(&mut Game, PlayerId, i64) -> Result<(), CommandError> + Send + Sync + 'static,
    ) -> Self {
        self.hooks.actions.insert(id.into(), Arc::new(handler));
        self
    }

    /// Describes a kind of damage (see the [`damage`](crate::damage)
    /// module): one of the game's own, or a built-in kind with other rules.
    ///
    /// ```
    /// use autoengine::content::{ContentSet, UnitDef};
    /// use autoengine::damage::DamageType;
    /// use autoengine::effect::{Ability, DamageKind, Effect, Selector};
    /// use autoengine::stats::Stat;
    ///
    /// const FIRE: DamageKind = DamageKind::custom("fire");
    /// const FIRE_RESIST: Stat = Stat::custom("fire_resist");
    ///
    /// let game = ContentSet::builder()
    ///     .damage_type(DamageType::new(FIRE, "Fire").resisted_by(FIRE_RESIST))
    ///     .unit(UnitDef::new("pyre", "Pyre").attack_kind(FIRE).stat(Stat::MaxMana, 60).ability(
    ///         Ability::new("Flare").effect(Effect::damage(150, FIRE).to(Selector::CurrentTarget)),
    ///     ))
    ///     .build()
    ///     .unwrap();
    /// assert_eq!(game.damage_types().len(), 4);
    /// ```
    pub fn damage_type(mut self, def: DamageType) -> Self {
        let types = &mut self.rules.combat.damage_types;
        match types.iter_mut().find(|t| t.kind == def.kind) {
            Some(existing) => *existing = def,
            None => types.push(def),
        }
        self
    }

    /// Takes a resource out of the game: `.without_resource("mana")` for a
    /// game that has no use for it.
    pub fn without_resource(mut self, id: &str) -> Self {
        self.rules.combat.resources.retain(|r| r.id != id);
        self
    }

    pub fn unit(mut self, unit: UnitDef) -> Self {
        self.units.push(unit);
        self
    }

    pub fn units(mut self, units: impl IntoIterator<Item = UnitDef>) -> Self {
        self.units.extend(units);
        self
    }

    pub fn trait_def(mut self, def: TraitDef) -> Self {
        self.traits.push(def);
        self
    }

    pub fn trait_defs(mut self, defs: impl IntoIterator<Item = TraitDef>) -> Self {
        self.traits.extend(defs);
        self
    }

    pub fn item(mut self, item: ItemDef) -> Self {
        self.items.push(item);
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = ItemDef>) -> Self {
        self.items.extend(items);
        self
    }

    pub fn augment(mut self, augment: AugmentDef) -> Self {
        self.augments.push(augment);
        self
    }

    pub fn augments(mut self, augments: impl IntoIterator<Item = AugmentDef>) -> Self {
        self.augments.extend(augments);
        self
    }

    /// The round schedule; without one the game is one PvP round, repeated.
    pub fn schedule(mut self, schedule: Schedule) -> Self {
        self.schedule = Some(schedule);
        self
    }

    /// Adds a Rhai script. A script named `x` defines the round kind `x`
    /// (see the `script` module). Pass the source with `include_str!` to
    /// keep it in its own file:
    /// `.script("payday", include_str!("scripts/payday.rhai"))`.
    pub fn script(mut self, name: impl Into<String>, source: impl Into<String>) -> Self {
        self.scripts.insert(name.into(), source.into());
        self
    }

    /// Registers a Rust hook for `Effect::Custom { name, .. }`, for logic
    /// the built-in effects cannot express. The hook runs once per selected
    /// unit and acts through the [`EffectContext`]. It must be deterministic:
    /// use [`EffectContext::rng`] for randomness.
    ///
    /// Hooks live in the process that built the content set, so a networked
    /// game needs the same hooks registered on the server and on any client
    /// that replays fights.
    ///
    /// ```
    /// use autoengine::content::{ContentSet, UnitDef};
    /// use autoengine::effect::{Ability, Effect, Selector};
    ///
    /// // Deals true damage equal to the gap in current health, if positive.
    /// let game = ContentSet::builder()
    ///     .custom_effect("equalize", |ctx| {
    ///         let me = ctx.combat().unit(ctx.source()).health();
    ///         let them = ctx.combat().unit(ctx.target()).health();
    ///         if them > me {
    ///             ctx.apply(ctx.target(), &Effect::true_damage(them - me));
    ///         }
    ///     })
    ///     .unit(UnitDef::new("judge", "Judge").ability(
    ///         Ability::new("Equalize").effect(Effect::custom("equalize").to(Selector::CurrentTarget)),
    ///     ))
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn custom_effect(
        mut self,
        name: impl Into<String>,
        hook: impl Fn(&mut EffectContext<'_>) + Send + Sync + 'static,
    ) -> Self {
        self.hooks.effects.insert(name.into(), Arc::new(hook));
        self
    }

    /// Registers a Rust hook that picks the units for
    /// `Selector::Custom(name)`, for targets the built-in selectors cannot
    /// describe. It must give the same units in the same order every time
    /// for the same fight.
    ///
    /// ```
    /// use autoengine::content::{ContentSet, UnitDef};
    /// use autoengine::effect::{Ability, Effect, Selector};
    ///
    /// // Every enemy standing in the same column as the caster.
    /// let game = ContentSet::builder()
    ///     .custom_selector("my_column", |ctx| {
    ///         let me = ctx.combat().unit(ctx.source());
    ///         let column = me.pos().to_offset().0;
    ///         let enemies = ctx.combat().units().iter().filter(|u| u.is_alive() && u.team() != me.team());
    ///         enemies.filter(|u| u.pos().to_offset().0 == column).map(|u| u.id()).collect()
    ///     })
    ///     .unit(UnitDef::new("lancer", "Lancer").ability(
    ///         Ability::new("Skewer").effect(Effect::physical(90).to(Selector::Custom("my_column".into()))),
    ///     ))
    ///     .build()
    ///     .unwrap();
    /// # let _ = game;
    /// ```
    pub fn custom_selector(
        mut self,
        name: impl Into<String>,
        hook: impl Fn(&SelectContext<'_>) -> Vec<UnitId> + Send + Sync + 'static,
    ) -> Self {
        self.hooks.selectors.insert(name.into(), Arc::new(hook));
        self
    }

    /// Registers a Rust predicate for `Condition::Custom(name)`.
    pub fn custom_condition(
        mut self,
        name: impl Into<String>,
        predicate: impl Fn(&ConditionContext<'_>) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.hooks.conditions.insert(name.into(), Arc::new(predicate));
        self
    }

    /// Validates everything and produces the content set.
    /// Text in another language for one key, such as
    /// `.translate("fr", "unit.ashen_hound.name", "Chien de cendre")`.
    pub fn translate(mut self, language: &str, key: &str, text: impl Into<String>) -> Self {
        self.translations.entry(language.to_string()).or_default().insert(key.to_string(), text.into());
        self
    }

    /// Many texts in one language, as `(key, text)` pairs.
    pub fn translations<K: Into<String>, T: Into<String>>(
        mut self,
        language: &str,
        texts: impl IntoIterator<Item = (K, T)>,
    ) -> Self {
        let table = self.translations.entry(language.to_string()).or_default();
        table.extend(texts.into_iter().map(|(k, t)| (k.into(), t.into())));
        self
    }

    pub fn build(self) -> Result<ContentSet, ContentError> {
        let set = ContentSet {
            units: index("unit", self.units, |u| &u.id)?,
            traits: index("trait", self.traits, |t| &t.id)?,
            items: index("item", self.items, |i| &i.id)?,
            augments: index("augment", self.augments, |a| &a.id)?,
            schedule: self.schedule.unwrap_or_default(),
            rules: self.rules,
            scripts: BTreeMap::new(),
            hooks: self.hooks,
            translations: self.translations,
        };
        set.validate()?;
        set.with_scripts(self.scripts)
    }
}

/// All the content of a game, validated and indexed by id.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ContentSet {
    units: BTreeMap<String, UnitDef>,
    traits: BTreeMap<String, TraitDef>,
    items: BTreeMap<String, ItemDef>,
    augments: BTreeMap<String, AugmentDef>,
    schedule: Schedule,
    rules: GameRules,
    /// Script sources by name (see the `script` module).
    scripts: BTreeMap<String, String>,
    /// Rust hooks registered with [`GameBuilder`].
    hooks: Hooks,
    translations: Translations,
}

impl ContentSet {
    /// Every translation, by language.
    pub fn translations(&self) -> &Translations {
        &self.translations
    }

    /// The languages there are translations for.
    pub fn languages(&self) -> impl Iterator<Item = &str> {
        self.translations.keys().map(String::as_str)
    }

    /// The same content with these translations instead, checked again.
    pub fn with_translations(mut self, translations: Translations) -> Result<Self, ContentError> {
        self.translations = translations;
        self.validate()?;
        Ok(self)
    }

    /// A definition's text in a language, falling back to the text the
    /// definition itself carries. `None` if there is no such definition.
    pub fn text(&self, language: Option<&str>, kind: &str, id: &str, field: &str) -> Option<String> {
        let own = match (kind, field) {
            ("unit", "name") => self.unit(id).map(|d| d.name.clone()),
            ("unit", field) => {
                let (index, description) = ability_of_field(field)?;
                let ability = self.unit(id)?.abilities.get(index)?;
                Some(if description { ability.description.clone() } else { ability.name.clone() })
            }
            ("item", "description") => self.item(id).map(|d| d.description.clone()),
            ("trait", "name") => self.trait_def(id).map(|d| d.name.clone()),
            ("trait", "description") => self.trait_def(id).map(|d| d.description.clone()),
            ("item", "name") => self.item(id).map(|d| d.name.clone()),
            ("augment", "name") => self.augment(id).map(|d| d.name.clone()),
            ("augment", "description") => self.augment(id).map(|d| d.description.clone()),
            ("resource", "name") => self.resource(id).map(|d| d.name.clone()),
            ("currency", "name") => self.currency(id).map(|d| d.name.clone()),
            ("action", "name") => self.action(id).map(|d| d.name.clone()),
            ("action", "description") => self.action(id).map(|d| d.description.clone()),
            ("tile", "name") => self.tile(id).map(|d| d.name.clone()),
            ("tile", "description") => self.tile(id).map(|d| d.description.clone()),
            _ => None,
        }?;
        let translated = language
            .and_then(|l| self.translations.get(l))
            .and_then(|table| table.get(&text_key(kind, id, field)));
        Some(translated.cloned().unwrap_or(own))
    }

    /// Every key a translation can give text for.
    pub fn text_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        for (id, unit) in &self.units {
            keys.push(text_key("unit", id, "name"));
            for index in 0..unit.abilities.len() {
                let (name, description) = ability_fields(index);
                keys.push(text_key("unit", id, &name));
                keys.push(text_key("unit", id, &description));
            }
        }
        for id in self.items.keys() {
            keys.push(text_key("item", id, "name"));
            keys.push(text_key("item", id, "description"));
        }
        for id in self.traits.keys() {
            keys.push(text_key("trait", id, "name"));
            keys.push(text_key("trait", id, "description"));
        }
        for id in self.augments.keys() {
            keys.push(text_key("augment", id, "name"));
            keys.push(text_key("augment", id, "description"));
        }
        for def in self.resources() {
            keys.push(text_key("resource", &def.id, "name"));
        }
        for def in self.currencies() {
            keys.push(text_key("currency", &def.id, "name"));
        }
        for def in self.actions() {
            keys.push(text_key("action", &def.id, "name"));
            keys.push(text_key("action", &def.id, "description"));
        }
        for def in self.tiles() {
            keys.push(text_key("tile", &def.id, "name"));
            keys.push(text_key("tile", &def.id, "description"));
        }
        keys
    }

    /// The name of whatever has this id (a unit, item, augment or trait, in
    /// that order) in a language, or `None` if nothing does.
    pub fn name_in(&self, language: Option<&str>, id: &str) -> Option<String> {
        TEXT_KINDS.iter().find_map(|kind| self.text(language, kind, id, "name"))
    }
    /// Starts defining a game in Rust.
    pub fn builder() -> GameBuilder {
        GameBuilder::new()
    }

    /// A builder that already holds everything in this content set, for
    /// making a variant of a game: the same game with one more unit, other
    /// rules, an extra resource. Adding a definition with an id that is
    /// already taken is an error when the variant is built.
    ///
    /// ```
    /// use autoengine::content::UnitDef;
    ///
    /// let base = autoengine::sample::content();
    /// let modded = base.clone().into_builder().unit(UnitDef::new("newcomer", "Newcomer").cost(1)).build().unwrap();
    /// assert_eq!(modded.units().count(), base.units().count() + 1);
    /// ```
    pub fn into_builder(self) -> GameBuilder {
        GameBuilder {
            units: self.units.into_values().collect(),
            traits: self.traits.into_values().collect(),
            items: self.items.into_values().collect(),
            augments: self.augments.into_values().collect(),
            schedule: Some(self.schedule),
            rules: self.rules,
            scripts: self.scripts,
            hooks: self.hooks,
            translations: self.translations,
        }
    }

    /// Adds scripts, given as name and source, after checking that they
    /// compile. A script named `x` defines the round kind `x`.
    pub fn with_scripts(mut self, scripts: BTreeMap<String, String>) -> Result<Self, ContentError> {
        #[cfg(feature = "scripting")]
        crate::script::ScriptHost::compile(&scripts)
            .map_err(|e| ContentError::Script { name: e.script, message: e.message })?;
        #[cfg(not(feature = "scripting"))]
        if let Some(name) = scripts.keys().next() {
            return Err(ContentError::Script {
                name: name.clone(),
                message: "this build of the engine has scripting turned off (the `scripting` cargo feature)".into(),
            });
        }
        self.scripts = scripts;
        Ok(self)
    }

    pub fn scripts(&self) -> &BTreeMap<String, String> {
        &self.scripts
    }

    /// Builds and validates a content set from already-parsed definitions.
    pub fn new(
        units: Vec<UnitDef>,
        traits: Vec<TraitDef>,
        items: Vec<ItemDef>,
        rules: GameRules,
    ) -> Result<Self, ContentError> {
        let set = ContentSet {
            units: index("unit", units, |u| &u.id)?,
            traits: index("trait", traits, |t| &t.id)?,
            items: index("item", items, |i| &i.id)?,
            augments: BTreeMap::new(),
            schedule: Schedule::default(),
            rules,
            scripts: BTreeMap::new(),
            hooks: Hooks::default(),
            translations: Translations::new(),
        };
        set.validate()?;
        Ok(set)
    }

    /// Adds augments and re-validates.
    pub fn with_augments(mut self, augments: Vec<AugmentDef>) -> Result<Self, ContentError> {
        self.augments = index("augment", augments, |a| &a.id)?;
        self.validate()?;
        Ok(self)
    }

    /// Replaces the round schedule and re-validates.
    pub fn with_schedule(mut self, schedule: Schedule) -> Result<Self, ContentError> {
        self.schedule = schedule;
        self.validate()?;
        Ok(self)
    }

    /// The same content under other rules, checked again.
    pub fn with_game_rules(mut self, rules: GameRules) -> Result<Self, ContentError> {
        self.rules = rules;
        self.validate()?;
        Ok(self)
    }

    fn validate(&self) -> Result<(), ContentError> {
        let r = &self.rules;
        if r.board_cols == 0 || r.board_rows == 0 {
            return invalid("the board must have at least one column and one row".into());
        }
        if r.star_levels.is_empty() {
            return invalid("rules need at least one star level".into());
        }
        if r.levels.is_empty() {
            return invalid("rules need at least one player level".into());
        }
        if r.levels.iter().any(|l| l.shop_odds.iter().all(|w| *w == 0)) {
            return invalid("every player level needs shop odds above zero".into());
        }
        if r.copies_to_upgrade < 2 {
            return invalid("copies_to_upgrade must be at least 2".into());
        }
        for (language, table) in &self.translations {
            for key in table.keys() {
                let parts: Vec<&str> = key.splitn(3, '.').collect();
                let known = match parts.as_slice() {
                    [kind, id, field] => match *kind {
                        "unit" => self.units.get(*id).is_some_and(|u| {
                            *field == "name" || ability_of_field(field).is_some_and(|(i, _)| i < u.abilities.len())
                        }),
                        "item" => self.items.contains_key(*id) && matches!(*field, "name" | "description"),
                        "trait" => self.traits.contains_key(*id) && matches!(*field, "name" | "description"),
                        "augment" => self.augments.contains_key(*id) && matches!(*field, "name" | "description"),
                        "resource" => self.resource(id).is_some() && *field == "name",
                        "currency" => self.currency(id).is_some() && *field == "name",
                        "action" => self.action(id).is_some() && matches!(*field, "name" | "description"),
                        "tile" => self.tile(id).is_some() && matches!(*field, "name" | "description"),
                        _ => false,
                    },
                    _ => false,
                };
                if !known {
                    let all: Vec<String> = self.text_keys();
                    return invalid(format!(
                        "translation `{key}` in `{language}` names nothing in the content{}",
                        suggestion(key, all.iter())
                    ));
                }
            }
        }
        for id in &r.lobby_modifiers {
            if !self.augments.contains_key(id) {
                return invalid(format!("lobby modifier `{id}` is not an augment{}", suggestion(id, self.augments.keys())));
            }
        }
        if r.combat.tick_ms == 0 || r.combat.max_duration_ms == 0 {
            return invalid("fights need a positive tick_ms and max_duration_ms".into());
        }
        if r.economy.interest_per <= 0 {
            return invalid("interest_per must be positive".into());
        }
        for (i, def) in r.currencies.iter().enumerate() {
            if def.id.is_empty() || def.id == "gold" {
                return invalid("a currency needs an id, and `gold` is taken: the engine has it built in".into());
            }
            if r.currencies[..i].iter().any(|c| c.id == def.id) {
                return invalid(format!("duplicate currency id `{}`", def.id));
            }
        }
        let currency_ids = || -> Vec<String> { std::iter::once("gold".to_string()).chain(r.currencies.iter().map(|c| c.id.clone())).collect() };
        // A pick has to have something it could offer.
        let check_offer = |owner: &str, offer: &ChoiceOffer| {
            let has_tier = |tier: u32| self.augments.values().any(|a| a.tier == tier);
            match offer {
                ChoiceOffer::AugmentsOfTier(tier) if !has_tier(*tier) => invalid(format!("{owner} offers augments of tier {tier}, but no augment has that tier")),
                ChoiceOffer::AugmentTiers(tiers) if tiers.iter().map(|(_, w)| *w as u64).sum::<u64>() == 0 => {
                    invalid(format!("{owner} rolls for an augment tier, but no tier has a weight above 0"))
                }
                ChoiceOffer::AugmentTiers(tiers) => match tiers.iter().find(|(tier, weight)| *weight > 0 && !has_tier(*tier)) {
                    Some((tier, _)) => invalid(format!("{owner} can roll augment tier {tier}, but no augment has that tier")),
                    None => Ok(()),
                },
                ChoiceOffer::ItemsTagged(tag) if !self.items.values().any(|i| i.tags.contains(tag)) => {
                    let known: BTreeSet<&String> = self.items.values().flat_map(|i| &i.tags).collect();
                    invalid(format!("{owner} offers items tagged `{tag}`, but no item has that tag{}", suggestion(tag, known.into_iter())))
                }
                ChoiceOffer::Units { cost } if !self.units.values().any(|u| u.in_shop && u.cost == *cost) => {
                    invalid(format!("{owner} offers units of cost {cost}, but no shop unit costs that"))
                }
                _ => Ok(()),
            }
        };
        // Whatever hands out a grant must name things that exist.
        let check_grant = |owner: &str, grant: &Grant| {
            match grant {
                Grant::Unit(id) if !self.units.contains_key(id) => {
                    return invalid(format!("{owner} gives unknown unit `{id}`{}", suggestion(id, self.units.keys())));
                }
                Grant::Item(id) if !self.items.contains_key(id) => {
                    return invalid(format!("{owner} gives unknown item `{id}`{}", suggestion(id, self.items.keys())));
                }
                Grant::Augment(id) if !self.augments.contains_key(id) => {
                    return invalid(format!("{owner} gives unknown augment `{id}`{}", suggestion(id, self.augments.keys())));
                }
                Grant::Currency { currency, .. } if currency != "gold" && self.currency(currency).is_none() => {
                    let known = currency_ids();
                    return invalid(format!("{owner} gives unknown currency `{currency}`{}", suggestion(currency, known.iter())));
                }
                Grant::Loot(id) if self.loot(id).is_none() => {
                    let known: Vec<&String> = r.loot.iter().map(|t| &t.id).collect();
                    return invalid(format!("{owner} rolls on unknown loot table `{id}`{}", suggestion(id, known.into_iter())));
                }
                Grant::ItemTagged(tag) if !self.items.values().any(|i| i.tags.contains(tag)) => {
                    let known: BTreeSet<&String> = self.items.values().flat_map(|i| &i.tags).collect();
                    return invalid(format!("{owner} gives an item tagged `{tag}`, but no item has that tag{}", suggestion(tag, known.into_iter())));
                }
                Grant::Choice { options: 0, .. } => return invalid(format!("{owner} opens a pick with no options")),
                Grant::Choice { offer, .. } => check_offer(owner, offer)?,
                _ => {}
            }
            Ok(())
        };
        if r.team_size == 0 {
            return invalid("rules have a team size of 0; it must be at least 1".into());
        }
        for special in &r.shop_specials {
            if special.star as usize > r.star_levels.len() {
                return invalid(format!("a shop special offers {} star units, but rules define levels 1 to {}", special.star, r.star_levels.len()));
            }
            for t in &special.traits {
                if !self.traits.contains_key(t) {
                    return invalid(format!("a shop special gives unknown trait `{t}`{}", suggestion(t, self.traits.keys())));
                }
            }
        }
        for (i, table) in r.loot.iter().enumerate() {
            if table.id.is_empty() {
                return invalid("a loot table needs an id".into());
            }
            if r.loot[..i].iter().any(|t| t.id == table.id) {
                return invalid(format!("duplicate loot table id `{}`", table.id));
            }
            if table.entries.iter().map(|e| e.weight as u64).sum::<u64>() == 0 {
                return invalid(format!("loot table `{}` has nothing with a weight above 0, so it never gives anything", table.id));
            }
            for grant in table.entries.iter().flat_map(|e| &e.gives) {
                check_grant(&format!("loot table `{}`", table.id), grant)?;
            }
        }
        // Perks hand out grants too, wherever they sit.
        let check_perks = |owner: &str, perks: &[Perk]| {
            for grant in perks.iter().flat_map(|p| &p.gives) {
                check_grant(owner, grant)?;
            }
            Ok(())
        };
        for augment in self.augments.values() {
            check_perks(&format!("augment `{}`", augment.id), &augment.perks)?;
        }
        for def in self.traits.values() {
            for tier in &def.breakpoints {
                check_perks(&format!("trait `{}`", def.id), &tier.perks)?;
            }
        }
        for item in self.items.values() {
            check_perks(&format!("item `{}`", item.id), &item.perks)?;
            if let Some(better) = &item.upgrades_to {
                if !self.items.contains_key(better) {
                    return invalid(format!("item `{}` upgrades to unknown item `{better}`{}", item.id, suggestion(better, self.items.keys())));
                }
            }
        }
        for unit in self.units.values() {
            if unit.team_slots == 0 {
                return invalid(format!("unit `{}` takes 0 team slots; every unit takes at least 1", unit.id));
            }
        }
        for round in &self.schedule.rounds {
            if let RoundKindDef::Choice { offer, .. } = &round.kind {
                check_offer(&format!("round `{}`", round.name), offer)?;
            }
        }
        for round in &self.schedule.rounds {
            if let RoundKindDef::Pve { rewards, .. } = &round.kind {
                for grant in rewards {
                    check_grant(&format!("round `{}`", round.name), grant)?;
                }
            }
        }
        for item in self.items.values() {
            if let Some(Consumable::Give(grant)) = &item.consumable {
                check_grant(&format!("item `{}`", item.id), grant)?;
            }
        }
        for (i, def) in r.actions.iter().enumerate() {
            let owner = format!("action `{}`", def.id);
            if def.id.is_empty() {
                return invalid("an action needs an id".into());
            }
            if r.actions[..i].iter().any(|a| a.id == def.id) {
                return invalid(format!("duplicate action id `{}`", def.id));
            }
            for price in &def.prices {
                if price.currency != "gold" && self.currency(&price.currency).is_none() {
                    let known = currency_ids();
                    return invalid(format!("{owner} costs unknown currency `{}`{}", price.currency, suggestion(&price.currency, known.iter())));
                }
                if price.amount <= 0 {
                    return invalid(format!("{owner} has a price of {} {}; a price must be positive (give things with `gives`)", price.amount, price.currency));
                }
            }
            if def.per_round == Some(0) {
                return invalid(format!("{owner} can be used 0 times a round, so never"));
            }
            for grant in &def.gives {
                check_grant(&owner, grant)?;
            }
        }
        for id in self.hooks.actions.keys() {
            if !r.actions.iter().any(|a| &a.id == id) {
                let known: Vec<String> = r.actions.iter().map(|a| a.id.clone()).collect();
                return invalid(format!("there is a handler for the action `{id}`, but no such action{}", suggestion(id, known.iter())));
            }
        }
        let resources = &r.combat.resources;
        let check_resource = |owner: &str, id: &str| {
            if resources.iter().any(|d| d.id == id) {
                Ok(())
            } else {
                let known: Vec<String> = resources.iter().map(|d| d.id.clone()).collect();
                invalid(format!("{owner} refers to unknown resource `{id}`{}", suggestion(id, known.iter())))
            }
        };
        let tiles = &r.combat.tiles;
        let check_tile = |owner: &str, id: &str| {
            if tiles.iter().any(|t| t.id == id) {
                Ok(())
            } else {
                let known: Vec<String> = tiles.iter().map(|t| t.id.clone()).collect();
                invalid(format!("{owner} refers to unknown tile `{id}`{}", suggestion(id, known.iter())))
            }
        };
        let damage_types = &r.combat.damage_types;
        let check_damage_kind = |owner: &str, kind: DamageKind| {
            if damage_types.iter().any(|t| t.kind == kind) {
                Ok(())
            } else {
                let known: Vec<String> = damage_types.iter().map(|t| t.kind.id().to_string()).collect();
                invalid(format!(
                    "{owner} deals `{}` damage, which the game does not describe (see `damage_type`){}",
                    kind.id(),
                    suggestion(kind.id(), known.iter())
                ))
            }
        };
        for (i, def) in damage_types.iter().enumerate() {
            if damage_types[..i].iter().any(|t| t.kind == def.kind) {
                return invalid(format!("damage type `{}` is described twice", def.kind.id()));
            }
        }
        let check_amount = |owner: &str, amount: &Amount| {
            amount.scalings.iter().try_for_each(|s| match &s.by {
                ScaleBy::Resource(id) => check_resource(owner, id),
                _ => Ok(()),
            })
        };
        for (i, def) in resources.iter().enumerate() {
            if def.id.is_empty() {
                return invalid("a resource has an empty id".into());
            }
            if resources[..i].iter().any(|d| d.id == def.id) {
                return invalid(format!("duplicate resource id `{}`", def.id));
            }
            let owner = format!("resource `{}`", def.id);
            for amount in [&def.start, &def.per_attack, &def.per_kill, &def.per_second] {
                check_amount(&owner, amount)?;
            }
        }
        let check_trait = |owner: &str, id: &str| {
            if self.traits.contains_key(id) {
                Ok(())
            } else {
                invalid(format!("{owner} refers to unknown trait `{id}`{}", suggestion(id, self.traits.keys())))
            }
        };
        let check_condition = |owner: &str, condition: Option<&Condition>| {
            let mut result = Ok(());
            if let Some(c) = condition {
                c.walk(&mut |c| {
                    if result.is_err() {
                        return;
                    }
                    match c {
                        Condition::Custom(name) if !self.hooks.conditions.contains_key(name) => {
                            result = invalid(format!("{owner} uses unregistered custom condition `{name}`"));
                        }
                        Condition::ResourceAtLeast { resource, .. } => result = check_resource(owner, resource),
                        Condition::OnTile { tile, .. } => result = check_tile(owner, tile),
                        _ => {}
                    }
                });
            }
            result
        };
        let check_effects = |owner: &str, effects: &[EffectSpec]| {
            for spec in effects {
                check_condition(owner, spec.condition.as_ref())?;
                for amount in spec.effect.amounts() {
                    check_amount(owner, amount)?;
                }
                if let Selector::Custom(name) = &spec.target
                    && !self.hooks.selectors.contains_key(name)
                {
                    return invalid(format!("{owner} uses unregistered custom selector `{name}`"));
                }
                if let Some(repeat) = spec.repeat
                    && (repeat.times == 0 || repeat.every_ms == 0)
                {
                    return invalid(format!("{owner} repeats an effect {} times every {} ms; both must be at least 1", repeat.times, repeat.every_ms));
                }
                if let Effect::Damage { kind, .. }
                | Effect::ApplyStatus { status: StatusKind::DamageOverTime { kind, .. }, .. } = &spec.effect
                {
                    check_damage_kind(owner, *kind)?;
                }
                match &spec.effect {
                    Effect::GainResource { resource, .. } => check_resource(owner, resource)?,
                    Effect::Summon { unit, .. } if !self.units.contains_key(unit) => {
                        return invalid(format!(
                            "{owner} summons unknown unit `{unit}`{}",
                            suggestion(unit, self.units.keys())
                        ));
                    }
                    Effect::ApplyStatus { status: StatusKind::StatMods(mods), .. } if mods.is_empty() => {
                        return invalid(format!("{owner} applies a stat status with no modifiers"));
                    }
                    Effect::AddStacks { stack, .. } | Effect::ClearStacks { stack } if stack.is_empty() => {
                        return invalid(format!("{owner} uses a stack with an empty name"));
                    }
                    Effect::Custom { name, .. } if !self.hooks.effects.contains_key(name) => {
                        return invalid(format!("{owner} uses unregistered custom effect `{name}`"));
                    }
                    Effect::PlaceTile { tile, .. } => check_tile(owner, tile)?,
                    Effect::Transform { unit } if !self.units.contains_key(unit) => {
                        return invalid(format!("{owner} transforms into unknown unit `{unit}`{}", suggestion(unit, self.units.keys())));
                    }
                    Effect::Reward { grant } => check_grant(owner, grant)?,
                    Effect::Grow { mods } if mods.is_empty() => {
                        return invalid(format!("{owner} grows a unit by nothing: give `Effect::grow` some stat changes"));
                    }
                    _ => {}
                }
            }
            Ok(())
        };
        let mut tile_ids = BTreeSet::new();
        for tile in &r.combat.tiles {
            let owner = format!("tile `{}`", tile.id);
            if tile.id.is_empty() {
                return invalid("a tile needs an id".into());
            }
            if !tile_ids.insert(&tile.id) {
                return invalid(format!("duplicate tile id `{}`", tile.id));
            }
            if tile.blocks && (tile.effects().next().is_some() || !tile.mods.is_empty()) {
                return invalid(format!("{owner} blocks its hex, so nothing can ever stand on it to get its effects"));
            }
            // A tile's effects happen to the unit on it.
            let specs: Vec<EffectSpec> = tile.effects().map(|e| e.clone().to(Selector::Source)).collect();
            check_effects(&owner, &specs)?;
        }
        for round in &self.schedule.rounds {
            for placement in &round.tiles {
                let owner = format!("round `{}`", round.name);
                check_tile(&owner, &placement.tile)?;
                let (col, row) = placement.pos;
                if !(0..r.board_cols as i32).contains(&col) || !(0..r.board_rows as i32).contains(&row) {
                    return invalid(format!("{owner} puts tile `{}` at {:?}, which is off the board", placement.tile, placement.pos));
                }
            }
        }
        let check_passives = |owner: &str, passives: &[Passive]| {
            passives.iter().try_for_each(|p| {
                check_condition(owner, p.condition.as_ref())?;
                check_effects(owner, &p.effects)
            })
        };

        for u in self.units.values() {
            if let crate::effect::Targeting::Custom(name) = u.targeting
                && !self.hooks.selectors.contains_key(name.as_str())
            {
                let known: Vec<String> = self.hooks.selectors.keys().cloned().collect();
                return invalid(format!("unit `{}` targets with unregistered custom selector `{name}`{}", u.id, suggestion(name.as_str(), known.iter())));
            }
            let owner = format!("unit `{}`", u.id);
            for t in &u.traits {
                check_trait(&owner, t)?;
            }
            check_damage_kind(&owner, u.attack_kind)?;
            for a in &u.abilities {
                let owner = format!("{owner}'s ability `{}`", a.name);
                check_effects(&owner, &a.effects)?;
                match &a.cost {
                    Cost::Full(resource) => check_resource(&owner, resource)?,
                    Cost::Amount { resource, amount } => {
                        check_resource(&owner, resource)?;
                        if *amount <= 0 {
                            return invalid(format!("{owner} costs {amount} {resource}; use `Cost::Free` for an ability that costs nothing"));
                        }
                    }
                    Cost::Free if a.cooldown_ms == 0 => {
                        return invalid(format!("{owner} is free and has no cooldown, so the unit would do nothing but cast"));
                    }
                    Cost::Free => {}
                }
            }
            check_passives(&owner, &u.passives)?;
        }
        for t in self.traits.values() {
            let owner = format!("trait `{}`", t.id);
            if t.breakpoints.is_empty() {
                return invalid(format!("{owner} has no breakpoints"));
            }
            if !t.breakpoints.windows(2).all(|w| w[0].count < w[1].count) || t.breakpoints[0].count == 0 {
                return invalid(format!("{owner} breakpoints must be positive and increasing"));
            }
            for bonus in t.breakpoints.iter().flat_map(|b| &b.bonuses) {
                if let Scope::WithTrait(other) = &bonus.scope {
                    check_trait(&owner, other)?;
                }
                check_passives(&owner, &bonus.passives)?;
            }
        }
        for i in self.items.values() {
            let owner = format!("item `{}`", i.id);
            for t in &i.grants_traits {
                check_trait(&owner, t)?;
            }
            check_passives(&owner, &i.passives)?;
            if let Some((a, b)) = &i.recipe {
                for part in [a, b] {
                    if !self.items.contains_key(part) {
                        return invalid(format!("{owner} is built from unknown item `{part}`{}", suggestion(part, self.items.keys())));
                    }
                    if self.items[part].consumable.is_some() {
                        return invalid(format!("{owner} is built from `{part}`, which is used up instead of being equipped"));
                    }
                }
            }
        }
        for a in self.augments.values() {
            let owner = format!("augment `{}`", a.id);
            for id in a.per_round.currencies.keys() {
                if self.currency(id).is_none() {
                    let known: Vec<String> = r.currencies.iter().map(|c| c.id.clone()).collect();
                    return invalid(format!("{owner} pays out unknown currency `{id}`{}", suggestion(id, known.iter())));
                }
            }
            for bonus in &a.bonuses {
                if let Scope::WithTrait(t) = &bonus.scope {
                    check_trait(&owner, t)?;
                }
                check_passives(&owner, &bonus.passives)?;
            }
        }
        let sched = &self.schedule;
        if sched.rounds.is_empty() || sched.repeat_from >= sched.rounds.len() {
            return invalid("the round schedule needs at least one round and a valid repeat_from".into());
        }
        for round in &sched.rounds {
            if let RoundKindDef::Pve { enemies, .. } = &round.kind {
                let team = TeamSetup { units: enemies.clone(), bonuses: vec![] };
                if let Err(e) = self.setup_combat(0, &[TeamSetup::default(), team]) {
                    return invalid(format!("round `{}`: {e}", round.name));
                }
            }
            if let RoundKindDef::Choice { options: 0, .. } = &round.kind {
                return invalid(format!("round `{}` offers no options", round.name));
            }
        }
        Ok(())
    }

    // ----- lookup ---------------------------------------------------------

    pub fn rules(&self) -> &GameRules {
        &self.rules
    }
    /// The resources units can have, in the order fights list them.
    pub fn resources(&self) -> &[ResourceDef] {
        &self.rules.combat.resources
    }
    pub fn resource(&self, id: &str) -> Option<&ResourceDef> {
        self.resources().iter().find(|r| r.id == id)
    }
    /// The currencies players hold besides gold.
    pub fn currencies(&self) -> &[CurrencyDef] {
        &self.rules.currencies
    }
    pub fn currency(&self, id: &str) -> Option<&CurrencyDef> {
        self.currencies().iter().find(|c| c.id == id)
    }

    /// The kinds of [tile](crate::tile) fights can have on the board.
    pub fn tiles(&self) -> &[TileDef] {
        &self.rules.combat.tiles
    }

    pub fn tile(&self, id: &str) -> Option<&TileDef> {
        self.tiles().iter().find(|t| t.id == id)
    }

    /// The game's [loot tables](LootTable).
    pub fn loot_tables(&self) -> &[LootTable] {
        &self.rules.loot
    }

    pub fn loot(&self, id: &str) -> Option<&LootTable> {
        self.loot_tables().iter().find(|t| t.id == id)
    }

    /// What players can do during planning besides the built-in commands.
    pub fn actions(&self) -> &[ActionDef] {
        &self.rules.actions
    }

    pub fn action(&self, id: &str) -> Option<&ActionDef> {
        self.actions().iter().find(|a| a.id == id)
    }

    /// The Rust handler of an action, if it has one.
    pub fn action_handler(&self, id: &str) -> Option<Arc<ActionFn>> {
        self.hooks.actions.get(id).cloned()
    }
    /// The rules for every kind of damage in the game.
    pub fn damage_types(&self) -> &[DamageType] {
        &self.rules.combat.damage_types
    }
    pub fn damage_type(&self, kind: DamageKind) -> Option<&DamageType> {
        self.damage_types().iter().find(|t| t.kind == kind)
    }
    pub fn unit(&self, id: &str) -> Option<&UnitDef> {
        self.units.get(id)
    }
    pub fn trait_def(&self, id: &str) -> Option<&TraitDef> {
        self.traits.get(id)
    }
    pub fn item(&self, id: &str) -> Option<&ItemDef> {
        self.items.get(id)
    }
    /// All units, ordered by id.
    pub fn units(&self) -> impl Iterator<Item = &UnitDef> {
        self.units.values()
    }
    /// All traits, ordered by id.
    pub fn traits(&self) -> impl Iterator<Item = &TraitDef> {
        self.traits.values()
    }
    /// All items, ordered by id.
    pub fn items(&self) -> impl Iterator<Item = &ItemDef> {
        self.items.values()
    }
    pub fn augment(&self, id: &str) -> Option<&AugmentDef> {
        self.augments.get(id)
    }
    /// All augments, ordered by id.
    pub fn augments(&self) -> impl Iterator<Item = &AugmentDef> {
        self.augments.values()
    }
    pub fn schedule(&self) -> &Schedule {
        &self.schedule
    }
    pub fn board(&self) -> HexBoard {
        HexBoard::new(self.rules.board_cols, self.rules.board_rows)
    }
    /// The item made by combining two components, in either order.
    pub fn combine_items(&self, a: &str, b: &str) -> Option<&ItemDef> {
        self.items.values().find(|i| {
            i.recipe.as_ref().is_some_and(|(x, y)| (x == a && y == b) || (x == b && y == a))
        })
    }

    /// Every passive in the content, each with what carries it (such as
    /// `unit ashen_hound` or `trait grove`), for tools that look across
    /// everything.
    pub fn passives(&self) -> Vec<(String, &Passive)> {
        fn add<'a>(out: &mut Vec<(String, &'a Passive)>, owner: String, passives: &'a [Passive]) {
            out.extend(passives.iter().map(|p| (owner.clone(), p)));
        }
        let mut out = Vec::new();
        for u in self.units.values() {
            add(&mut out, format!("unit {}", u.id), &u.passives);
        }
        for t in self.traits.values() {
            for bonus in t.breakpoints.iter().flat_map(|b| &b.bonuses) {
                add(&mut out, format!("trait {}", t.id), &bonus.passives);
            }
        }
        for i in self.items.values() {
            add(&mut out, format!("item {}", i.id), &i.passives);
        }
        for a in self.augments.values() {
            for bonus in &a.bonuses {
                add(&mut out, format!("augment {}", a.id), &bonus.passives);
            }
        }
        out
    }

    /// Every effect in the content, from abilities and passives alike, each
    /// with what carries it.
    pub fn effects(&self) -> Vec<(String, &EffectSpec)> {
        let mut out = Vec::new();
        for u in self.units.values() {
            for a in &u.abilities {
                out.extend(a.effects.iter().map(|e| (format!("unit {}", u.id), e)));
            }
        }
        for (owner, passive) in self.passives() {
            out.extend(passive.effects.iter().map(|e| (owner.clone(), e)));
        }
        out
    }

    /// Every [grant](Grant) the content hands out, with what hands it out:
    /// round rewards, single-use items, actions, perks, loot tables and
    /// `Effect::reward`. For tools; picks a grant opens are not followed.
    pub fn grants(&self) -> Vec<(String, &Grant)> {
        let mut out: Vec<(String, &Grant)> = Vec::new();
        for round in &self.schedule.rounds {
            if let RoundKindDef::Pve { rewards, .. } = &round.kind {
                out.extend(rewards.iter().map(|g| (format!("round {}", round.name), g)));
            }
        }
        for item in self.items.values() {
            if let Some(Consumable::Give(g)) = &item.consumable {
                out.push((format!("item {}", item.id), g));
            }
            out.extend(item.perks.iter().flat_map(|p| &p.gives).map(|g| (format!("item {}", item.id), g)));
        }
        for action in &self.rules.actions {
            out.extend(action.gives.iter().map(|g| (format!("action {}", action.id), g)));
        }
        for augment in self.augments.values() {
            out.extend(augment.perks.iter().flat_map(|p| &p.gives).map(|g| (format!("augment {}", augment.id), g)));
        }
        for t in self.traits.values() {
            out.extend(t.breakpoints.iter().flat_map(|b| &b.perks).flat_map(|p| &p.gives).map(|g| (format!("trait {}", t.id), g)));
        }
        for table in &self.rules.loot {
            out.extend(table.entries.iter().flat_map(|e| &e.gives).map(|g| (format!("loot table {}", table.id), g)));
        }
        for (owner, spec) in self.effects() {
            if let Effect::Reward { grant } = &spec.effect {
                out.push((owner, grant));
            }
        }
        out
    }

    // ----- building teams -------------------------------------------------

    fn unit_def(&self, id: &str) -> Result<&UnitDef, ContentError> {
        self.units.get(id).ok_or_else(|| ContentError::Invalid(format!("unknown unit `{id}`{}", suggestion(id, self.units.keys()))))
    }

    fn item_def(&self, id: &str) -> Result<&ItemDef, ContentError> {
        self.items.get(id).ok_or_else(|| ContentError::Invalid(format!("unknown item `{id}`{}", suggestion(id, self.items.keys()))))
    }

    /// The traits a board unit counts as having: its own plus any granted
    /// by its items.
    fn traits_of(&self, unit: &BoardUnit) -> Result<BTreeSet<&str>, ContentError> {
        let mut out: BTreeSet<&str> = self.unit_def(&unit.unit)?.traits.iter().map(String::as_str).collect();
        for item in &unit.items {
            out.extend(self.item_def(item)?.grants_traits.iter().map(String::as_str));
        }
        for extra in &unit.traits {
            // The set borrows the trait's own id, not the board unit's copy.
            match self.traits.get_key_value(extra) {
                Some((id, _)) => out.insert(id.as_str()),
                None => {
                    return invalid(format!("unit `{}` is given unknown trait `{extra}`{}", unit.unit, suggestion(extra, self.traits.keys())));
                }
            };
        }
        Ok(out)
    }

    /// Counts traits for a board. Several copies of the same unit count once.
    /// Traits with no members on the board are omitted; the rest are ordered
    /// by id.
    pub fn active_traits(&self, units: &[BoardUnit]) -> Result<Vec<ActiveTrait>, ContentError> {
        let mut members: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for unit in units {
            for t in self.traits_of(unit)? {
                members.entry(t).or_default().insert(&unit.unit);
            }
        }
        Ok(members
            .into_iter()
            .map(|(id, who)| {
                let count = who.len() as u32;
                let breakpoints = &self.traits[id].breakpoints;
                let tier = self.traits[id].tier(count);
                let next_at = breakpoints.iter().map(|b| b.count).find(|c| *c > count);
                ActiveTrait { id: id.to_string(), count, tier, next_at }
            })
            .collect())
    }

    /// A unit's template at a star level, with no items or trait bonuses.
    pub fn base_template(&self, unit_id: &str, star: u8) -> Result<UnitTemplate, ContentError> {
        let def = self.unit_def(unit_id)?;
        let Some(star_mods) = (star as usize).checked_sub(1).and_then(|i| self.rules.star_levels.get(i)) else {
            return invalid(format!(
                "unit `{unit_id}` has star level {star}, but rules define levels 1 to {}",
                self.rules.star_levels.len()
            ));
        };
        let mut stats = StatBlock::base();
        for (stat, value) in &def.stats {
            stats.set(*stat, *value);
        }
        Ok(UnitTemplate {
            id: def.id.clone(),
            name: def.name.clone(),
            stats: stats.with_mods(star_mods),
            abilities: def.abilities.clone(),
            passives: def.passives.clone(),
            tags: def.traits.clone(),
            targeting: def.targeting,
            attack_kind: def.attack_kind,
        })
    }

    /// What casting one of its abilities (by position, the first is 0)
    /// costs a unit at a star level, before items and traits: the resource
    /// and how much of it. `None` for an ability that is free or is not
    /// there.
    pub fn ability_cost(&self, unit_id: &str, star: u8, ability: usize) -> Option<(&ResourceDef, i64)> {
        let template = self.base_template(unit_id, star).ok()?;
        let ability = template.abilities.get(ability)?;
        let resource = self.resource(ability.cost.resource()?)?;
        let amount = match ability.cost {
            Cost::Amount { amount, .. } => amount,
            _ => template.stats.get(resource.max),
        };
        Some((resource, amount))
    }

    /// Turns one player's board into combat-ready units, folding in star
    /// levels, items, active trait bonuses and team bonuses.
    pub fn build_team(&self, team: &TeamSetup) -> Result<Vec<Placement>, ContentError> {
        let active = self.active_traits(&team.units)?;
        let count_of = |t: &str| active.iter().find(|a| a.id == t).map_or(0, |a| a.count) as i64;
        let distinct_units = team.units.iter().map(|u| u.unit.as_str()).collect::<BTreeSet<_>>().len() as i64;
        // (trait the bonus came from, bonus)
        let mut bonuses: Vec<(Option<&str>, &Bonus)> = Vec::new();
        for a in &active {
            if let Some(tier) = a.tier {
                let def = &self.traits[&a.id];
                bonuses.extend(def.breakpoints[tier].bonuses.iter().map(|b| (Some(def.id.as_str()), b)));
            }
        }
        bonuses.extend(team.bonuses.iter().map(|b| (None, b)));

        let mut out = Vec::with_capacity(team.units.len());
        for unit in &team.units {
            if unit.items.len() > self.rules.max_items_per_unit {
                return invalid(format!(
                    "unit `{}` holds {} items, the limit is {}",
                    unit.unit,
                    unit.items.len(),
                    self.rules.max_items_per_unit
                ));
            }
            let mut template = self.base_template(&unit.unit, unit.star)?;
            let traits = self.traits_of(unit)?;
            let mut mods: Vec<StatMod> = unit.growth.clone();
            for item in &unit.items {
                let item = self.item_def(item)?;
                mods.extend(&item.mods);
                template.passives.extend(item.passives.iter().cloned());
            }
            for (source, bonus) in &bonuses {
                let applies = match (&bonus.scope, source) {
                    (Scope::Team, _) | (Scope::Members, None) => true,
                    (Scope::Members, Some(t)) => traits.contains(t),
                    (Scope::WithTrait(t), _) => traits.contains(t.as_str()),
                };
                if applies {
                    mods.extend(&bonus.mods);
                    if !bonus.mods_per_member.is_empty() {
                        let members = match (source, &bonus.scope) {
                            (Some(t), _) => count_of(t),
                            (None, Scope::WithTrait(t)) => count_of(t),
                            (None, _) => distinct_units,
                        };
                        mods.extend(bonus.mods_per_member.iter().map(|m| StatMod {
                            stat: m.stat,
                            flat: m.flat * members,
                            permille: m.permille * members,
                        }));
                    }
                    template.passives.extend(bonus.passives.iter().cloned());
                }
            }
            template.stats = template.stats.with_mods(&mods);
            template.tags = traits.into_iter().map(str::to_string).collect();
            out.push(Placement { template: Arc::new(template), pos: Hex::from_offset(unit.pos.0, unit.pos.1) });
        }
        Ok(out)
    }

    /// Sets up a fight between teams. Positions are given from each player's
    /// own point of view; the first team is placed as written and the second
    /// is rotated half a turn so the two face each other. Every unit in the
    /// content set is registered so summons can find it.
    pub fn setup_combat(&self, seed: u64, teams: &[TeamSetup]) -> Result<Combat, ContentError> {
        self.setup_combat_with(seed, teams, &[])
    }

    /// [`setup_combat`](Self::setup_combat) on a board that starts with
    /// [tiles](crate::tile). They are laid once the units stand, so a
    /// blocking tile is left out where a unit already is.
    pub fn setup_combat_with(&self, seed: u64, teams: &[TeamSetup], tiles: &[TilePlacement]) -> Result<Combat, ContentError> {
        let board = self.board();
        let mut combat = Combat::with_config(board.clone(), seed, self.rules.combat.clone());
        for (name, hook) in &self.hooks.effects {
            let hook = hook.clone();
            combat.register_custom_effect(name.clone(), move |ctx| hook(ctx));
        }
        for (name, predicate) in &self.hooks.conditions {
            let predicate = predicate.clone();
            combat.register_custom_condition(name.clone(), move |ctx| predicate(ctx));
        }
        for (name, hook) in &self.hooks.selectors {
            let hook = hook.clone();
            combat.register_custom_selector(name.clone(), move |ctx| hook(ctx));
        }
        for id in self.units.keys() {
            combat.register_template(id.clone(), self.base_template(id, 1)?);
        }
        for (i, team) in teams.iter().enumerate() {
            for p in self.build_team(team)? {
                let pos = if i % 2 == 1 { board.mirror(p.pos) } else { p.pos };
                combat
                    .spawn(p.template.clone(), Team(i as u8), pos)
                    .map_err(|e| ContentError::Invalid(format!("cannot place `{}`: {e}", p.template.name)))?;
            }
        }
        for placement in tiles {
            if self.tile(&placement.tile).is_none() {
                let known: Vec<String> = self.tiles().iter().map(|t| t.id.clone()).collect();
                return invalid(format!(
                    "the fight starts with unknown tile `{}`{}",
                    placement.tile,
                    suggestion(&placement.tile, known.iter())
                ));
            }
            let pos = Hex::from_offset(placement.pos.0, placement.pos.1);
            if !board.contains(pos) {
                return invalid(format!("tile `{}` at {:?} is off the board", placement.tile, placement.pos));
            }
            combat.place_tile(&placement.tile, pos, None, None);
            if placement.mirrored && board.mirror(pos) != pos {
                combat.place_tile(&placement.tile, board.mirror(pos), None, None);
            }
        }
        Ok(combat)
    }
}
