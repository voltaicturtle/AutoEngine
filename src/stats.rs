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

use std::collections::BTreeSet;
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A numeric property of a unit.
///
/// The engine gives the built-in stats their meaning. A game can add its own
/// with [`Stat::custom`]: the engine stores and modifies them like any other
/// stat, and content reads them through scalings, selectors, resources and
/// Rust hooks.
///
/// ```
/// use autoengine::stats::{Stat, StatBlock};
///
/// const LUCK: Stat = Stat::custom("luck");
///
/// let stats = StatBlock::base().with(LUCK, 7).with_mods(&[LUCK.flat(3)]);
/// assert_eq!(stats.get(LUCK), 10);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stat {
    MaxHealth,
    AttackDamage,
    AbilityPower,
    Armor,
    MagicResist,
    AttackSpeed,
    /// Attack range in hexes.
    Range,
    /// Capacity of the built-in mana resource, which is also what an ability
    /// costs by default. `0` means the unit has no mana.
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
    /// Chance, in thousandths, that a basic attack against the unit misses.
    Dodge,
    /// Cuts how long crowd control lasts on the unit, in thousandths (at
    /// most 80%).
    Tenacity,
    /// A stat defined by the game; see [`Stat::custom`].
    Custom(&'static str),
}

/// How many built-in stats there are.
pub const STAT_COUNT: usize = 18;

const NAMES: [&str; STAT_COUNT] = [
    "MaxHealth",
    "AttackDamage",
    "AbilityPower",
    "Armor",
    "MagicResist",
    "AttackSpeed",
    "Range",
    "MaxMana",
    "StartingMana",
    "ManaPerAttack",
    "CritChance",
    "CritDamage",
    "MoveSpeed",
    "DamageAmp",
    "DamageReduction",
    "Lifesteal",
    "Dodge",
    "Tenacity",
];

/// Longest name a custom stat may have, in bytes.
pub const MAX_STAT_NAME: usize = 48;

/// Most custom stat names one process will learn from data it reads (saved
/// games, messages). Names written in code do not count.
const MAX_LEARNED_NAMES: usize = 1024;

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const fn built_in(name: &str) -> Option<usize> {
    let mut i = 0;
    while i < STAT_COUNT {
        if same(NAMES[i], name) {
            return Some(i);
        }
        i += 1;
    }
    None
}

impl Stat {
    /// Every built-in stat.
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
        Stat::Dodge,
        Stat::Tenacity,
    ];

    /// A stat of the game's own, such as `Stat::custom("luck")`. Declare it
    /// once as a constant and use that everywhere. Units have 0 of it unless
    /// given some.
    ///
    /// # Panics
    ///
    /// If the name is empty, longer than [`MAX_STAT_NAME`] or the name of a
    /// built-in stat. In a `const` this is a compile error.
    pub const fn custom(name: &'static str) -> Stat {
        assert!(!name.is_empty() && name.len() <= MAX_STAT_NAME, "a custom stat needs a short, non-empty name");
        assert!(built_in(name).is_none(), "a custom stat cannot take the name of a built-in stat");
        Stat::Custom(name)
    }

    /// The stat with this name: a built-in one (`"Armor"`) or else a custom
    /// one. `None` if the name cannot be a stat's (see [`Stat::custom`]).
    pub fn named(name: &str) -> Option<Stat> {
        if let Some(i) = built_in(name) {
            return Some(Stat::ALL[i]);
        }
        if name.is_empty() || name.len() > MAX_STAT_NAME {
            return None;
        }
        // Custom stats hold a `&'static str` so that `Stat` stays `Copy`.
        // Names that arrive as data are kept for the life of the process,
        // up to a limit so that hostile input cannot grow it forever.
        static LEARNED: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
        let mut learned = LEARNED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(known) = learned.get(name) {
            return Some(Stat::Custom(known));
        }
        if learned.len() >= MAX_LEARNED_NAMES {
            return None;
        }
        let kept: &'static str = Box::leak(name.to_string().into_boxed_str());
        learned.insert(kept);
        Some(Stat::Custom(kept))
    }

    /// The stat's name: `"Armor"`, or the name a custom stat was given.
    pub const fn name(self) -> &'static str {
        match self.slot() {
            Ok(i) => NAMES[i],
            Err(name) => name,
        }
    }

    /// Whether the game defined this stat.
    pub const fn is_custom(self) -> bool {
        matches!(self, Stat::Custom(_))
    }

    /// Where a built-in stat is stored, or the name of a custom one.
    const fn slot(self) -> Result<usize, &'static str> {
        Ok(match self {
            Stat::MaxHealth => 0,
            Stat::AttackDamage => 1,
            Stat::AbilityPower => 2,
            Stat::Armor => 3,
            Stat::MagicResist => 4,
            Stat::AttackSpeed => 5,
            Stat::Range => 6,
            Stat::MaxMana => 7,
            Stat::StartingMana => 8,
            Stat::ManaPerAttack => 9,
            Stat::CritChance => 10,
            Stat::CritDamage => 11,
            Stat::MoveSpeed => 12,
            Stat::DamageAmp => 13,
            Stat::DamageReduction => 14,
            Stat::Lifesteal => 15,
            Stat::Dodge => 16,
            Stat::Tenacity => 17,
            Stat::Custom(name) => return Err(name),
        })
    }

    /// A flat modifier to this stat: `Stat::Armor.flat(15)`.
    pub const fn flat(self, amount: i64) -> StatMod {
        StatMod::flat(self, amount)
    }

    /// A percentage modifier to this stat, in permille:
    /// `Stat::AttackSpeed.percent(200)` is +20%.
    pub const fn percent(self, permille: i64) -> StatMod {
        StatMod::percent(self, permille)
    }
}

