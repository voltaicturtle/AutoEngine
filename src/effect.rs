//! The data model for everything a unit can *do* beyond basic attacks.
//!
//! Abilities, trait bonuses, items and augment-style modifiers are all
//! expressed with the same three building blocks:
//!
//! * an [`Effect`] says what happens (damage, heal, stun, buff, summon...),
//! * a [`Selector`] says who it happens to,
//! * a [`Trigger`] says when a [`Passive`] fires.
//!
//! These are plain data with no behaviour of their own, so content can be
//! authored in data files and the combat simulation interprets them.

use serde::{Deserialize, Serialize};

use crate::stats::{Stat, StatBlock, StatMod};

/// How damage is mitigated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DamageKind {
    /// Reduced by armor.
    Physical,
    /// Reduced by magic resist.
    Magic,
    /// Ignores resistances.
    True,
}

/// Which unit a scaling value is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Who {
    /// The unit producing the effect.
    Source,
    /// The unit receiving the effect.
    Target,
}

/// A value read from a unit at the moment an effect resolves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScaleBy {
    Stat(Stat),
    CurrentHealth,
    MissingHealth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scaling {
    pub who: Who,
    pub by: ScaleBy,
    pub permille: i64,
}

/// A computed quantity: `flat + sum(scalings)`, optionally multiplied by the
/// source's ability power.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Amount {
    pub flat: i64,
    pub scalings: Vec<Scaling>,
    /// Multiply the total by the source's `AbilityPower` percent.
    pub power_scaled: bool,
}

impl Amount {
    /// A fixed amount.
    pub fn flat(value: i64) -> Self {
        Amount { flat: value, ..Default::default() }
    }

    /// A fixed amount multiplied by the source's ability power.
    pub fn power(value: i64) -> Self {
        Amount { flat: value, power_scaled: true, ..Default::default() }
    }

    /// Adds a scaling term, e.g. 150% of the source's attack damage.
    pub fn plus(mut self, who: Who, by: ScaleBy, permille: i64) -> Self {
        self.scalings.push(Scaling { who, by, permille });
        self
    }
}

/// A lasting condition on a unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatusKind {
    /// Cannot move, attack or cast.
    Stun,
    /// Temporary stat changes, reverted when the status ends.
    StatMods(Vec<StatMod>),
    /// Damage every tick, expressed per second.
    DamageOverTime { per_second: Amount, kind: DamageKind },
}

/// What happens to each selected unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    Damage {
        amount: Amount,
        kind: DamageKind,
        #[serde(default)]
        can_crit: bool,
    },
    Heal { amount: Amount },
    /// Absorbs damage before health. `None` lasts the whole combat.
    Shield {
        amount: Amount,
        #[serde(default)]
        duration_ms: Option<u32>,
    },
    /// Positive or negative.
    GainMana { amount: i64 },
    ApplyStatus { status: StatusKind, duration_ms: u32 },
    /// Stat changes that last for the rest of the combat.
    ModifyStats { mods: Vec<StatMod> },
    /// Spawns `count` copies of a unit on the selected unit's team at the
    /// nearest free hexes to it. `unit` is looked up in the templates
    /// registered with the combat; unknown ids do nothing.
    Summon { unit: String, count: u32 },
}

/// The reference point for area selectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Anchor {
    Source,
    CurrentTarget,
}

/// Which units an effect applies to, relative to the unit producing it.
/// "Allies" includes the source itself. Results are always produced in a
/// deterministic order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Selector {
    /// The unit producing the effect.
    Source,
    /// The source's attack target (or the nearest enemy if it has none).
    CurrentTarget,
    /// The other unit involved in the triggering event: the unit attacked
    /// for `OnAttack`, the attacker for `OnDamaged`, the victim for `OnKill`,
    /// the killer for `OnDeath`, the fallen ally for `OnAllyDeath`.
    EventOther,
    NearestEnemies(u32),
    FarthestEnemies(u32),
    LowestHealthEnemies(u32),
    LowestHealthAllies(u32),
    AllEnemies,
    AllAllies,
    EnemiesWithin { anchor: Anchor, radius: u32 },
    AlliesWithin { anchor: Anchor, radius: u32 },
    /// Allies whose template carries the given tag (trait, role, ...).
    AlliesWithTag(String),
}

/// An effect together with who receives it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectSpec {
    pub target: Selector,
    pub effect: Effect,
}

impl EffectSpec {
    pub fn new(target: Selector, effect: Effect) -> Self {
        EffectSpec { target, effect }
    }
}

/// An active ability, cast automatically when the unit's mana is full.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ability {
    pub name: String,
    /// Time the unit spends casting before the effects resolve.
    #[serde(default)]
    pub cast_time_ms: u32,
    pub effects: Vec<EffectSpec>,
}

/// When a passive fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trigger {
    CombatStart,
    /// The owner lands a basic attack.
    OnAttack,
    /// The owner takes damage from an attack or ability.
    OnDamaged,
    /// The owner finishes casting its ability.
    OnCast,
    OnKill,
    OnDeath,
    OnAllyDeath,
    /// Repeats on a fixed interval while the owner is alive.
    Every { interval_ms: u32 },
    /// The owner's health drops to or below this share of its maximum.
    HealthBelow { permille: u32 },
}

/// A triggered set of effects. Traits, items and augments attach these to
/// units before combat begins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passive {
    #[serde(default)]
    pub name: String,
    pub trigger: Trigger,
    pub effects: Vec<EffectSpec>,
    /// Maximum number of times this fires per combat. `None` is unlimited.
    #[serde(default)]
    pub max_procs: Option<u32>,
}

/// Everything needed to put a unit on the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitTemplate {
    pub name: String,
    pub stats: StatBlock,
    pub ability: Option<Ability>,
    pub passives: Vec<Passive>,
    /// Free-form labels (traits, roles) that selectors can match on.
    pub tags: Vec<String>,
}

impl UnitTemplate {
    pub fn new(name: impl Into<String>, stats: StatBlock) -> Self {
        UnitTemplate {
            name: name.into(),
            stats,
            ability: None,
            passives: Vec::new(),
            tags: Vec::new(),
        }
    }

    pub fn with_ability(mut self, ability: Ability) -> Self {
        self.ability = Some(ability);
        self
    }

    pub fn with_passive(mut self, passive: Passive) -> Self {
        self.passives.push(passive);
        self
    }

    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }
}
