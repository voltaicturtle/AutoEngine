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

use crate::name::Name;
use crate::stats::{Stat, StatBlock, StatMod};

/// The type of some damage, which decides how it is mitigated. The rules
/// for each kind are a [`DamageType`](crate::damage::DamageType) in the
/// fight's settings, and a game can add kinds of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DamageKind {
    /// Reduced by armor.
    Physical,
    /// Reduced by magic resist.
    Magic,
    /// Ignores resistances.
    True,
    /// A kind the game defines; see [`DamageKind::custom`].
    Custom(Name),
}

impl DamageKind {
    /// A damage type of the game's own, such as `DamageKind::custom("fire")`.
    /// Declare it once as a constant, and describe it to the game with
    /// [`GameBuilder::damage_type`](crate::content::GameBuilder::damage_type).
    pub const fn custom(id: &'static str) -> Self {
        DamageKind::Custom(Name::new(id))
    }

    /// A short lower-case id: `"physical"`, `"magic"`, `"true"`, or the
    /// name a custom kind was given.
    pub const fn id(self) -> &'static str {
        match self {
            DamageKind::Physical => "physical",
            DamageKind::Magic => "magic",
            DamageKind::True => "true",
            DamageKind::Custom(name) => name.as_str(),
        }
    }
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
    /// How much of a resource the unit has right now, by resource id.
    Resource(String),
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

    /// The value of one of the source's stats.
    pub fn stat(stat: Stat) -> Self {
        Amount::default().plus(Who::Source, ScaleBy::Stat(stat), 1000)
    }

    /// Adds a scaling term, e.g. 150% of the source's attack damage.
    pub fn plus(mut self, who: Who, by: ScaleBy, permille: i64) -> Self {
        self.scalings.push(Scaling { who, by, permille });
        self
    }

    /// Whether it is always nothing.
    pub fn is_zero(&self) -> bool {
        self.flat == 0 && self.scalings.is_empty()
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
    /// Cannot start casting. Resources still build up.
    Silence,
    /// Cannot make basic attacks, but can move and cast.
    Disarm,
    /// Cannot move, but can attack what is in range and cast.
    Root,
    /// Must attack the unit that applied it while that unit lives.
    Taunt,
    /// Healing received is cut by this many thousandths. The strongest
    /// wound on a unit applies.
    Wound { permille: u32 },
    /// Takes no damage.
    Invulnerable,
    /// New debuffs do not take hold (see [`StatusKind::is_debuff`]).
    Unstoppable,
    /// Does nothing by itself: a named mark for conditions to look for
    /// ([`Condition::HasStatus`]), such as "burning" or "marked". A
    /// `harmful` mark counts as a debuff.
    Marker { name: String, harmful: bool },
}

impl StatusKind {
    /// Whether the status hurts the unit it is on, so that
    /// [`Effect::Cleanse`] removes it. Stat changes count when any of them
    /// lowers a stat.
    pub fn is_debuff(&self) -> bool {
        match self {
            StatusKind::Stun
            | StatusKind::Silence
            | StatusKind::Disarm
            | StatusKind::Root
            | StatusKind::Taunt
            | StatusKind::Wound { .. }
            | StatusKind::DamageOverTime { .. } => true,
            StatusKind::StatMods(mods) => mods.iter().any(StatMod::lowers),
            StatusKind::Invulnerable | StatusKind::Unstoppable => false,
            StatusKind::Marker { harmful, .. } => *harmful,
        }
    }

