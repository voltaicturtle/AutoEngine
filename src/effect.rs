//! The data model for everything a unit can *do* beyond basic attacks.
//!
//! Abilities, trait bonuses, items and augment-style modifiers are all
//! expressed with the same three building blocks:
//!
//! * an [`Effect`] says what happens (damage, heal, stun, buff, summon...),
//! * a [`Selector`] says who it happens to,
//! * a [`Trigger`] says when a [`Passive`] fires,
//! * an optional [`Condition`] gates a passive or a single effect.
//!
//! Units can also carry named *stacks* (counters that may grant stats per
//! stack), and games can register Rust hooks for anything the data model
//! does not cover: see [`Effect::Custom`] and [`Condition::Custom`].
//!
//! These are plain data with no behaviour of their own; the combat
//! simulation interprets them. Each type has small constructors so content
//! reads naturally in code:
//!
//! ```
//! use autoengine::effect::{Ability, Effect, Selector};
//! use autoengine::stats::Stat;
//!
//! let ability = Ability::new("Searing Flurry")
//!     .cast_time(250)
//!     .effect(Effect::physical(80).with_crit().to(Selector::CurrentTarget))
//!     .effect(Effect::stat_status([Stat::AttackSpeed.percent(400)], 3000).to(Selector::Source));
//! assert_eq!(ability.effects.len(), 2);
//! ```

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

/// Which unit a scaling value or condition is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Who {
    /// The unit producing the effect.
    Source,
    /// The unit receiving the effect. In a passive's own condition, this is
    /// the other unit in the triggering event (see [`Selector::EventOther`]).
    Target,
}

