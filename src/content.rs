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

use crate::combat::{Combat, ConditionContext, CustomConditionFn, CustomEffectFn, EffectContext, Team};
use crate::effect::{Ability, Condition, Effect, EffectSpec, Passive, StatusKind, UnitTemplate};
use crate::hex::{Hex, HexBoard};
use crate::stats::{Stat, StatBlock, StatMod};

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
    #[serde(default)]
    pub ability: Option<Ability>,
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
            ability: None,
            passives: Vec::new(),
            in_shop: true,
            model: None,
        }
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

    pub fn ability(mut self, ability: Ability) -> Self {
        self.ability = Some(ability);
        self
    }

    pub fn passive(mut self, passive: Passive) -> Self {
        self.passives.push(passive);
        self
    }

    /// Keeps the unit out of shops, e.g. for units that are only summoned.
    pub fn summon_only(mut self) -> Self {
        self.in_shop = false;
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breakpoint {
    pub count: u32,
    pub bonuses: Vec<Bonus>,
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
        self.breakpoints.push(Breakpoint { count, bonuses: bonuses.into_iter().collect() });
        self
    }
}

/// An item definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDef {
    pub id: String,
    pub name: String,
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
}

impl ItemDef {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        ItemDef {
            id: id.into(),
            name: name.into(),
            mods: Vec::new(),
            passives: Vec::new(),
            grants_traits: Vec::new(),
            recipe: None,
        }
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
}

/// What a choice round offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ChoiceOffer {
    #[default]
    Augments,
    Items,
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
        Schedule {
            rounds: vec![RoundDef { name: "Fight".into(), stage: 1, kind: RoundKindDef::Pvp }],
            repeat_from: 0,
        }
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
        AugmentDef { id: id.into(), name: name.into(), description: String::new(), bonuses: Vec::new(), gold: 0 }
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
}

impl RoundKindDef {
    pub fn pvp() -> Self {
        RoundKindDef::Pvp
    }

    /// Everyone fights this neutral board. Add rewards with
    /// [`gold`](Self::gold) and [`items`](Self::items).
    pub fn pve(enemies: impl IntoIterator<Item = BoardUnit>) -> Self {
        RoundKindDef::Pve { enemies: enemies.into_iter().collect(), gold: 0, items: 0 }
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
        RoundDef { name: name.into(), stage, kind }
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
}

impl BoardUnit {
    /// A one-star unit with no items at `(column, row)`.
    pub fn new(unit: impl Into<String>, pos: (i32, i32)) -> Self {
        BoardUnit { unit: unit.into(), star: 1, items: Vec::new(), pos }
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
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hooks")
            .field("effects", &self.effects.keys().collect::<Vec<_>>())
            .field("conditions", &self.conditions.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Hooks are closures, so two sets are equal when they register the same names.
impl PartialEq for Hooks {
    fn eq(&self, other: &Self) -> bool {
        self.effects.keys().eq(other.effects.keys()) && self.conditions.keys().eq(other.conditions.keys())
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

    /// Stat modifiers for each star level, starting at one star.
    pub fn star_levels<L: IntoIterator<Item = StatMod>>(mut self, levels: impl IntoIterator<Item = L>) -> Self {
        self.rules = self.rules.star_levels(levels);
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
}

impl ContentSet {
    /// Starts defining a game in Rust.
    pub fn builder() -> GameBuilder {
        GameBuilder::new()
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
        if r.economy.interest_per <= 0 {
            return invalid("interest_per must be positive".into());
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
                    if let Condition::Custom(name) = c
                        && !self.hooks.conditions.contains_key(name)
                        && result.is_ok()
                    {
                        result = invalid(format!("{owner} uses unregistered custom condition `{name}`"));
                    }
                });
            }
            result
        };
        let check_effects = |owner: &str, effects: &[EffectSpec]| {
            for spec in effects {
                check_condition(owner, spec.condition.as_ref())?;
                match &spec.effect {
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
                    _ => {}
                }
            }
            Ok(())
        };
        let check_passives = |owner: &str, passives: &[Passive]| {
            passives.iter().try_for_each(|p| {
                check_condition(owner, p.condition.as_ref())?;
                check_effects(owner, &p.effects)
            })
        };

        for u in self.units.values() {
            let owner = format!("unit `{}`", u.id);
            for t in &u.traits {
                check_trait(&owner, t)?;
            }
            if let Some(a) = &u.ability {
                check_effects(&owner, &a.effects)?;
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
                }
            }
        }
        for a in self.augments.values() {
            let owner = format!("augment `{}`", a.id);
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
                let tier = breakpoints.iter().rposition(|b| b.count <= count);
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
            ability: def.ability.clone(),
            passives: def.passives.clone(),
            tags: def.traits.clone(),
        })
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
            let mut mods: Vec<StatMod> = Vec::new();
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
        let board = self.board();
        let mut combat = Combat::new(board.clone(), seed);
        for (name, hook) in &self.hooks.effects {
            let hook = hook.clone();
            combat.register_custom_effect(name.clone(), move |ctx| hook(ctx));
        }
        for (name, predicate) in &self.hooks.conditions {
            let predicate = predicate.clone();
            combat.register_custom_condition(name.clone(), move |ctx| predicate(ctx));
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
        Ok(combat)
    }
}