impl std::fmt::Display for Stat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Stats are written as their name, so they can be the keys of a JSON map.
impl Serialize for Stat {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for Stat {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = std::borrow::Cow::<str>::deserialize(deserializer)?;
        Stat::named(&name).ok_or_else(|| serde::de::Error::custom(format!("`{name}` cannot be the name of a stat")))
    }
}

/// One value per [`Stat`]. Custom stats a block was never given are 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatBlock {
    core: [i64; STAT_COUNT],
    /// Custom stats that are not zero, ordered by name.
    custom: Vec<(&'static str, i64)>,
}

impl StatBlock {
    /// All stats zero.
    pub const fn zero() -> Self {
        StatBlock { core: [0; STAT_COUNT], custom: Vec::new() }
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
        match stat.slot() {
            Ok(i) => self.core[i],
            Err(name) => self.custom.iter().find(|(n, _)| *n == name).map_or(0, |(_, v)| *v),
        }
    }

    pub fn set(&mut self, stat: Stat, value: i64) {
        match stat.slot() {
            Ok(i) => self.core[i] = value,
            Err(name) => match self.custom.binary_search_by(|(n, _)| (*n).cmp(name)) {
                Ok(at) if value == 0 => {
                    self.custom.remove(at);
                }
                Ok(at) => self.custom[at].1 = value,
                Err(_) if value == 0 => {}
                Err(at) => self.custom.insert(at, (name, value)),
            },
        }
    }

    pub fn add(&mut self, stat: Stat, delta: i64) {
        match stat.slot() {
            Ok(i) => self.core[i] += delta,
            Err(_) => self.set(stat, self.get(stat) + delta),
        }
    }

    /// The custom stats the block holds a value for, ordered by name.
    pub fn custom(&self) -> impl Iterator<Item = (Stat, i64)> + '_ {
        self.custom.iter().map(|(name, value)| (Stat::Custom(name), *value))
    }

    /// Returns a new block with the modifiers permanently folded in, using
    /// the same stacking rules as [`Stats`].
    pub fn with_mods(&self, mods: &[StatMod]) -> StatBlock {
        let mut stats = Stats::new(self.clone());
        for m in mods {
            stats.apply(m);
        }
        let mut out = StatBlock::zero();
        let custom = self.custom().map(|(stat, _)| stat).chain(mods.iter().map(|m| m.stat).filter(|s| s.is_custom()));
        for stat in Stat::ALL.into_iter().chain(custom) {
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

    /// Whether it makes the unit worse: a cut to any stat, except the cost
    /// of casting, where a cut helps.
    pub fn lowers(&self) -> bool {
        let sign = if self.stat == Stat::MaxMana { -1 } else { 1 };
        self.flat * sign < 0 || self.permille * sign < 0
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
            assert_eq!(s.slot(), Ok(i));
            assert_eq!(Stat::named(s.name()), Some(*s));
        }
    }

    #[test]
    fn custom_stats_behave_like_built_in_ones() {
        const LUCK: Stat = Stat::custom("luck");
        let mut s = Stats::new(StatBlock::base().with(LUCK, 10));
        s.apply(&LUCK.flat(10));
        s.apply(&LUCK.percent(500));
        assert_eq!(s.get(LUCK), 30);
        assert_eq!(s.get(Stat::custom("unset")), 0);
        s.remove(&LUCK.flat(10));
        assert_eq!(s.get(LUCK), 15);

        let folded = StatBlock::base().with_mods(&[LUCK.flat(4)]);
        assert_eq!(folded.get(LUCK), 4);
        assert_eq!(folded.custom().collect::<Vec<_>>(), vec![(LUCK, 4)]);
        // Setting a custom stat back to zero leaves no trace, so equal
        // blocks compare equal however they were built.
        assert_eq!(folded.with(LUCK, 0), StatBlock::base());
    }

    #[test]
    fn stats_are_written_by_name() {
        let luck = Stat::custom("luck");
        assert_eq!(serde_json::to_string(&[Stat::Armor, luck]).unwrap(), r#"["Armor","luck"]"#);
        let back: Vec<Stat> = serde_json::from_str(r#"["Armor","luck"]"#).unwrap();
        assert_eq!(back, vec![Stat::Armor, luck]);
        assert!(serde_json::from_str::<Stat>(r#""""#).is_err());
        assert!(Stat::named(&"x".repeat(MAX_STAT_NAME + 1)).is_none());
    }

    #[test]
    #[should_panic(expected = "built-in")]
    fn a_custom_stat_cannot_shadow_a_built_in_one() {
        let _ = Stat::custom("Armor");
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
