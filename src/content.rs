//! Game content defined in data files.
//!
//! A [`ContentSet`] holds everything a game built on the engine ships as
//! data: units, traits (synergies), items and a few global rules. It is
//! loaded from [RON](https://github.com/ron-rs/ron) files and validated up
//! front, so mistakes such as a unit naming a trait that does not exist are
//! reported when content loads, not in the middle of a match.
//!
//! The content layer sits on top of combat without combat knowing about it:
//! [`ContentSet::build_team`] turns "these units, at these star levels, with
//! these items, on these hexes" into plain [`UnitTemplate`]s with every trait
//! and item bonus already folded in.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::combat::{Combat, Team};
use crate::effect::{Ability, Effect, EffectSpec, Passive, StatusKind, UnitTemplate};
use crate::hex::{Hex, HexBoard};
use crate::stats::{Stat, StatBlock, StatMod};

fn default_true() -> bool {
    true
}

/// A unit as authored in `units.ron`.
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
    pub passives: Vec<Passive>,
}

/// One tier of a trait: active when at least `count` distinct members are on
/// the board. Only the highest tier reached applies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breakpoint {
    pub count: u32,
    pub bonuses: Vec<Bonus>,
}

/// A trait (synergy) as authored in `traits.ron`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub breakpoints: Vec<Breakpoint>,
}

/// An item as authored in `items.ron`.
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

/// Global rules, authored in `rules.ron`. Every field has a default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameRules {
    pub board_cols: u32,
    pub board_rows: u32,
    pub max_items_per_unit: usize,
    /// Stat modifiers for each star level; the first entry is one star.
    pub star_levels: Vec<Vec<StatMod>>,
}

impl Default for GameRules {
    fn default() -> Self {
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
        }
    }
}

fn default_star() -> u8 {
    1
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

/// One side of a fight.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TeamSetup {
    pub units: Vec<BoardUnit>,
    /// Extra team-wide bonuses that do not come from traits or items, such
    /// as augment-style modifiers a player has picked up.
    #[serde(default)]
    pub bonuses: Vec<Bonus>,
}

/// A whole fight described in data: the teams and where they stand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchSetup {
    pub teams: Vec<TeamSetup>,
}

/// The state of one trait for a given board.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    Io { path: String, message: String },
    Parse { file: String, message: String },
    Invalid(String),
}

impl std::fmt::Display for ContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentError::Io { path, message } => write!(f, "cannot read {path}: {message}"),
            ContentError::Parse { file, message } => write!(f, "{file}: {message}"),
            ContentError::Invalid(message) => write!(f, "invalid content: {message}"),
        }
    }
}

impl std::error::Error for ContentError {}

fn invalid<T>(message: String) -> Result<T, ContentError> {
    Err(ContentError::Invalid(message))
}

/// Parses a RON document. Optional fields may be written without `Some(..)`.
pub fn parse_ron<T: DeserializeOwned>(file: &str, text: &str) -> Result<T, ContentError> {
    ron::Options::default()
        .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
        .from_str(text)
        .map_err(|e| ContentError::Parse { file: file.to_string(), message: e.to_string() })
}

fn read(path: &Path) -> Result<String, ContentError> {
    std::fs::read_to_string(path)
        .map_err(|e| ContentError::Io { path: path.display().to_string(), message: e.to_string() })
}

/// All the content of a game, validated and indexed by id.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ContentSet {
    units: BTreeMap<String, UnitDef>,
    traits: BTreeMap<String, TraitDef>,
    items: BTreeMap<String, ItemDef>,
    rules: GameRules,
}

impl ContentSet {
    /// Builds and validates a content set from already-parsed definitions.
    pub fn new(
        units: Vec<UnitDef>,
        traits: Vec<TraitDef>,
        items: Vec<ItemDef>,
        rules: GameRules,
    ) -> Result<Self, ContentError> {
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
        let set = ContentSet {
            units: index("unit", units, |u| &u.id)?,
            traits: index("trait", traits, |t| &t.id)?,
            items: index("item", items, |i| &i.id)?,
            rules,
        };
        set.validate()?;
        Ok(set)
    }

    /// Parses content from RON text. `items` and `rules` may be `None`.
    pub fn from_ron(
        units: &str,
        traits: &str,
        items: Option<&str>,
        rules: Option<&str>,
    ) -> Result<Self, ContentError> {
        ContentSet::new(
            parse_ron("units.ron", units)?,
            parse_ron("traits.ron", traits)?,
            items.map(|t| parse_ron("items.ron", t)).transpose()?.unwrap_or_default(),
            rules.map(|t| parse_ron("rules.ron", t)).transpose()?.unwrap_or_default(),
        )
    }

    /// Loads `units.ron` and `traits.ron`, plus `items.ron` and `rules.ron`
    /// if present, from a directory.
    pub fn load_dir(dir: impl AsRef<Path>) -> Result<Self, ContentError> {
        let dir = dir.as_ref();
        let optional = |name: &str| -> Result<Option<String>, ContentError> {
            let path = dir.join(name);
            if path.exists() { read(&path).map(Some) } else { Ok(None) }
        };
        let units = read(&dir.join("units.ron"))?;
        let traits = read(&dir.join("traits.ron"))?;
        let items = optional("items.ron")?;
        let rules = optional("rules.ron")?;
        ContentSet::from_ron(&units, &traits, items.as_deref(), rules.as_deref())
    }

    fn validate(&self) -> Result<(), ContentError> {
        let r = &self.rules;
        if r.board_cols == 0 || r.board_rows == 0 {
            return invalid("the board must have at least one column and one row".into());
        }
        if r.star_levels.is_empty() {
            return invalid("rules need at least one star level".into());
        }
        let check_trait = |owner: &str, id: &str| {
            if self.traits.contains_key(id) {
                Ok(())
            } else {
                invalid(format!("{owner} refers to unknown trait `{id}`"))
            }
        };
        let check_effects = |owner: &str, effects: &[EffectSpec]| {
            for spec in effects {
                if let Effect::Summon { unit, .. } = &spec.effect
                    && !self.units.contains_key(unit)
                {
                    return invalid(format!("{owner} summons unknown unit `{unit}`"));
                }
                if let Effect::ApplyStatus { status: StatusKind::StatMods(mods), .. } = &spec.effect
                    && mods.is_empty()
                {
                    return invalid(format!("{owner} applies a stat status with no modifiers"));
                }
            }
            Ok(())
        };
        let check_passives = |owner: &str, passives: &[Passive]| {
            passives.iter().try_for_each(|p| check_effects(owner, &p.effects))
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
                        return invalid(format!("{owner} is built from unknown item `{part}`"));
                    }
                }
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
        self.units.get(id).ok_or_else(|| ContentError::Invalid(format!("unknown unit `{id}`")))
    }

    fn item_def(&self, id: &str) -> Result<&ItemDef, ContentError> {
        self.items.get(id).ok_or_else(|| ContentError::Invalid(format!("unknown item `{id}`")))
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
