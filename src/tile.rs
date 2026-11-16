//! Tiles: effects that belong to a place on the board instead of a unit.
//!
//! A [`TileDef`] describes one kind of tile: what happens to a unit that
//! steps onto it, stands on it or leaves it, and whether it can be stood on
//! at all. A game lists its tiles in
//! [`CombatConfig::tiles`](crate::combat::CombatConfig) (with
//! [`GameBuilder::tile`](crate::content::GameBuilder::tile)), and tiles get
//! onto the board in three ways:
//!
//! - an ability or passive lays them with
//!   [`Effect::tile`](crate::effect::Effect::tile), for a while or for the
//!   whole fight;
//! - a round has them from the start (see
//!   [`RoundDef::tile`](crate::content::RoundDef::tile));
//! - Rust code calls [`Combat::place_tile`](crate::combat::Combat::place_tile).
//!
//! ```
//! use autoengine::effect::Effect;
//! use autoengine::stats::Stat;
//! use autoengine::tile::TileDef;
//!
//! // Burning ground: hurts the caster's enemies while they stand in it.
//! let embers = TileDef::new("embers", "Embers").enemies_only().per_second(Effect::magic(40));
//! // A shrine: whoever stands on it hits harder.
//! let shrine = TileDef::new("shrine", "Shrine").mods([Stat::AttackDamage.percent(200)]);
//! // A boulder: nobody can stand there, and units walk around it.
//! let boulder = TileDef::new("boulder", "Boulder").blocking();
//! assert!(boulder.blocks && !embers.blocks && shrine.mods.len() == 1);
//! ```
//!
//! Effects on a tile have no selector: they happen to the unit on the tile.
//! Their source is the unit that laid the tile, or the unit on it when
//! nobody did, so amounts that scale with the source's stats work for tiles
//! laid by abilities. For anything wider, give the tile an
//! [`Effect::signal`](crate::effect::Effect::signal) and let a passive
//! answer it.

use serde::{Deserialize, Serialize};

use crate::effect::Effect;
use crate::stats::StatMod;

/// Whom a tile does something to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Affects {
    /// Any unit on it.
    #[default]
    Everyone,
    /// Units on the side of whoever laid it.
    Allies,
    /// Units on the other side.
    Enemies,
}

/// One kind of tile; see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileDef {
    /// What content calls it: `"embers"`.
    pub id: String,
    /// What players call it.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Nobody can stand on it, and units find their way around it.
    #[serde(default)]
    pub blocks: bool,
    /// Whom it does something to. A tile nobody laid (one a round starts
    /// with) has no side, and affects everyone whatever this says.
    #[serde(default)]
    pub affects: Affects,
    /// Happens to a unit when it steps onto the tile, or when the tile
    /// appears under it.
    #[serde(default)]
    pub on_enter: Vec<Effect>,
    /// Happens to a unit when it steps off the tile.
    #[serde(default)]
    pub on_leave: Vec<Effect>,
    /// Happens once a second to the unit standing on it.
    #[serde(default)]
    pub per_second: Vec<Effect>,
    /// Stat changes a unit has for as long as it stands on the tile.
    #[serde(default)]
    pub mods: Vec<StatMod>,
    /// The color clients tint the hex with, as red, green and blue.
    #[serde(default)]
    pub color: Option<[u8; 3]>,
}

impl TileDef {
    /// A tile that does nothing yet and affects everyone.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        TileDef {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            blocks: false,
            affects: Affects::Everyone,
            on_enter: Vec::new(),
            on_leave: Vec::new(),
            per_second: Vec::new(),
            mods: Vec::new(),
            color: None,
        }
    }

    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = text.into();
        self
    }

    /// Makes it a hex nobody can stand on.
    pub fn blocking(mut self) -> Self {
        self.blocks = true;
        self
    }

    /// Only affects units on the side of whoever laid it.
    pub fn allies_only(mut self) -> Self {
        self.affects = Affects::Allies;
        self
    }

    /// Only affects the enemies of whoever laid it.
    pub fn enemies_only(mut self) -> Self {
        self.affects = Affects::Enemies;
        self
    }

    /// Adds something that happens to a unit stepping onto the tile.
    pub fn on_enter(mut self, effect: Effect) -> Self {
        self.on_enter.push(effect);
        self
    }

    /// Adds something that happens to a unit stepping off the tile.
    pub fn on_leave(mut self, effect: Effect) -> Self {
        self.on_leave.push(effect);
        self
    }

    /// Adds something that happens every second to the unit on the tile.
    pub fn per_second(mut self, effect: Effect) -> Self {
        self.per_second.push(effect);
        self
    }

    /// Adds stat changes for whoever stands on the tile.
    pub fn mods(mut self, mods: impl IntoIterator<Item = StatMod>) -> Self {
        self.mods.extend(mods);
        self
    }

    pub fn color(mut self, rgb: [u8; 3]) -> Self {
        self.color = Some(rgb);
        self
    }

    /// Every effect the tile can cause.
    pub fn effects(&self) -> impl Iterator<Item = &Effect> {
        self.on_enter
            .iter()
            .chain(&self.on_leave)
            .chain(&self.per_second)
    }
}

/// A tile a fight starts with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TilePlacement {
    /// The tile's id.
    pub tile: String,
    /// `(column, row)` on the board, from the home side's point of view.
    pub pos: (i32, i32),
    /// Also lay it on the same hex as the other side sees it, so neither
    /// side has the better ground.
    #[serde(default)]
    pub mirrored: bool,
}

/// A tile on the board right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileView<'a> {
    pub def: &'a TileDef,
    pub pos: crate::hex::Hex,
    /// The side of whoever laid it, if anyone did.
    pub team: Option<crate::combat::Team>,
    /// How long it has left. `None` is the rest of the fight.
    pub remaining_ms: Option<u32>,
}