    /// A short name for logs, tooltips and [`Condition::HasStatus`]. A
    /// marker's name is the one it was given.
    pub fn name(&self) -> &str {
        match self {
            StatusKind::Marker { name, .. } => name,
            StatusKind::Stun => "stunned",
            StatusKind::StatMods(_) => "stat-modified",
            StatusKind::DamageOverTime { .. } => "afflicted",
            StatusKind::Silence => "silenced",
            StatusKind::Disarm => "disarmed",
            StatusKind::Root => "rooted",
            StatusKind::Taunt => "taunted",
            StatusKind::Wound { .. } => "wounded",
            StatusKind::Invulnerable => "invulnerable",
            StatusKind::Unstoppable => "unstoppable",
        }
    }
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
    /// Gives the unit some of a resource (see the
    /// [`resource`](crate::resource) module), or takes it away when the
    /// amount is negative. Does nothing to a unit without that resource.
    GainResource { resource: String, amount: Amount },
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
    /// Removes every debuff from the unit (see [`StatusKind::is_debuff`]).
    Cleanse,
    /// Kills the unit outright if its health is at or below this many
    /// thousandths of its maximum. Invulnerable units are spared.
    Execute { permille: u32 },
    /// Drags the unit up to `hexes` steps toward the source, stopping next
    /// to it or at another unit.
    Pull { hexes: u32 },
    /// Pushes the unit up to `hexes` steps directly away from the source.
    Knockback { hexes: u32 },
    /// The source jumps to the free hex closest to the selected unit, and
    /// targets it if it is an enemy.
    Leap,
    /// Fires the selected unit's [`Trigger::Signal`] passives of this name,
    /// with the source as the other unit. This is how content makes
    /// occasions of its own for passives to react to.
    Signal { name: Name },
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

    /// Gives mana, or drains it when negative.
    pub fn mana(amount: impl Into<Amount>) -> Self {
        Effect::resource(crate::resource::MANA, amount)
    }

    /// Gives some of a resource, or drains it when negative.
    pub fn resource(resource: impl Into<String>, amount: impl Into<Amount>) -> Self {
        Effect::GainResource { resource: resource.into(), amount: amount.into() }
    }

    /// The amounts the effect works out when it resolves.
    pub fn amounts(&self) -> Vec<&Amount> {
        match self {
            Effect::Damage { amount, .. }
            | Effect::Heal { amount }
            | Effect::Shield { amount, .. }
            | Effect::GainResource { amount, .. } => vec![amount],
            Effect::ApplyStatus { status: StatusKind::DamageOverTime { per_second, .. }, .. } => vec![per_second],
            _ => Vec::new(),
        }
    }

    pub fn stun(duration_ms: u32) -> Self {
        Effect::ApplyStatus { status: StatusKind::Stun, duration_ms }
    }

    pub fn silence(duration_ms: u32) -> Self {
        Effect::ApplyStatus { status: StatusKind::Silence, duration_ms }
    }

    pub fn disarm(duration_ms: u32) -> Self {
        Effect::ApplyStatus { status: StatusKind::Disarm, duration_ms }
    }

    pub fn root(duration_ms: u32) -> Self {
        Effect::ApplyStatus { status: StatusKind::Root, duration_ms }
    }

    /// Forces the selected enemies to attack the source.
    pub fn taunt(duration_ms: u32) -> Self {
        Effect::ApplyStatus { status: StatusKind::Taunt, duration_ms }
    }

    /// Cuts healing received by `permille` thousandths.
    pub fn wound(permille: u32, duration_ms: u32) -> Self {
        Effect::ApplyStatus { status: StatusKind::Wound { permille }, duration_ms }
    }

    pub fn invulnerable(duration_ms: u32) -> Self {
        Effect::ApplyStatus { status: StatusKind::Invulnerable, duration_ms }
    }

    pub fn cleanse() -> Self {
        Effect::Cleanse
    }

    pub fn unstoppable(duration_ms: u32) -> Self {
        Effect::ApplyStatus { status: StatusKind::Unstoppable, duration_ms }
    }

    /// Puts a named mark on the unit for conditions to look for. It does
    /// nothing by itself and is not a debuff.
    pub fn mark(name: impl Into<String>, duration_ms: u32) -> Self {
        Effect::ApplyStatus { status: StatusKind::Marker { name: name.into(), harmful: false }, duration_ms }
    }

    /// A mark that counts as a debuff: cleansing removes it and an
    /// unstoppable unit shrugs it off.
    pub fn harmful_mark(name: impl Into<String>, duration_ms: u32) -> Self {
        Effect::ApplyStatus { status: StatusKind::Marker { name: name.into(), harmful: true }, duration_ms }
    }

