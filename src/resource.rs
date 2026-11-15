//! Resources: what units build up and spend to cast abilities.
//!
//! Mana is the resource the engine ships with, but nothing in the simulation
//! is specific to it. A [`ResourceDef`] describes one kind of resource: which
//! stat holds a unit's capacity, how much it starts a fight with, and how it
//! is gained (per attack, from damage taken or dealt, per kill, over time).
//! A game lists its resources in
//! [`CombatConfig::resources`](crate::combat::CombatConfig), and an ability
//! names the one it spends with a [`Cost`](crate::effect::Cost).
//!
//! A unit has a resource when its capacity stat is above zero, so one game
//! can mix units that run on different resources, and one unit can carry
//! several.
//!
//! ```
//! use autoengine::effect::Amount;
//! use autoengine::resource::ResourceDef;
//! use autoengine::stats::Stat;
//!
//! const MAX_RAGE: Stat = Stat::custom("max_rage");
//!
//! // Rage: built by fighting, lost over time, never handed out for free.
//! let rage = ResourceDef::new("rage", "Rage", MAX_RAGE)
//!     .per_attack(5)
//!     .from_damage_taken(100, Some(25))
//!     .from_damage_dealt(50, None)
//!     .per_second(-4);
//! assert_eq!(rage.per_attack, Amount::flat(5));
//! ```
//!
//! Anything the fixed gains do not cover is a passive with
//! [`Effect::resource`](crate::effect::Effect::resource): rage on an ally's
//! death, energy when a shield breaks, and so on.

use serde::{Deserialize, Serialize};

use crate::effect::Amount;
use crate::stats::Stat;

/// The id of the built-in mana resource.
pub const MANA: &str = "mana";

/// A share of some damage, turned into a resource.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    /// Resource gained per point of damage, in thousandths.
    pub permille: i64,
    /// The most one instance of damage can give. `None` is no limit.
    #[serde(default)]
    pub cap: Option<i64>,
}

impl Share {
    /// What `damage` is worth.
    pub fn of(&self, damage: i64) -> i64 {
        let gain = damage * self.permille / 1000;
        self.cap.map_or(gain, |cap| gain.min(cap))
    }
}

/// One kind of resource; see the [module docs](self).
///
/// Every [`Amount`] here is worked out for the unit that owns the resource,
/// so it can be a plain number or read one of the unit's stats (which lets
/// items and traits change it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceDef {
    /// What content calls it: `"mana"`.
    pub id: String,
    /// What players call it.
    pub name: String,
    /// The stat that holds how much of it a unit can have. A unit with none
    /// of this stat does not have the resource at all.
    pub max: Stat,
    /// How much a unit starts a fight with (no more than its capacity).
    #[serde(default)]
    pub start: Amount,
    /// Gained each time the unit makes a basic attack, hit or miss.
    #[serde(default)]
    pub per_attack: Amount,
    /// Gained from damage the unit survives, shields included.
    #[serde(default)]
    pub from_damage_taken: Share,
    /// Gained from damage the unit deals.
    #[serde(default)]
    pub from_damage_dealt: Share,
    /// Gained when the unit kills an enemy.
    #[serde(default)]
    pub per_kill: Amount,
    /// Gained once a second. Negative drains it.
    #[serde(default)]
    pub per_second: Amount,
    /// Whether it keeps changing while the unit is casting. Mana does not,
    /// so a long cast cannot fill the bar for the next one.
    #[serde(default)]
    pub changes_while_casting: bool,
    /// The color of its bar, as red, green and blue. Front ends pick their
    /// own when there is none.
    #[serde(default)]
    pub color: Option<[u8; 3]>,
}

impl ResourceDef {
    /// A resource nothing gives yet: add the ways to gain it.
    pub fn new(id: impl Into<String>, name: impl Into<String>, max: Stat) -> Self {
        ResourceDef {
            id: id.into(),
            name: name.into(),
            max,
            start: Amount::default(),
            per_attack: Amount::default(),
            from_damage_taken: Share::default(),
            from_damage_dealt: Share::default(),
            per_kill: Amount::default(),
            per_second: Amount::default(),
            changes_while_casting: false,
            color: None,
        }
    }

    /// The built-in mana: capacity `MaxMana`, starting at `StartingMana`,
    /// `ManaPerAttack` per basic attack and 3% of damage taken (at most 40
    /// per hit).
    pub fn mana() -> Self {
        ResourceDef::new(MANA, "Mana", Stat::MaxMana)
            .start(Amount::stat(Stat::StartingMana))
            .per_attack(Amount::stat(Stat::ManaPerAttack))
            .from_damage_taken(30, Some(40))
            .color([70, 130, 230])
    }

    pub fn start(mut self, amount: impl Into<Amount>) -> Self {
        self.start = amount.into();
        self
    }

    /// Starts every fight full.
    pub fn starts_full(mut self) -> Self {
        self.start = Amount::stat(self.max);
        self
    }

    pub fn per_attack(mut self, amount: impl Into<Amount>) -> Self {
        self.per_attack = amount.into();
        self
    }

    pub fn from_damage_taken(mut self, permille: i64, cap: Option<i64>) -> Self {
        self.from_damage_taken = Share { permille, cap };
        self
    }

    pub fn from_damage_dealt(mut self, permille: i64, cap: Option<i64>) -> Self {
        self.from_damage_dealt = Share { permille, cap };
        self
    }

    pub fn per_kill(mut self, amount: impl Into<Amount>) -> Self {
        self.per_kill = amount.into();
        self
    }

    /// A gain every second, or a drain when negative.
    pub fn per_second(mut self, amount: impl Into<Amount>) -> Self {
        self.per_second = amount.into();
        self
    }

    /// Keeps the resource changing while its owner casts.
    pub fn changes_while_casting(mut self) -> Self {
        self.changes_while_casting = true;
        self
    }

    pub fn color(mut self, rgb: [u8; 3]) -> Self {
        self.color = Some(rgb);
        self
    }
}

/// How much of one resource a unit has right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pool<'a> {
    pub def: &'a ResourceDef,
    pub amount: i64,
    /// The unit's capacity at the moment.
    pub max: i64,
}
