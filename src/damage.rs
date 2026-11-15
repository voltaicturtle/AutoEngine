//! Damage types: how each kind of damage is mitigated.
//!
//! Physical, magic and true damage are the kinds the engine ships with, and
//! each is described by a [`DamageType`] in
//! [`CombatConfig::damage_types`](crate::combat::CombatConfig). A game can
//! change those descriptions or add kinds of its own: fire that a fire
//! resistance stat cuts, poison that goes through shields, and so on.
//!
//! ```
//! use autoengine::damage::DamageType;
//! use autoengine::effect::{DamageKind, Effect};
//! use autoengine::stats::Stat;
//!
//! const FIRE: DamageKind = DamageKind::custom("fire");
//! const FIRE_RESIST: Stat = Stat::custom("fire_resist");
//! const FIRE_POWER: Stat = Stat::custom("fire_power");
//!
//! let fire = DamageType::new(FIRE, "Fire").resisted_by(FIRE_RESIST).amplified_by(FIRE_POWER);
//! let scorch = Effect::damage(120, FIRE);
//! assert_eq!(fire.kind, FIRE);
//! # let _ = scorch;
//! ```

use serde::{Deserialize, Serialize};

use crate::effect::DamageKind;
use crate::stats::Stat;

fn yes() -> bool {
    true
}

/// The rules for one kind of damage; see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DamageType {
    /// The kind these rules are for.
    pub kind: DamageKind,
    /// What players call it.
    pub name: String,
    /// The target's stat that cuts it: damage taken is
    /// `damage × k / (k + resist)`, with `k` the fight's
    /// `resist_constant`. `None` is never resisted.
    #[serde(default)]
    pub resist: Option<Stat>,
    /// A stat of the source that adds to it, in thousandths, on top of
    /// `DamageAmp`.
    #[serde(default)]
    pub amp: Option<Stat>,
    /// Whether the target's `DamageReduction` cuts it.
    #[serde(default = "yes")]
    pub reducible: bool,
    /// Whether shields soak it before health does.
    #[serde(default = "yes")]
    pub shieldable: bool,
    /// The color of its damage numbers, as red, green and blue. Front ends
    /// pick their own when there is none.
    #[serde(default)]
    pub color: Option<[u8; 3]>,
}

impl DamageType {
    /// Damage that nothing resists, that damage reduction cuts and that
    /// shields soak.
    pub fn new(kind: DamageKind, name: impl Into<String>) -> Self {
        DamageType { kind, name: name.into(), resist: None, amp: None, reducible: true, shieldable: true, color: None }
    }

    /// The built-in physical damage: cut by `Armor`.
    pub fn physical() -> Self {
        DamageType::new(DamageKind::Physical, "Physical").resisted_by(Stat::Armor).color([255, 140, 77])
    }

    /// The built-in magic damage: cut by `MagicResist`.
    pub fn magic() -> Self {
        DamageType::new(DamageKind::Magic, "Magic").resisted_by(Stat::MagicResist).color([140, 166, 255])
    }

    /// The built-in true damage: nothing resists or reduces it.
    pub fn true_damage() -> Self {
        DamageType::new(DamageKind::True, "True").ignores_reduction().color([255, 255, 255])
    }

    /// The three built-in kinds.
    pub fn defaults() -> Vec<Self> {
        vec![DamageType::physical(), DamageType::magic(), DamageType::true_damage()]
    }

    /// The target's stat that cuts it.
    pub fn resisted_by(mut self, stat: Stat) -> Self {
        self.resist = Some(stat);
        self
    }

    /// Nothing resists it.
    pub fn unresisted(mut self) -> Self {
        self.resist = None;
        self
    }

    /// A stat of the source that adds to it, in thousandths.
    pub fn amplified_by(mut self, stat: Stat) -> Self {
        self.amp = Some(stat);
        self
    }

    /// The target's `DamageReduction` does not cut it.
    pub fn ignores_reduction(mut self) -> Self {
        self.reducible = false;
        self
    }

    /// Goes straight to health, past any shields.
    pub fn ignores_shields(mut self) -> Self {
        self.shieldable = false;
        self
    }

    pub fn color(mut self, rgb: [u8; 3]) -> Self {
        self.color = Some(rgb);
        self
    }
}