    /// Fires the selected units' passives that wait for this signal.
    pub fn signal(name: &'static str) -> Self {
        Effect::Signal { name: Name::new(name) }
    }

    pub fn execute(permille: u32) -> Self {
        Effect::Execute { permille }
    }

    pub fn pull(hexes: u32) -> Self {
        Effect::Pull { hexes }
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
    /// Up to `n` enemies picked at random (from the fight's seeded dice).
    RandomEnemies(u32),
    /// The `n` enemies with the most of a stat.
    HighestStatEnemies { stat: Stat, n: u32 },
    /// The `n` allies (the source included) with the most of a stat.
    HighestStatAllies { stat: Stat, n: u32 },
    /// The `n` enemies with the least of a stat.
    LowestStatEnemies { stat: Stat, n: u32 },
    /// The `n` allies (the source included) with the least of a stat.
    LowestStatAllies { stat: Stat, n: u32 },
    /// Up to `n` allies picked at random, the source included.
    RandomAllies(u32),
    /// Every ally except the source.
    OtherAllies,
    /// Units chosen by a Rust hook registered under this name (see
    /// [`GameBuilder::custom_selector`](crate::content::GameBuilder::custom_selector)).
    Custom(String),
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
    /// The unit has at least one debuff (see [`StatusKind::is_debuff`]).
    HasDebuff(Who),
    HasTag { who: Who, tag: String },
    /// The unit has at least `count` of a named stack.
    StacksAtLeast { who: Who, stack: String, count: u32 },
    /// At least `count` living units on the source's team (itself included)
    /// carry a tag.
    AlliesWithTagAtLeast { tag: String, count: u32 },
    /// The unit has a resource filled to at least this share of its
    /// capacity, in permille. False for a unit without the resource.
    ResourceAtLeast { who: Who, resource: String, permille: u32 },
    /// The unit has a status with this [name](StatusKind::name), such as
    /// `"stunned"` or the name of a [marker](StatusKind::Marker).
    HasStatus { who: Who, status: String },
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

/// What a unit pays to cast an ability. The unit casts as soon as it can
/// pay (and the ability is off cooldown).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cost {
    /// A full bar of a resource, which the cast empties. A unit with none
    /// of the resource never casts.
    Full(String),
    /// A fixed amount of a resource.
    Amount { resource: String, amount: i64 },
    /// Nothing. Give the ability a cooldown, or the unit does little else.
    Free,
}

impl Cost {
    /// A full bar of a resource.
    pub fn full(resource: impl Into<String>) -> Self {
        Cost::Full(resource.into())
    }

    /// A fixed amount of a resource.
    pub fn of(resource: impl Into<String>, amount: i64) -> Self {
        Cost::Amount { resource: resource.into(), amount }
    }

    /// The resource it is paid in, if any.
    pub fn resource(&self) -> Option<&str> {
        match self {
            Cost::Full(resource) | Cost::Amount { resource, .. } => Some(resource),
            Cost::Free => None,
        }
    }
}

/// A full bar of mana.
impl Default for Cost {
    fn default() -> Self {
        Cost::full(crate::resource::MANA)
    }
}

/// An active ability, cast automatically as soon as the unit can pay its
/// [`Cost`]: by default, when its mana is full.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ability {
    pub name: String,
    /// What it does, for players to read.
    #[serde(default)]
    pub description: String,
    /// Time the unit spends casting before the effects resolve.
    #[serde(default)]
    pub cast_time_ms: u32,
    /// What the unit pays to cast it.
    #[serde(default)]
    pub cost: Cost,
    /// The least time between two casts, counted from when a cast starts.
    #[serde(default)]
    pub cooldown_ms: u32,
    pub effects: Vec<EffectSpec>,
}

