//! Names for things a game defines itself: stats, damage types, signals.
//!
//! The types these names sit in ([`Stat`](crate::stats::Stat),
//! [`DamageKind`](crate::effect::DamageKind),
//! [`Trigger`](crate::effect::Trigger)) are small `Copy` values that are
//! compared all the time, so a name is a `&'static str` rather than a
//! `String`. In code that is a literal. Names read from data (a saved game,
//! a message from a server) are kept for the life of the process, each
//! once, up to a limit so that hostile input cannot grow memory forever.

use std::collections::BTreeSet;
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Longest name allowed, in bytes.
pub const MAX_NAME: usize = 48;

/// Most names one process will learn from data it reads. Names written in
/// code do not count.
const MAX_LEARNED_NAMES: usize = 1024;

/// Turns a name read from data into one that lives as long as the process.
/// `None` for a name that is empty, longer than [`MAX_NAME`] or one too
/// many.
pub(crate) fn intern(name: &str) -> Option<&'static str> {
    static LEARNED: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    if name.is_empty() || name.len() > MAX_NAME {
        return None;
    }
    let mut learned = LEARNED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(known) = learned.get(name) {
        return Some(known);
    }
    if learned.len() >= MAX_LEARNED_NAMES {
        return None;
    }
    let kept: &'static str = Box::leak(name.to_string().into_boxed_str());
    learned.insert(kept);
    Some(kept)
}

/// Whether two strings are equal, for use in a `const fn`.
pub(crate) const fn same(a: &str, b: &str) -> bool {
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

/// The name of something a game defines, such as a damage type or a signal.
/// Make one from a literal with [`Name::new`] or `.into()`; it is written
/// to and read from data as plain text.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(&'static str);

impl Name {
    /// # Panics
    ///
    /// If the name is empty or longer than [`MAX_NAME`]. In a `const` this
    /// is a compile error.
    pub const fn new(name: &'static str) -> Self {
        assert!(!name.is_empty() && name.len() <= MAX_NAME, "a name must be short and not empty");
        Name(name)
    }

    /// The name read from data, or `None` if it cannot be one.
    pub fn parse(name: &str) -> Option<Self> {
        intern(name).map(Name)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl From<&'static str> for Name {
    fn from(name: &'static str) -> Self {
        Name::new(name)
    }
}

impl std::fmt::Debug for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.0, f)
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl Serialize for Name {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for Name {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = std::borrow::Cow::<str>::deserialize(deserializer)?;
        Name::parse(&name).ok_or_else(|| serde::de::Error::custom(format!("`{name}` cannot be used as a name")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_survive_a_round_trip_and_bad_ones_are_refused() {
        let fire = Name::new("fire");
        assert_eq!(serde_json::to_string(&fire).unwrap(), r#""fire""#);
        assert_eq!(serde_json::from_str::<Name>(r#""fire""#).unwrap(), fire);
        assert_eq!(fire, "fire");
        assert!(serde_json::from_str::<Name>(r#""""#).is_err());
        assert!(Name::parse(&"x".repeat(MAX_NAME + 1)).is_none());
        // The same name read twice is stored once.
        let (a, b) = (Name::parse("read_twice").unwrap(), Name::parse("read_twice").unwrap());
        assert!(std::ptr::eq(a.as_str(), b.as_str()));
    }
}