/// A value read from a unit at the moment an effect resolves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScaleBy {
    Stat(Stat),
    CurrentHealth,
    MissingHealth,
    /// The unit's current count of a named stack.
    Stacks(String),
    /// Living units on the unit's team (itself included) carrying a tag.
    AlliesWithTag(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

impl From<i64> for Amount {
    fn from(value: i64) -> Self {
        Amount::flat(value)
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
    /// Adds `count` to a named stack, up to `max`. Each stack actually gained
    /// grants `mods_per_stack` for the rest of the combat (or until cleared).
    AddStacks {
        stack: String,
        count: u32,
        #[serde(default)]
        max: Option<u32>,
        #[serde(default)]
        mods_per_stack: Vec<StatMod>,
    },
    /// Resets a named stack to zero and removes the stats it granted.
    ClearStacks { stack: String },
    /// Pushes the unit up to `hexes` steps directly away from the source.
    Knockback { hexes: u32 },
    /// The source jumps to the free hex closest to the selected unit, and
    /// targets it if it is an enemy.
    Leap,
    /// Runs a Rust hook registered under `name` (see
    /// [`GameBuilder::custom_effect`](crate::content::GameBuilder::custom_effect)),
    /// once per selected unit. `args` are passed through to the hook so one
    /// hook can serve several pieces of content.
    Custom {
        name: String,
        #[serde(default)]
        args: Vec<i64>,
    },
}

impl Effect {
    /// Damage of the given kind that cannot crit; see [`Effect::with_crit`].
    pub fn damage(amount: impl Into<Amount>, kind: DamageKind) -> Self {
        Effect::Damage { amount: amount.into(), kind, can_crit: false }
    }

    pub fn physical(amount: impl Into<Amount>) -> Self {
        Effect::damage(amount, DamageKind::Physical)
    }

    pub fn magic(amount: impl Into<Amount>) -> Self {
        Effect::damage(amount, DamageKind::Magic)
    }

    pub fn true_damage(amount: impl Into<Amount>) -> Self {
        Effect::damage(amount, DamageKind::True)
    }

    pub fn heal(amount: impl Into<Amount>) -> Self {
        Effect::Heal { amount: amount.into() }
    }

    /// A shield that lasts the whole combat.
    pub fn shield(amount: impl Into<Amount>) -> Self {
        Effect::Shield { amount: amount.into(), duration_ms: None }
    }

    pub fn shield_for(amount: impl Into<Amount>, duration_ms: u32) -> Self {
        Effect::Shield { amount: amount.into(), duration_ms: Some(duration_ms) }
    }

    pub fn mana(amount: i64) -> Self {
        Effect::GainMana { amount }
    }

    pub fn stun(duration_ms: u32) -> Self {
        Effect::ApplyStatus { status: StatusKind::Stun, duration_ms }
    }

    /// Temporary stat changes (positive or negative), reverted afterwards.
    pub fn stat_status(mods: impl IntoIterator<Item = StatMod>, duration_ms: u32) -> Self {
        Effect::ApplyStatus { status: StatusKind::StatMods(mods.into_iter().collect()), duration_ms }
    }

    /// Damage over time, given per second.
    pub fn damage_over_time(per_second: impl Into<Amount>, kind: DamageKind, duration_ms: u32) -> Self {
        Effect::ApplyStatus { status: StatusKind::DamageOverTime { per_second: per_second.into(), kind }, duration_ms }
    }

    /// Stat changes for the rest of the combat.
    pub fn modify_stats(mods: impl IntoIterator<Item = StatMod>) -> Self {
        Effect::ModifyStats { mods: mods.into_iter().collect() }
    }

    pub fn summon(unit: impl Into<String>, count: u32) -> Self {
        Effect::Summon { unit: unit.into(), count }
    }

    /// Adds stacks with no cap and no stats; see [`Effect::capped_at`] and
    /// [`Effect::per_stack`].
    pub fn add_stacks(stack: impl Into<String>, count: u32) -> Self {
        Effect::AddStacks { stack: stack.into(), count, max: None, mods_per_stack: Vec::new() }
    }

    pub fn clear_stacks(stack: impl Into<String>) -> Self {
        Effect::ClearStacks { stack: stack.into() }
    }

    pub fn knockback(hexes: u32) -> Self {
        Effect::Knockback { hexes }
    }

    pub fn leap() -> Self {
        Effect::Leap
    }

    /// Calls a registered Rust hook with no arguments.
    pub fn custom(name: impl Into<String>) -> Self {
        Effect::Custom { name: name.into(), args: Vec::new() }
    }

    /// Calls a registered Rust hook with arguments.
    pub fn custom_with(name: impl Into<String>, args: impl IntoIterator<Item = i64>) -> Self {
        Effect::Custom { name: name.into(), args: args.into_iter().collect() }
    }

    /// Caps an [`Effect::AddStacks`]. Other effects are returned unchanged.
    pub fn capped_at(mut self, cap: u32) -> Self {
        if let Effect::AddStacks { max, .. } = &mut self {
            *max = Some(cap);
        }
        self
    }

    /// Stats granted by each stack an [`Effect::AddStacks`] adds. Other
    /// effects are returned unchanged.
    pub fn per_stack(mut self, mods: impl IntoIterator<Item = StatMod>) -> Self {
        if let Effect::AddStacks { mods_per_stack, .. } = &mut self {
            mods_per_stack.extend(mods);
        }
        self
    }

    /// Lets a damage effect crit. Other effects are returned unchanged.
    pub fn with_crit(mut self) -> Self {
        if let Effect::Damage { can_crit, .. } = &mut self {
            *can_crit = true;
        }
        self
    }

    /// Pairs the effect with the units it applies to.
    pub fn to(self, target: Selector) -> EffectSpec {
        EffectSpec::new(target, self)
    }
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

/// A yes/no question asked when a passive fires or an effect resolves.
///
/// On an [`EffectSpec`] it is checked once per selected unit, with that unit
/// as [`Who::Target`]. On a [`Passive`] it is checked once when the trigger
/// fires, with the other unit in the event as the target; conditions about a
/// target that does not exist are false. A passive whose condition fails
/// does not use up a proc.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Condition {
    /// Health at or below this share of maximum, in permille.
    HealthBelow { who: Who, permille: u32 },
    /// Health above this share of maximum, in permille.
    HealthAbove { who: Who, permille: u32 },
    Stunned(Who),
    Shielded(Who),
    HasTag { who: Who, tag: String },
    /// The unit has at least `count` of a named stack.
    StacksAtLeast { who: Who, stack: String, count: u32 },
    /// At least `count` living units on the source's team (itself included)
    /// carry a tag.
    AlliesWithTagAtLeast { tag: String, count: u32 },
    /// A seeded random roll, in permille.
    Chance { permille: u32 },
    Not(Box<Condition>),
    All(Vec<Condition>),
    Any(Vec<Condition>),
    /// A Rust predicate registered under this name (see
    /// [`GameBuilder::custom_condition`](crate::content::GameBuilder::custom_condition)).
    Custom(String),
}

impl Condition {
    pub fn chance(permille: u32) -> Self {
        Condition::Chance { permille }
    }

    pub fn custom(name: impl Into<String>) -> Self {
        Condition::Custom(name.into())
    }

    #[allow(clippy::should_implement_trait)]
    pub fn not(self) -> Self {
        Condition::Not(Box::new(self))
    }

    /// Both conditions must hold.
    pub fn and(self, other: Condition) -> Self {
        match self {
            Condition::All(mut all) => {
                all.push(other);
                Condition::All(all)
            }
            first => Condition::All(vec![first, other]),
        }
    }

    /// Either condition may hold.
    pub fn or(self, other: Condition) -> Self {
        match self {
            Condition::Any(mut any) => {
                any.push(other);
                Condition::Any(any)
            }
            first => Condition::Any(vec![first, other]),
        }
    }

    /// Every condition nested in this one, including itself, depth first.
    pub fn walk(&self, f: &mut impl FnMut(&Condition)) {
        f(self);
        match self {
            Condition::Not(c) => c.walk(f),
            Condition::All(cs) | Condition::Any(cs) => cs.iter().for_each(|c| c.walk(f)),
            _ => {}
        }
    }
}

/// An effect together with who receives it, and optionally a condition
/// checked for each receiver.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectSpec {
    pub target: Selector,
    pub effect: Effect,
    #[serde(default)]
    pub condition: Option<Condition>,
}

impl EffectSpec {
    pub fn new(target: Selector, effect: Effect) -> Self {
        EffectSpec { target, effect, condition: None }
    }

    /// Only applies the effect to selected units for which `condition` holds.
    /// Several calls combine with "and".
    pub fn when(mut self, condition: Condition) -> Self {
        self.condition = Some(match self.condition.take() {
            Some(existing) => existing.and(condition),
            None => condition,
        });
        self
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

impl Ability {
    pub fn new(name: impl Into<String>) -> Self {
        Ability { name: name.into(), cast_time_ms: 0, effects: Vec::new() }
    }

    pub fn cast_time(mut self, ms: u32) -> Self {
        self.cast_time_ms = ms;
        self
    }

    pub fn effect(mut self, spec: EffectSpec) -> Self {
        self.effects.push(spec);
        self
    }
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
    /// Checked each time the trigger fires; the passive does nothing (and
    /// uses no proc) when it fails.
    #[serde(default)]
    pub condition: Option<Condition>,
}

impl Passive {
    pub fn new(name: impl Into<String>, trigger: Trigger) -> Self {
        Passive { name: name.into(), trigger, effects: Vec::new(), max_procs: None, condition: None }
    }

    /// Only fires when `condition` holds. Several calls combine with "and".
    pub fn when(mut self, condition: Condition) -> Self {
        self.condition = Some(match self.condition.take() {
            Some(existing) => existing.and(condition),
            None => condition,
        });
        self
    }

    pub fn effect(mut self, spec: EffectSpec) -> Self {
        self.effects.push(spec);
        self
    }

    pub fn max_procs(mut self, n: u32) -> Self {
        self.max_procs = Some(n);
        self
    }
}

/// Everything needed to put a unit on the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitTemplate {
    /// Stable identifier of what kind of unit this is, such as the content
    /// id it was built from. Clients use it to pick visuals; the simulation
    /// never reads it. Empty for ad-hoc templates.
    pub id: String,
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
            id: String::new(),
            name: name.into(),
            stats,
            ability: None,
            passives: Vec::new(),
            tags: Vec::new(),
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
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