impl Ability {
    pub fn new(name: impl Into<String>) -> Self {
        Ability {
            name: name.into(),
            description: String::new(),
            cast_time_ms: 0,
            cost: Cost::default(),
            cooldown_ms: 0,
            effects: Vec::new(),
        }
    }

    /// What the unit pays to cast it: `.cost(Cost::of("energy", 40))`.
    pub fn cost(mut self, cost: Cost) -> Self {
        self.cost = cost;
        self
    }

    /// The least time between two casts.
    pub fn cooldown(mut self, ms: u32) -> Self {
        self.cooldown_ms = ms;
        self
    }

    /// What it does, for players to read.
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = text.into();
        self
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
    /// The owner dodged a basic attack; the other unit is the attacker.
    OnDodge,
    /// The owner was healed by an ability or an effect of one (not by
    /// lifesteal or a passive); the other unit is the healer.
    OnHealed,
    /// An ally finished casting; the other unit is the caster.
    OnAllyCast,
    /// An enemy died; the other unit is the one that died.
    OnEnemyDeath,
    /// An [`Effect::Signal`] of this name reached the owner; the other unit
    /// is the one that sent it.
    Signal(Name),
    CombatStart,
    /// The owner lands a basic attack.
    OnAttack,
    /// The owner takes damage from an attack or ability.
    OnDamaged,
    /// The owner finishes casting an ability.
    OnCast,
    OnKill,
    OnDeath,
    OnAllyDeath,
    /// The unit landed a critical hit; the other unit is the one hit.
    OnCrit,
    /// One of the unit's shields was used up by damage; the other unit is
    /// the one that broke it.
    OnShieldBroken,
    /// Repeats on a fixed interval while the owner is alive.
    Every { interval_ms: u32 },
    /// The owner's health drops to or below this share of its maximum.
    HealthBelow { permille: u32 },
}

impl Trigger {
    /// Fires when an [`Effect::signal`] of this name reaches the owner.
    pub const fn signal(name: &'static str) -> Self {
        Trigger::Signal(Name::new(name))
    }
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
    /// The abilities it casts. Each tick it casts the first one it can pay
    /// for, so put the ones that should win first.
    pub abilities: Vec<Ability>,
    pub passives: Vec<Passive>,
    /// Free-form labels (traits, roles) that selectors can match on.
    pub tags: Vec<String>,
    /// Which enemy the unit goes after when it needs a new target.
    pub targeting: Targeting,
    /// The type of damage its basic attacks deal.
    pub attack_kind: DamageKind,
}

/// How a unit picks its target when it has none (or its target died).
/// Taunts still win.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Targeting {
    #[default]
    Nearest,
    /// The enemy farthest away: assassins that dive the back line.
    Farthest,
    /// The enemy with the least health left.
    LowestHealth,
    /// The enemy with the most attack damage.
    HighestDamage,
    /// The enemy with the least armor.
    LowestArmor,
    /// The enemy with the most of any stat.
    HighestStat(Stat),
    /// The enemy with the least of any stat.
    LowestStat(Stat),
}

impl UnitTemplate {
    pub fn new(name: impl Into<String>, stats: StatBlock) -> Self {
        UnitTemplate {
            id: String::new(),
            name: name.into(),
            stats,
            abilities: Vec::new(),
            passives: Vec::new(),
            tags: Vec::new(),
            targeting: Targeting::Nearest,
            attack_kind: DamageKind::Physical,
        }
    }

    /// The type of damage its basic attacks deal, instead of physical.
    pub fn with_attack_kind(mut self, kind: DamageKind) -> Self {
        self.attack_kind = kind;
        self
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Adds an ability. A unit can have several; see
    /// [`UnitTemplate::abilities`].
    pub fn with_ability(mut self, ability: Ability) -> Self {
        self.abilities.push(ability);
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

    pub fn with_targeting(mut self, targeting: Targeting) -> Self {
        self.targeting = targeting;
        self
    }

    /// Sets one base stat.
    pub fn with_stat(mut self, stat: crate::stats::Stat, value: i64) -> Self {
        self.stats = self.stats.with(stat, value);
        self
    }
}
