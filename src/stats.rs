//! Unit statistics and modifiers.
//!
//! Every stat is an integer so that combat is bit-for-bit reproducible on
//! every platform. Fractional quantities use fixed units:
//!
//! * `AttackSpeed`: thousandths of an attack per second (`750` = 0.75/s).
//! * `MoveSpeed`: thousandths of a hex per second.
//! * `CritChance`, `DamageAmp`, `DamageReduction`, `Lifesteal`: permille
//!   (`250` = 25%).
//! * `CritDamage`: permille of normal damage (`1400` = 140%).
//! * `AbilityPower`: percent multiplier on power-scaled amounts (`100` = 1x).
//! * Everything else is a plain amount.

use serde::{Deserialize, Serialize};

/// A numeric property of a unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Stat {
    MaxHealth,
    AttackDamage,
    AbilityPower,
    Armor,
    MagicResist,
    AttackSpeed,
    /// Attack range in hexes.
    Range,
    /// Mana needed to cast. `0` means the unit never casts.
    MaxMana,
    StartingMana,
    /// Mana gained per basic attack.
    ManaPerAttack,
    CritChance,
    CritDamage,
    MoveSpeed,
    /// Bonus to all damage dealt.
    DamageAmp,
    /// Reduction to all damage taken (capped by the combat config).
    DamageReduction,
    /// Share of damage dealt that is returned as healing.
    Lifesteal,
}

pub const STAT_COUNT: usize = 16;

impl Stat {
    pub const ALL: [Stat; STAT_COUNT] = [
        Stat::MaxHealth,
        Stat::AttackDamage,
        Stat::AbilityPower,
        Stat::Armor,
        Stat::MagicResist,
        Stat::AttackSpeed,
        Stat::Range,
        Stat::MaxMana,
        Stat::StartingMana,
        Stat::ManaPerAttack,
        Stat::CritChance,
        Stat::CritDamage,
        Stat::MoveSpeed,
        Stat::DamageAmp,
        Stat::DamageReduction,
        Stat::Lifesteal,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

/// One value per [`Stat`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatBlock([i64; STAT_COUNT]);

impl StatBlock {
    /// All stats zero.
    pub const fn zero() -> Self {
        StatBlock([0; STAT_COUNT])
    }

    /// Reasonable defaults for a melee unit; override what you need with
    /// [`StatBlock::with`].
    pub fn base() -> Self {
        StatBlock::zero()
            .with(Stat::MaxHealth, 500)
            .with(Stat::AttackDamage, 50)
            .with(Stat::AbilityPower, 100)
            .with(Stat::AttackSpeed, 700)
            .with(Stat::Range, 1)
            .with(Stat::ManaPerAttack, 10)
            .with(Stat::CritChance, 250)
            .with(Stat::CritDamage, 1400)
            .with(Stat::MoveSpeed, 1800)
    }

    pub fn with(mut self, stat: Stat, value: i64) -> Self {
        self.set(stat, value);
        self
    }

    pub fn get(&self, stat: Stat) -> i64 {
        self.0[stat.index()]
    }

    pub fn set(&mut self, stat: Stat, value: i64) {
        self.0[stat.index()] = value;
    }

    pub fn add(&mut self, stat: Stat, delta: i64) {
        self.0[stat.index()] += delta;
    }

    /// Returns a new block with the modifiers permanently folded in, using
    /// the same stacking rules as [`Stats`].
    pub fn with_mods(&self, mods: &[StatMod]) -> StatBlock {
        let mut stats = Stats::new(self.clone());
        for m in mods {
            stats.apply(m);
        }
        let mut out = StatBlock::zero();
        for stat in Stat::ALL {
            out.set(stat, stats.get(stat));
        }
        out
    }
}

/// A change to one stat: a flat amount, a percentage (permille) of the
/// flat-adjusted value, or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatMod {
    pub stat: Stat,
    #[serde(default)]
    pub flat: i64,
    #[serde(default)]
    pub permille: i64,
}

impl StatMod {
    pub const fn flat(stat: Stat, amount: i64) -> Self {
        StatMod { stat, flat: amount, permille: 0 }
    }

    pub const fn percent(stat: Stat, permille: i64) -> Self {
        StatMod { stat, flat: 0, permille }
    }
}

/// A unit's base stats plus all currently applied modifiers.
///
/// Effective value = `(base + flat) * (1000 + permille) / 1000`, with all
/// flat modifiers summed and all percentage modifiers summed (additive
/// stacking), then clamped at zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    base: StatBlock,
    flat: StatBlock,
    permille: StatBlock,
}

impl Stats {
    pub fn new(base: StatBlock) -> Self {
        Stats { base, flat: StatBlock::zero(), permille: StatBlock::zero() }
    }

    pub fn base(&self) -> &StatBlock {
        &self.base
    }

    pub fn get(&self, stat: Stat) -> i64 {
        let flat = self.base.get(stat) + self.flat.get(stat);
        let scaled = flat * (1000 + self.permille.get(stat)) / 1000;
        scaled.max(0)
    }

    pub fn apply(&mut self, m: &StatMod) {
        self.flat.add(m.stat, m.flat);
        self.permille.add(m.stat, m.permille);
    }

    pub fn remove(&mut self, m: &StatMod) {
        self.flat.add(m.stat, -m.flat);
        self.permille.add(m.stat, -m.permille);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_all_matches_indices() {
        for (i, s) in Stat::ALL.iter().enumerate() {
            assert_eq!(s.index(), i);
        }
    }

    #[test]
    fn modifiers_stack_and_revert() {
        let mut s = Stats::new(StatBlock::base().with(Stat::AttackDamage, 100));
        let a = StatMod::flat(Stat::AttackDamage, 20);
        let b = StatMod::percent(Stat::AttackDamage, 500);
        s.apply(&a);
        s.apply(&b);
        assert_eq!(s.get(Stat::AttackDamage), 180);
        s.remove(&a);
        assert_eq!(s.get(Stat::AttackDamage), 150);
        s.remove(&b);
        assert_eq!(s.get(Stat::AttackDamage), 100);
    }

    #[test]
    fn effective_stats_never_negative() {
        let mut s = Stats::new(StatBlock::base().with(Stat::Armor, 10));
        s.apply(&StatMod::flat(Stat::Armor, -50));
        assert_eq!(s.get(Stat::Armor), 0);
    }
}
