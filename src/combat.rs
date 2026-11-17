//! The deterministic combat simulation.
//!
//! A [`Combat`] is a self-contained fight on a hex board between any number
//! of teams. It advances in fixed ticks using only integer math and a seeded
//! [`Rng`], so the same seed and the same starting units always produce the
//! same fight, on any machine. Everything that happens is recorded as an
//! [`Event`]; renderers, logs and network clients read that stream and the
//! simulation itself never does any I/O.
//!
//! Units act in id order within a tick. Units spawned during a tick start
//! acting on the next one.
//!
//! Games can extend the simulation with Rust hooks: a [`CustomEffectFn`]
//! runs for [`Effect::Custom`] and a [`CustomConditionFn`] answers
//! [`Condition::Custom`]. Hooks see the whole fight and act through an
//! [`EffectContext`], which only exposes operations that keep the
//! simulation consistent and reproducible.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::effect::{
    Amount, Anchor, Condition, Cost, DamageKind, Effect, EffectSpec, ScaleBy, Selector, StatusKind, Targeting,
    Trigger, UnitTemplate, Who,
};
use crate::damage::DamageType;
use crate::hex::{Hex, HexBoard};
use crate::resource::{MANA, Pool, ResourceDef};
use crate::rng::Rng;
use crate::stats::{Stat, StatMod, Stats};
use crate::tile::{Affects, TileDef, TileView};

/// Identifies a unit within one combat. Ids are assigned in spawn order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UnitId(pub u32);

/// Identifies a side. Units on different teams are enemies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Team(pub u8);

/// Progress needed for one attack or one step: speeds are in thousandths per
/// second and time is in milliseconds.
const PROGRESS_FULL: i64 = 1_000_000;

/// Upper bound on triggered passives resolved in one go, as a guard against
/// content that triggers itself forever.
const MAX_TRIGGER_CHAIN: usize = 10_000;
/// How many tiles' effects can push a unit from one tile onto the next.
const MAX_TILE_CHAIN: u32 = 4;

/// How deeply custom effects may call other custom effects.
const MAX_HOOK_DEPTH: u32 = 16;

/// A Rust hook run by [`Effect::Custom`], once per selected unit.
///
/// Hooks must be deterministic: use [`EffectContext::rng`] for randomness and
/// never read clocks, global state or anything else outside the combat.
pub type CustomEffectFn = dyn Fn(&mut EffectContext<'_>) + Send + Sync;

/// A Rust predicate checked by [`Condition::Custom`]. The same determinism
/// rules as [`CustomEffectFn`] apply.
pub type CustomConditionFn = dyn Fn(&ConditionContext<'_>) -> bool + Send + Sync;

/// A Rust hook that picks the units for [`Selector::Custom`]. It must give
/// the same units in the same order every time for the same fight, so sort
/// by something stable such as the unit id. Dead units it returns are
/// skipped.
pub type CustomSelectorFn = dyn Fn(&SelectContext<'_>) -> Vec<UnitId> + Send + Sync;

/// Tunable rules of the simulation. A game sets them in
/// [`GameRules::combat`](crate::content::GameRules).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CombatConfig {
    /// Length of one simulation step.
    pub tick_ms: u32,
    /// The fight ends in a draw after this long.
    pub max_duration_ms: u32,
    /// The resources units build up and spend to cast: mana, unless the
    /// game says otherwise. See the [`resource`](crate::resource) module.
    pub resources: Vec<ResourceDef>,
    /// How each kind of damage is mitigated. See the
    /// [`damage`](crate::damage) module.
    pub damage_types: Vec<DamageType>,
    /// The kinds of tile that can be on the board. See the
    /// [`tile`](crate::tile) module.
    pub tiles: Vec<TileDef>,
    /// Damage multiplier from resistance is `k / (k + resist)`.
    pub resist_constant: i64,
    /// Cap on the `DamageReduction` stat, in permille.
    pub max_damage_reduction: i64,
    /// From this point on all damage grows every second, so fights between
    /// two boards that cannot finish each other still end. `0` turns
    /// overtime off.
    pub overtime_after_ms: u32,
    /// How much more damage every second of overtime adds, in thousandths:
    /// at 250, damage is doubled four seconds in.
    pub overtime_damage_per_second: i64,
}

impl Default for CombatConfig {
    fn default() -> Self {
        CombatConfig {
            tick_ms: 50,
            max_duration_ms: 60_000,
            resources: vec![ResourceDef::mana()],
            damage_types: DamageType::defaults(),
            tiles: Vec::new(),
            resist_constant: 100,
            max_damage_reduction: 900,
            overtime_after_ms: 30_000,
            overtime_damage_per_second: 250,
        }
    }
}

#[derive(Clone, Debug)]
struct StatusInstance {
    kind: StatusKind,
    remaining_ms: u32,
    source: UnitId,
}

#[derive(Clone, Debug)]
struct ShieldInstance {
    amount: i64,
    remaining_ms: Option<u32>,
}

#[derive(Clone, Debug, Default)]
struct PassiveState {
    procs: u32,
    timer_ms: u32,
}

#[derive(Clone, Debug, Default)]
struct StackState {
    count: u32,
    /// Stat modifiers granted so far, removed again when the stack clears.
    granted: Vec<StatMod>,
}

/// A unit taking part in a combat.
#[derive(Clone, Debug)]
pub struct CombatUnit {
    id: UnitId,
    team: Team,
    template: Arc<UnitTemplate>,
    pos: Hex,
    stats: Stats,
    health: i64,
    /// The fight's resource rules, and how much of each the unit has.
    resource_defs: Arc<[ResourceDef]>,
    resources: Vec<i64>,
    alive: bool,
    target: Option<UnitId>,
    attack_progress: i64,
    move_progress: i64,
    cast_remaining_ms: Option<u32>,
    /// Which ability is being cast, by position in the template.
    casting: usize,
    /// Time until each ability can be cast again.
    cooldowns: Vec<u32>,
    /// Time until the unit can gain each resource again (see
    /// `ResourceDef::lock_after_cast_ms`).
    resource_locks: Vec<u32>,
    /// Time since the last once-a-second resource change.
    second_ms: u32,
    /// The tick the unit last died on, while it is dead.
    died_at: Option<u32>,
    /// The tiles whose stat changes the unit has right now.
    on_tiles: Vec<OnTile>,
    shields: Vec<ShieldInstance>,
    statuses: Vec<StatusInstance>,
    passive_state: Vec<PassiveState>,
    stacks: BTreeMap<String, StackState>,
}

impl CombatUnit {
    pub fn id(&self) -> UnitId {
        self.id
    }
    pub fn team(&self) -> Team {
        self.team
    }
    pub fn template(&self) -> &UnitTemplate {
        &self.template
    }
    pub fn name(&self) -> &str {
        &self.template.name
    }
    pub fn pos(&self) -> Hex {
        self.pos
    }
    pub fn is_alive(&self) -> bool {
        self.alive
    }
    pub fn health(&self) -> i64 {
        self.health
    }
    /// How much mana the unit has; short for `resource("mana")`.
    pub fn mana(&self) -> i64 {
        self.resource(MANA)
    }
    /// How much of a resource the unit has, by resource id. Zero for a
    /// resource the unit or the fight does not have.
    pub fn resource(&self, id: &str) -> i64 {
        self.pool(id).map_or(0, |p| p.amount)
    }
    /// One of the unit's resources, by id. `None` if it does not have it.
    pub fn pool(&self, id: &str) -> Option<Pool<'_>> {
        self.pools().find(|p| p.def.id == id)
    }
    /// Every resource the unit has, in the order the fight lists them.
    pub fn pools(&self) -> impl Iterator<Item = Pool<'_>> {
        self.resource_defs
            .iter()
            .zip(&self.resources)
            .map(|(def, amount)| Pool { def, amount: *amount, max: self.stats.get(def.max) })
            .filter(|p| p.max > 0)
    }
    /// Time until one of the unit's abilities (by position) can be cast
    /// again.
    pub fn cooldown_ms(&self, ability: usize) -> u32 {
        self.cooldowns.get(ability).copied().unwrap_or(0)
    }
    /// The ability being cast, by position, if the unit is casting.
    pub fn casting(&self) -> Option<usize> {
        self.cast_remaining_ms.map(|_| self.casting)
    }
    /// Total remaining shield.
    pub fn shield(&self) -> i64 {
        self.shields.iter().map(|s| s.amount).sum()
    }
    /// Effective value of a stat, including all modifiers.
    pub fn stat(&self, stat: Stat) -> i64 {
        self.stats.get(stat)
    }
    pub fn target(&self) -> Option<UnitId> {
        self.target
    }
    pub fn is_casting(&self) -> bool {
        self.cast_remaining_ms.is_some()
    }
    pub fn is_stunned(&self) -> bool {
        self.has_status(|s| matches!(s, StatusKind::Stun))
    }
    /// Whether any status on the unit matches.
    pub fn has_status(&self, matches: impl Fn(&StatusKind) -> bool) -> bool {
        self.statuses.iter().any(|s| matches(&s.kind))
    }
    /// The statuses on the unit, with the time each has left.
    pub fn statuses(&self) -> impl Iterator<Item = (&StatusKind, u32)> {
        self.statuses.iter().map(|s| (&s.kind, s.remaining_ms))
    }
    /// Every stack the unit has, by name.
    pub fn all_stacks(&self) -> impl Iterator<Item = (&str, u32)> {
        self.stacks.iter().filter(|(_, s)| s.count > 0).map(|(k, s)| (k.as_str(), s.count))
    }
    /// Current count of a named stack (zero if the unit has none).
    pub fn stacks(&self, stack: &str) -> u32 {
        self.stacks.get(stack).map_or(0, |s| s.count)
    }
    pub fn has_tag(&self, tag: &str) -> bool {
        self.template.tags.iter().any(|t| t == tag)
    }
    /// Whether enemies can choose it right now.
    pub fn is_targetable(&self) -> bool {
        self.alive && !self.has_status(|s| matches!(s, StatusKind::Untargetable))
    }
}

/// Something that happened during combat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    Spawned { unit: UnitId, team: Team, pos: Hex, name: String, max_health: i64 },
    Moved { unit: UnitId, from: Hex, to: Hex },
    /// Moved by an effect (knockback, leap, a custom hook) rather than by
    /// walking.
    Displaced { unit: UnitId, from: Hex, to: Hex },
    /// A basic attack missed because the target dodged it.
    Dodged { source: UnitId, target: UnitId },
    /// The fight has gone on long enough that damage starts to grow (see
    /// [`CombatConfig::overtime_after_ms`]).
    Overtime,
    Attacked { source: UnitId, target: UnitId },
    Damaged {
        source: UnitId,
        target: UnitId,
        /// Health lost.
        amount: i64,
        /// Damage soaked by shields.
        absorbed: i64,
        kind: DamageKind,
        crit: bool,
    },
    /// `source` is whoever did the healing (the unit itself for lifesteal).
    Healed { source: UnitId, target: UnitId, amount: i64 },
    Shielded { source: UnitId, target: UnitId, amount: i64 },
    /// `ability` is the position of the ability among the unit's own.
    CastStarted {
        unit: UnitId,
        #[serde(default)]
        ability: usize,
    },
    CastFinished {
        unit: UnitId,
        #[serde(default)]
        ability: usize,
    },
    PassiveTriggered { unit: UnitId, passive: usize },
    StatusApplied { source: UnitId, target: UnitId, status: StatusKind, duration_ms: u32 },
    StatusExpired { target: UnitId, status: StatusKind },
    StacksChanged { unit: UnitId, stack: String, count: u32 },
    /// A unit now has `amount` of the resource with this id, out of `max`.
    /// Sent when either number changes, and for each resource a unit has
    /// when it enters the fight.
    ResourceChanged { unit: UnitId, resource: String, amount: i64, max: i64 },
    Died { unit: UnitId, killer: UnitId },
    /// A tile is on a hex, newly laid or laid again. `team` is the side of
    /// whoever laid it, and `duration_ms` how long it has left (`None` is
    /// the rest of the fight).
    TilePlaced {
        tile: String,
        pos: Hex,
        team: Option<Team>,
        duration_ms: Option<u32>,
    },
    /// A tile is gone from a hex.
    TileRemoved {
        tile: String,
        pos: Hex,
    },
    /// A dead unit is back, at `pos` with `health`.
    Revived { unit: UnitId, source: UnitId, pos: Hex, health: i64 },
    /// The player whose unit this is earned something to keep after the
    /// fight (see [`Effect::reward`]).
    Rewarded { unit: UnitId, team: Team, grant: crate::game::Grant },
    /// A unit's stats changed for good (see [`Effect::grow`]).
    Grew { unit: UnitId, mods: Vec<StatMod> },
    /// A unit turned into another one: `into` is the new unit's id and
    /// `name` what it is called now.
    Transformed { unit: UnitId, into: String, name: String },
    /// A unit's health and most health were set outright: a copy that
    /// starts hurt, a new form.
    HealthSet { unit: UnitId, health: i64, max_health: i64 },
    Ended { winner: Option<Team>, timed_out: bool },
}

/// An [`EventKind`] stamped with the tick it happened on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub tick: u32,
    pub kind: EventKind,
}

/// How a combat finished.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    /// The last team standing, or `None` for a draw.
    pub winner: Option<Team>,
    pub ticks: u32,
    pub duration_ms: u32,
    pub timed_out: bool,
    pub survivors: Vec<UnitId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnError {
    OffBoard(Hex),
    Occupied(Hex),
    /// A tile nobody can stand on is there.
    Blocked(Hex),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::OffBoard(h) => write!(f, "hex {:?} is off the board", h.to_offset()),
            SpawnError::Occupied(h) => write!(f, "hex {:?} is occupied", h.to_offset()),
            SpawnError::Blocked(h) => write!(f, "hex {:?} is blocked by a tile", h.to_offset()),
        }
    }
}

impl std::error::Error for SpawnError {}

struct PendingTrigger {
    owner: UnitId,
    passive: usize,
    other: Option<UnitId>,
}

/// A tile on the board.
#[derive(Clone, Debug)]
struct TileInstance {
    /// Tells this tile from every other laid in the fight.
    uid: u32,
    /// Which [`TileDef`], by position in the config.
    def: usize,
    pos: Hex,
    owner: Option<UnitId>,
    team: Option<Team>,
    remaining_ms: Option<u32>,
    /// Time since its last once-a-second effect.
    second_ms: u32,
}

/// A tile a unit is standing on and affected by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OnTile {
    uid: u32,
    def: usize,
    owner: Option<UnitId>,
}

/// An effect on its way: it lands on `target` at `at_tick`.
struct Delayed {
    at_tick: u32,
    source: UnitId,
    target: UnitId,
    other: Option<UnitId>,
    effect: Effect,
    direct: bool,
    /// Landings still to come after this one, and the ticks between them.
    more: u32,
    every_ticks: u32,
}

/// One fight, from setup to outcome.
pub struct Combat {
    board: HexBoard,
    config: CombatConfig,
    rng: Rng,
    resources: Arc<[ResourceDef]>,
    units: Vec<CombatUnit>,
    library: HashMap<String, Arc<UnitTemplate>>,
    occupancy: Vec<Option<UnitId>>,
    tick: u32,
    started: bool,
    events: Vec<Event>,
    pending: VecDeque<PendingTrigger>,
    delayed: Vec<Delayed>,
    tile_defs: Arc<[TileDef]>,
    tiles: Vec<TileInstance>,
    next_tile: u32,
    /// How many blocking tiles are on each hex.
    blocked: Vec<u16>,
    tile_depth: u32,
    /// What players have earned in this fight, by the side that earned it.
    rewards: Vec<(Team, crate::game::Grant)>,
    /// Stat changes units have earned for good.
    growth: Vec<(UnitId, Vec<StatMod>)>,
    outcome: Option<Outcome>,
    custom_effects: HashMap<String, Arc<CustomEffectFn>>,
    custom_conditions: HashMap<String, Arc<CustomConditionFn>>,
    custom_selectors: HashMap<String, Arc<CustomSelectorFn>>,
    hook_depth: u32,
}

impl Combat {
    pub fn new(board: HexBoard, seed: u64) -> Self {
        Combat::with_config(board, seed, CombatConfig::default())
    }

    pub fn with_config(board: HexBoard, seed: u64, config: CombatConfig) -> Self {
        assert!(config.tick_ms > 0, "tick_ms must be positive");
        let cells = board.len();
        let tile_defs: Arc<[TileDef]> = config.tiles.clone().into();
        Combat {
            board,
            resources: config.resources.clone().into(),
            config,
            rng: Rng::new(seed),
            units: Vec::new(),
            library: HashMap::new(),
            occupancy: vec![None; cells],
            tick: 0,
            started: false,
            events: Vec::new(),
            pending: VecDeque::new(),
            delayed: Vec::new(),
            tile_defs,
            tiles: Vec::new(),
            next_tile: 0,
            blocked: vec![0; cells],
            tile_depth: 0,
            rewards: Vec::new(),
            growth: Vec::new(),
            outcome: None,
            custom_effects: HashMap::new(),
            custom_conditions: HashMap::new(),
            custom_selectors: HashMap::new(),
            hook_depth: 0,
        }
    }

    // ----- inspection -----------------------------------------------------

    pub fn board(&self) -> &HexBoard {
        &self.board
    }
    pub fn config(&self) -> &CombatConfig {
        &self.config
    }
    pub fn tick(&self) -> u32 {
        self.tick
    }
    pub fn time_ms(&self) -> u32 {
        self.tick * self.config.tick_ms
    }
    pub fn units(&self) -> &[CombatUnit] {
        &self.units
    }
    pub fn unit(&self, id: UnitId) -> &CombatUnit {
        &self.units[id.0 as usize]
    }
    pub fn unit_at(&self, hex: Hex) -> Option<UnitId> {
        self.board.index(hex).and_then(|i| self.occupancy[i])
    }
    /// The units a selector would pick for `source` right now, for tools,
    /// tests and front ends that want to preview an ability.
    pub fn targets(&self, source: UnitId, selector: &Selector) -> Vec<UnitId> {
        self.select(selector, source, None)
    }

    /// What each side has earned so far, in the order it was earned (see
    /// [`Effect::reward`]). The game hands these to the players after the
    /// fight.
    pub fn rewards(&self) -> &[(Team, crate::game::Grant)] {
        &self.rewards
    }

    /// The stat changes units have earned for good so far (see
    /// [`Effect::grow`]), in order. The game writes these onto the
    /// players' units after the fight.
    pub fn growth(&self) -> &[(UnitId, Vec<StatMod>)] {
        &self.growth
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }
    /// Every event so far, in order.
    pub fn events(&self) -> &[Event] {
        &self.events
    }
    /// Removes and returns the events recorded since the last call. Useful
    /// for clients that render or forward events as the fight progresses.
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    // ----- setup and stepping ---------------------------------------------

    /// Makes a template available to `Effect::Summon` under the given id.
    pub fn register_template(&mut self, id: impl Into<String>, template: impl Into<Arc<UnitTemplate>>) {
        self.library.insert(id.into(), template.into());
    }

    /// Registers the hook run by `Effect::Custom { name, .. }`. Custom
    /// effects with no registered hook do nothing.
    pub fn register_custom_effect(
        &mut self,
        name: impl Into<String>,
        hook: impl Fn(&mut EffectContext<'_>) + Send + Sync + 'static,
    ) {
        self.custom_effects.insert(name.into(), Arc::new(hook));
    }

    /// Registers the predicate behind `Condition::Custom(name)`. Unregistered
    /// custom conditions are false.
    pub fn register_custom_condition(
        &mut self,
        name: impl Into<String>,
        predicate: impl Fn(&ConditionContext<'_>) -> bool + Send + Sync + 'static,
    ) {
        self.custom_conditions.insert(name.into(), Arc::new(predicate));
    }

    /// Registers the hook behind `Selector::Custom(name)`. Unregistered
    /// custom selectors pick nobody.
    pub fn register_custom_selector(
        &mut self,
        name: impl Into<String>,
        hook: impl Fn(&SelectContext<'_>) -> Vec<UnitId> + Send + Sync + 'static,
    ) {
        self.custom_selectors.insert(name.into(), Arc::new(hook));
    }

    /// Places a unit on the board.
    pub fn spawn(
        &mut self,
        template: impl Into<Arc<UnitTemplate>>,
        team: Team,
        pos: Hex,
    ) -> Result<UnitId, SpawnError> {
        let idx = self.board.index(pos).ok_or(SpawnError::OffBoard(pos))?;
        if self.occupancy[idx].is_some() {
            return Err(SpawnError::Occupied(pos));
        }
        if self.blocked[idx] > 0 {
            return Err(SpawnError::Blocked(pos));
        }
        let template = template.into();
        let id = UnitId(self.units.len() as u32);
        let stats = Stats::new(template.stats.clone());
        let health = stats.get(Stat::MaxHealth).max(1);
        let passive_state = vec![PassiveState::default(); template.passives.len()];
        let cooldowns = vec![0; template.abilities.len()];
        self.units.push(CombatUnit {
            id,
            team,
            template,
            pos,
            stats,
            health,
            resource_defs: self.resources.clone(),
            resources: vec![0; self.resources.len()],
            resource_locks: vec![0; self.resources.len()],
            alive: true,
            target: None,
            attack_progress: 0,
            move_progress: 0,
            cast_remaining_ms: None,
            casting: 0,
            cooldowns,
            second_ms: 0,
            died_at: None,
            on_tiles: Vec::new(),
            shields: Vec::new(),
            statuses: Vec::new(),
            passive_state,
            stacks: BTreeMap::new(),
        });
        self.occupancy[idx] = Some(id);
        let name = self.u(id).template.name.clone();
        self.emit(EventKind::Spawned { unit: id, team, pos, name, max_health: health });
        let defs = self.resources.clone();
        for (index, def) in defs.iter().enumerate() {
            let max = self.u(id).stat(def.max);
            if max > 0 {
                let start = self.eval(&def.start, id, id).clamp(0, max);
                self.set_resource(id, index, start, true);
            }
        }
        if self.started {
            self.sync_tiles(id);
            self.fire(id, None, |t| matches!(t, Trigger::CombatStart));
        }
        Ok(id)
    }

    /// Advances the fight by one tick. Returns the outcome once it is over.
    pub fn step(&mut self) -> Option<&Outcome> {
        if self.outcome.is_some() {
            return self.outcome.as_ref();
        }
        if !self.started {
            self.started = true;
            // Units standing on a tile the fight starts with step onto it now.
            for i in 0..self.units.len() {
                self.sync_tiles(UnitId(i as u32));
            }
            for i in 0..self.units.len() {
                self.fire(UnitId(i as u32), None, |t| matches!(t, Trigger::CombatStart));
            }
            self.drain_triggers();
        }
        self.tick += 1;
        let overtime = self.config.overtime_after_ms;
        if overtime > 0 && self.time_ms() > overtime && self.time_ms() - self.config.tick_ms <= overtime {
            self.emit(EventKind::Overtime);
        }
        self.land_delayed();
        self.tick_tiles();
        let acting = self.units.len();
        for i in 0..acting {
            if self.units[i].alive {
                self.update_unit(UnitId(i as u32));
                self.drain_triggers();
            }
        }
        self.check_end();
        self.outcome.as_ref()
    }

    /// Runs the fight to completion.
    pub fn run(&mut self) -> &Outcome {
        while self.outcome.is_none() {
            self.step();
        }
        self.outcome.as_ref().unwrap()
    }

    // ----- per-unit update ------------------------------------------------

    fn u(&self, id: UnitId) -> &CombatUnit {
        &self.units[id.0 as usize]
    }

    fn u_mut(&mut self, id: UnitId) -> &mut CombatUnit {
        &mut self.units[id.0 as usize]
    }

    fn emit(&mut self, kind: EventKind) {
        self.events.push(Event { tick: self.tick, kind });
    }

    fn update_unit(&mut self, id: UnitId) {
        self.tick_timers(id);
        if !self.u(id).alive || self.u(id).is_stunned() {
            return;
        }
        let tick_ms = self.config.tick_ms;

        // Finish or continue a cast in progress.
        if let Some(remaining) = self.u(id).cast_remaining_ms {
            let remaining = remaining.saturating_sub(tick_ms);
            if remaining == 0 {
                self.finish_cast(id);
            } else {
                self.u_mut(id).cast_remaining_ms = Some(remaining);
            }
            return;
        }

        // Start a cast as soon as the unit can pay for an ability: the
        // first one it can afford, in the order they are listed.
        let silenced = self.u(id).has_status(|s| matches!(s, StatusKind::Silence));
        let castable = (0..self.u(id).template.abilities.len()).find_map(|i| Some((i, self.payment(id, i)?)));
        if !silenced && let Some((which, payment)) = castable {
            let ability = &self.u(id).template.abilities[which];
            let (cast_time, cooldown) = (ability.cast_time_ms, ability.cooldown_ms);
            if let Some((index, amount)) = payment {
                let left = self.u(id).resources[index] - amount;
                self.set_resource(id, index, left, false);
                let lock = self.resources[index].lock_after_cast_ms;
                if lock > 0 {
                    self.u_mut(id).resource_locks[index] = cast_time + lock;
                }
            }
            let unit = self.u_mut(id);
            unit.cooldowns[which] = cooldown;
            unit.casting = which;
            self.emit(EventKind::CastStarted { unit: id, ability: which });
            if cast_time == 0 {
                self.finish_cast(id);
            } else {
                self.u_mut(id).cast_remaining_ms = Some(cast_time);
            }
            return;
        }

        let Some(target) = self.acquire_target(id) else {
            return;
        };
        let pos = self.u(id).pos;
        let target_pos = self.u(target).pos;
        let range = self.u(id).stat(Stat::Range).max(1) as u32;

        if pos.distance(target_pos) <= range {
            if self.u(id).has_status(|s| matches!(s, StatusKind::Disarm)) {
                return;
            }
            let speed = self.u(id).stat(Stat::AttackSpeed);
            self.u_mut(id).attack_progress += speed * tick_ms as i64;
            while self.u(id).attack_progress >= PROGRESS_FULL {
                if !self.u(id).alive || !self.u(target).alive {
                    let unit = self.u_mut(id);
                    unit.attack_progress = unit.attack_progress.min(PROGRESS_FULL);
                    break;
                }
                self.u_mut(id).attack_progress -= PROGRESS_FULL;
                self.basic_attack(id, target);
            }
        } else {
            if self.u(id).has_status(|s| matches!(s, StatusKind::Root)) {
                return;
            }
            let speed = self.u(id).stat(Stat::MoveSpeed);
            self.u_mut(id).move_progress += speed * tick_ms as i64;
            if self.u(id).move_progress >= PROGRESS_FULL {
                let board = &self.board;
                let (occupancy, blocked) = (&self.occupancy, &self.blocked);
                let free = |h: Hex| board.index(h).is_some_and(|i| occupancy[i].is_none() && blocked[i] == 0);
                let path = board.find_path(pos, |h| h.distance(target_pos) <= range && free(h), free);
                match path.and_then(|p| p.first().copied()) {
                    Some(next) => {
                        self.u_mut(id).move_progress -= PROGRESS_FULL;
                        self.move_unit(id, next);
                    }
                    // Blocked: stay ready to step as soon as a path opens.
                    None => self.u_mut(id).move_progress = PROGRESS_FULL,
                }
            }
        }
    }

    /// What casting one of its abilities would cost a unit right now:
    /// `None` if it cannot cast it, `Some(None)` if the cast is free, or the
    /// resource (by position) and amount to take.
    fn payment(&self, id: UnitId, ability: usize) -> Option<Option<(usize, i64)>> {
        let unit = self.u(id);
        if unit.cooldowns[ability] > 0 {
            return None;
        }
        let ability = &unit.template.abilities[ability];
        let (resource, amount) = match &ability.cost {
            Cost::Free => return Some(None),
            Cost::Full(resource) => (resource, None),
            Cost::Amount { resource, amount } => (resource, Some((*amount).max(0))),
        };
        let index = self.resources.iter().position(|d| d.id == *resource)?;
        let max = unit.stat(self.resources[index].max);
        let amount = amount.unwrap_or(max);
        (max > 0 && unit.resources[index] >= amount).then_some(Some((index, amount)))
    }

    /// Sets how much of a resource a unit has, and reports it if that is
    /// news (or `always`).
    fn set_resource(&mut self, id: UnitId, index: usize, amount: i64, always: bool) {
        let max = self.u(id).stat(self.resources[index].max).max(0);
        let amount = amount.clamp(0, max);
        if std::mem::replace(&mut self.u_mut(id).resources[index], amount) != amount || always {
            let resource = self.resources[index].id.clone();
            self.emit(EventKind::ResourceChanged { unit: id, resource, amount, max });
        }
    }

    /// Gives a unit some of a resource (or takes it, when negative), within
    /// the resource's rules.
    fn add_resource(&mut self, id: UnitId, index: usize, amount: i64) {
        let def = &self.resources[index];
        let unit = self.u(id);
        let locked = (unit.cast_remaining_ms.is_some() && !def.changes_while_casting) || (amount > 0 && unit.resource_locks[index] > 0);
        if amount == 0 || !unit.alive || locked || unit.stat(def.max) <= 0 {
            return;
        }
        self.set_resource(id, index, unit.resources[index] + amount, false);
    }

    /// Gives a unit what each resource's rules say an occasion is worth.
    fn gain_resources(&mut self, id: UnitId, worth: impl Fn(&Combat, &ResourceDef) -> i64) {
        for index in 0..self.resources.len() {
            let amount = worth(self, &self.resources[index]);
            self.add_resource(id, index, amount);
        }
    }

    /// Advances shields, statuses, resources and periodic passives by one
    /// tick.
    fn tick_timers(&mut self, id: UnitId) {
        let tick_ms = self.config.tick_ms;

        // Cooldown, and what resources gain or lose every second.
        let unit = self.u_mut(id);
        for cooldown in &mut unit.cooldowns {
            *cooldown = cooldown.saturating_sub(tick_ms);
        }
        for lock in &mut unit.resource_locks {
            *lock = lock.saturating_sub(tick_ms);
        }
        unit.second_ms += tick_ms;
        if unit.second_ms >= 1000 {
            unit.second_ms -= 1000;
            self.gain_resources(id, |c, def| if def.per_second.is_zero() { 0 } else { c.eval(&def.per_second, id, id) });
        }

        // Shields.
        self.u_mut(id).shields.retain_mut(|s| match &mut s.remaining_ms {
            Some(ms) => {
                *ms = ms.saturating_sub(tick_ms);
                *ms > 0
            }
            None => true,
        });

        // Statuses: apply damage over time, then expire.
        let dots: Vec<(Amount, DamageKind, UnitId)> = self
            .u(id)
            .statuses
            .iter()
            .filter_map(|s| match &s.kind {
                StatusKind::DamageOverTime { per_second, kind } => {
                    Some((per_second.clone(), *kind, s.source))
                }
                _ => None,
            })
            .collect();
        for (per_second, kind, source) in dots {
            let raw = self.eval(&per_second, source, id) * tick_ms as i64 / 1000;
            self.deal_damage(source, id, raw, kind, false, false);
            if !self.u(id).alive {
                return;
            }
        }
        let mut expired = Vec::new();
        self.u_mut(id).statuses.retain_mut(|s| {
            s.remaining_ms = s.remaining_ms.saturating_sub(tick_ms);
            if s.remaining_ms == 0 {
                expired.push(s.kind.clone());
                false
            } else {
                true
            }
        });
        for kind in expired {
            if let StatusKind::StatMods(mods) = &kind {
                self.change_stats(id, mods, false);
            }
            self.emit(EventKind::StatusExpired { target: id, status: kind });
        }

        // Periodic passives.
        let template = self.u(id).template.clone();
        for (i, passive) in template.passives.iter().enumerate() {
            if let Trigger::Every { interval_ms } = passive.trigger {
                let state = &mut self.u_mut(id).passive_state[i];
                state.timer_ms += tick_ms;
                if interval_ms > 0 && state.timer_ms >= interval_ms {
                    state.timer_ms -= interval_ms;
                    self.pending.push_back(PendingTrigger { owner: id, passive: i, other: None });
                }
            }
        }
    }

    fn acquire_target(&mut self, id: UnitId) -> Option<UnitId> {
        // The latest taunt from a living enemy decides.
        let taunter = self.u(id).statuses.iter().rev().find_map(|s| {
            let alive_enemy = self.u(s.source).alive && self.u(s.source).team != self.u(id).team;
            (matches!(s.kind, StatusKind::Taunt) && alive_enemy).then_some(s.source)
        });
        if let Some(t) = taunter {
            self.u_mut(id).target = Some(t);
            return Some(t);
        }
        if let Some(t) = self.u(id).target
            && self.u(t).is_targetable()
        {
            return Some(t);
        }
        let me = self.u(id);
        let enemies = self.units.iter().filter(|o| o.is_targetable() && o.team != me.team);
        // Ties go to the nearest, then the lowest id.
        let near = |o: &CombatUnit| (me.pos.distance(o.pos), o.id);
        let target = match me.template.targeting {
            Targeting::Nearest => enemies.min_by_key(|o| near(o)),
            Targeting::Farthest => enemies.min_by_key(|o| (std::cmp::Reverse(me.pos.distance(o.pos)), o.id)),
            Targeting::LowestHealth => enemies.min_by_key(|o| (o.health, near(o))),
            Targeting::HighestDamage => enemies.min_by_key(|o| (std::cmp::Reverse(o.stat(Stat::AttackDamage)), near(o))),
            Targeting::LowestArmor => enemies.min_by_key(|o| (o.stat(Stat::Armor), near(o))),
            Targeting::HighestStat(stat) => enemies.min_by_key(|o| (std::cmp::Reverse(o.stat(stat)), near(o))),
            Targeting::LowestStat(stat) => enemies.min_by_key(|o| (o.stat(stat), near(o))),
            Targeting::Custom(name) => {
                let picked = self.custom_selectors.get(name.as_str()).and_then(|hook| {
                    hook(&SelectContext { combat: self, source: id, other: None })
                        .into_iter()
                        .find(|t| self.units.get(t.0 as usize).is_some_and(|o| o.is_targetable() && o.team != me.team))
                });
                match picked {
                    Some(t) => Some(self.u(t)),
                    None => enemies.min_by_key(|o| near(o)),
                }
            }
        }
        .map(|o| o.id);
        self.u_mut(id).target = target;
        target
    }

    /// Moves a unit to a free hex and returns where it came from.
    fn place(&mut self, id: UnitId, to: Hex) -> Hex {
        let from = self.u(id).pos;
        let from_idx = self.board.index(from).unwrap();
        let to_idx = self.board.index(to).unwrap();
        debug_assert!(self.occupancy[to_idx].is_none());
        self.occupancy[from_idx] = None;
        self.occupancy[to_idx] = Some(id);
        self.u_mut(id).pos = to;
        from
    }

    fn move_unit(&mut self, id: UnitId, to: Hex) {
        let from = self.place(id, to);
        self.emit(EventKind::Moved { unit: id, from, to });
        self.sync_tiles(id);
    }

    fn displace(&mut self, id: UnitId, to: Hex) {
        if self.u(id).pos != to {
            let from = self.place(id, to);
            self.emit(EventKind::Displaced { unit: id, from, to });
            self.sync_tiles(id);
        }
    }

    /// Whether a unit could stand on a hex: on the board, empty and not
    /// blocked by a tile.
    fn is_free(&self, hex: Hex) -> bool {
        self.board.index(hex).is_some_and(|i| self.occupancy[i].is_none() && self.blocked[i] == 0)
    }

    // ----- tiles ----------------------------------------------------------

    /// Lays a tile on a hex, for `duration_ms` or for the rest of the fight.
    /// `owner` is the unit that laid it: its side decides whom the tile
    /// affects, and it is the source of the tile's effects. Laying a tile
    /// where the same kind already lies keeps the longer time and hands it
    /// to the new owner.
    ///
    /// Returns `false` if nothing was laid: an unknown tile, a hex off the
    /// board, or a blocking tile on a hex a unit is standing on.
    pub fn place_tile(&mut self, tile: &str, pos: Hex, owner: Option<UnitId>, duration_ms: Option<u32>) -> bool {
        let Some(def) = self.tile_defs.iter().position(|d| d.id == tile) else {
            return false;
        };
        let Some(idx) = self.board.index(pos) else {
            return false;
        };
        let team = owner.map(|o| self.u(o).team);
        let remaining_ms = match self.tiles.iter_mut().find(|t| t.def == def && t.pos == pos) {
            Some(existing) => {
                existing.remaining_ms = match (existing.remaining_ms, duration_ms) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    _ => None,
                };
                existing.owner = owner;
                existing.team = team;
                existing.remaining_ms
            }
            None => {
                let blocks = self.tile_defs[def].blocks;
                if blocks && self.occupancy[idx].is_some() {
                    return false;
                }
                let uid = self.next_tile;
                self.next_tile += 1;
                self.tiles.push(TileInstance { uid, def, pos, owner, team, remaining_ms: duration_ms, second_ms: 0 });
                if blocks {
                    self.blocked[idx] += 1;
                }
                duration_ms
            }
        };
        self.emit(EventKind::TilePlaced { tile: tile.to_string(), pos, team, duration_ms: remaining_ms });
        if let Some(unit) = self.occupancy[idx] {
            self.sync_tiles(unit);
        }
        true
    }

    /// Takes a kind of tile off a hex. Returns whether one was there.
    pub fn clear_tile(&mut self, tile: &str, pos: Hex) -> bool {
        match self.tiles.iter().find(|t| t.pos == pos && self.tile_defs[t.def].id == tile).map(|t| t.uid) {
            Some(uid) => {
                self.remove_tile(uid);
                true
            }
            None => false,
        }
    }

    /// The tiles on the board right now, in the order they were laid.
    pub fn tiles(&self) -> Vec<TileView<'_>> {
        self.tiles
            .iter()
            .map(|t| TileView { def: &self.tile_defs[t.def], pos: t.pos, team: t.team, remaining_ms: t.remaining_ms })
            .collect()
    }

    /// Whether a hex has a tile of this kind on it.
    pub fn has_tile(&self, pos: Hex, tile: &str) -> bool {
        self.tiles.iter().any(|t| t.pos == pos && self.tile_defs[t.def].id == tile)
    }

    /// Whether a unit could stand on a hex right now.
    pub fn can_stand(&self, hex: Hex) -> bool {
        self.is_free(hex)
    }

    fn remove_tile(&mut self, uid: u32) {
        let Some(at) = self.tiles.iter().position(|t| t.uid == uid) else {
            return;
        };
        let tile = self.tiles.remove(at);
        let idx = self.board.index(tile.pos).expect("tiles are on the board");
        if self.tile_defs[tile.def].blocks {
            self.blocked[idx] -= 1;
        }
        self.emit(EventKind::TileRemoved { tile: self.tile_defs[tile.def].id.clone(), pos: tile.pos });
        if let Some(unit) = self.occupancy[idx] {
            self.sync_tiles(unit);
        }
    }

    fn tile_affects(&self, def: usize, tile_team: Option<Team>, unit_team: Team) -> bool {
        match (self.tile_defs[def].affects, tile_team) {
            (Affects::Everyone, _) | (_, None) => true,
            (Affects::Allies, Some(team)) => team == unit_team,
            (Affects::Enemies, Some(team)) => team != unit_team,
        }
    }

    /// Brings a unit in line with the tiles under it after anything that
    /// could change them: it moved, died or came back, or a tile appeared,
    /// changed hands or went away. Stat changes follow the tiles exactly;
    /// enter effects run for tiles that are new to it, and leave effects
    /// for tiles it walked off (not ones that ran out, and not on death).
    fn sync_tiles(&mut self, id: UnitId) {
        if !self.started || (self.tiles.is_empty() && self.u(id).on_tiles.is_empty()) {
            return;
        }
        let unit = self.u(id);
        let now: Vec<OnTile> = if unit.alive {
            self.tiles
                .iter()
                .filter(|t| t.pos == unit.pos && self.tile_affects(t.def, t.team, unit.team))
                .map(|t| OnTile { uid: t.uid, def: t.def, owner: t.owner })
                .collect()
        } else {
            Vec::new()
        };
        let before = unit.on_tiles.clone();
        let left: Vec<OnTile> = before.iter().filter(|b| !now.iter().any(|n| n.uid == b.uid)).copied().collect();
        let entered: Vec<OnTile> = now.iter().filter(|n| !before.iter().any(|b| b.uid == n.uid)).copied().collect();
        if left.is_empty() && entered.is_empty() {
            return;
        }
        self.u_mut(id).on_tiles = now;
        let defs = self.tile_defs.clone();
        // Effects of one tile can push a unit onto the next; that has to
        // stop somewhere.
        let run_effects = self.tile_depth < MAX_TILE_CHAIN;
        self.tile_depth += 1;
        for tile in left {
            self.change_stats(id, &defs[tile.def].mods, false);
            let still_there = self.tiles.iter().any(|t| t.uid == tile.uid);
            if run_effects && still_there && self.u(id).alive {
                for effect in &defs[tile.def].on_leave {
                    self.apply_effect(tile.owner.unwrap_or(id), id, None, effect, false);
                }
            }
        }
        for tile in entered {
            self.change_stats(id, &defs[tile.def].mods, true);
            if run_effects {
                for effect in &defs[tile.def].on_enter {
                    self.apply_effect(tile.owner.unwrap_or(id), id, None, effect, false);
                }
            }
        }
        self.tile_depth -= 1;
    }

    /// Runs out the tiles that were laid for a time, and once a second
    /// does what each tile does to the unit standing on it.
    fn tick_tiles(&mut self) {
        if self.tiles.is_empty() {
            return;
        }
        let tick_ms = self.config.tick_ms;
        let defs = self.tile_defs.clone();
        let due: Vec<u32> = self
            .tiles
            .iter_mut()
            .filter_map(|tile| {
                tile.second_ms += tick_ms;
                (tile.second_ms >= 1000).then(|| {
                    tile.second_ms -= 1000;
                    tile.uid
                })
            })
            .collect();
        for uid in due {
            // An earlier tile's effects may have changed who stands where.
            let Some(tile) = self.tiles.iter().find(|t| t.uid == uid).cloned() else {
                continue;
            };
            if defs[tile.def].per_second.is_empty() {
                continue;
            }
            let Some(unit) = self.unit_at(tile.pos) else {
                continue;
            };
            if !self.tile_affects(tile.def, tile.team, self.u(unit).team) {
                continue;
            }
            for effect in &defs[tile.def].per_second {
                self.apply_effect(tile.owner.unwrap_or(unit), unit, None, effect, false);
            }
            self.drain_triggers();
        }
        // A tile laid for two seconds gets its second pulse before it goes.
        let mut expired = Vec::new();
        for tile in &mut self.tiles {
            if let Some(remaining) = &mut tile.remaining_ms {
                *remaining = remaining.saturating_sub(tick_ms);
                if *remaining == 0 {
                    expired.push(tile.uid);
                }
            }
        }
        for uid in expired {
            self.remove_tile(uid);
        }
    }

    /// Living units on `id`'s team (itself included) that carry `tag`.
    fn allies_with_tag(&self, id: UnitId, tag: &str) -> u32 {
        let team = self.u(id).team;
        self.units.iter().filter(|o| o.alive && o.team == team && o.has_tag(tag)).count() as u32
    }

    fn basic_attack(&mut self, id: UnitId, target: UnitId) {
        self.emit(EventKind::Attacked { source: id, target });
        let damage = self.u(id).stat(Stat::AttackDamage);
        self.gain_resources(id, |c, def| if def.per_attack.is_zero() { 0 } else { c.eval(&def.per_attack, id, id) });
        let dodge = self.u(target).stat(Stat::Dodge);
        if dodge > 0 && self.rng.chance(dodge.min(1000) as u64, 1000) {
            self.emit(EventKind::Dodged { source: id, target });
            self.fire(target, Some(id), |t| matches!(t, Trigger::OnDodge));
            return;
        }
        let kind = self.u(id).template.attack_kind;
        self.deal_damage(id, target, damage, kind, true, true);
        self.fire(id, Some(target), |t| matches!(t, Trigger::OnAttack));
    }

    fn finish_cast(&mut self, id: UnitId) {
        self.u_mut(id).cast_remaining_ms = None;
        let which = self.u(id).casting;
        self.emit(EventKind::CastFinished { unit: id, ability: which });
        let template = self.u(id).template.clone();
        if let Some(ability) = template.abilities.get(which) {
            self.resolve(id, None, &ability.effects, true);
        }
        self.fire(id, None, |t| matches!(t, Trigger::OnCast));
        let team = self.u(id).team;
        for i in 0..self.units.len() {
            let ally = &self.units[i];
            if ally.alive && ally.team == team && ally.id != id {
                self.fire(ally.id, Some(id), |t| matches!(t, Trigger::OnAllyCast));
            }
        }
    }

    // ----- triggers -------------------------------------------------------

    /// Queues every passive of `owner` whose trigger matches.
    fn fire(&mut self, owner: UnitId, other: Option<UnitId>, matches: impl Fn(&Trigger) -> bool) {
        let template = self.u(owner).template.clone();
        for (i, passive) in template.passives.iter().enumerate() {
            if matches(&passive.trigger) {
                self.pending.push_back(PendingTrigger { owner, passive: i, other });
            }
        }
    }

    fn drain_triggers(&mut self) {
        let mut resolved = 0;
        while let Some(p) = self.pending.pop_front() {
            resolved += 1;
            if resolved > MAX_TRIGGER_CHAIN {
                self.pending.clear();
                break;
            }
            let template = self.u(p.owner).template.clone();
            let passive = &template.passives[p.passive];
            // Dead units only get to resolve their death triggers.
            if !self.u(p.owner).alive && passive.trigger != Trigger::OnDeath {
                continue;
            }
            let limit = match passive.trigger {
                // A health threshold fires once unless told otherwise.
                Trigger::HealthBelow { .. } => Some(passive.max_procs.unwrap_or(1)),
                _ => passive.max_procs,
            };
            if limit.is_some_and(|max| self.u(p.owner).passive_state[p.passive].procs >= max) {
                continue;
            }
            if let Some(condition) = &passive.condition
                && !self.check(condition, p.owner, p.other)
            {
                continue;
            }
            self.u_mut(p.owner).passive_state[p.passive].procs += 1;
            self.emit(EventKind::PassiveTriggered { unit: p.owner, passive: p.passive });
            // Effects from passives do not themselves count as attacks or
            // ability hits, which keeps reactive passives from ping-ponging.
            self.resolve(p.owner, p.other, &passive.effects, false);
        }
    }

    // ----- effect resolution ----------------------------------------------

    fn resolve(&mut self, source: UnitId, other: Option<UnitId>, effects: &[EffectSpec], direct: bool) {
        let tick_ms = self.config.tick_ms;
        // A wait is at least one tick, so a delayed effect never lands in
        // the middle of the tick that started it.
        let ticks = |ms: u32| ms.div_ceil(tick_ms).max(1);
        for spec in effects {
            for target in self.select(&spec.target, source, other) {
                if let Some(condition) = &spec.condition
                    && !self.check(condition, source, Some(target))
                {
                    continue;
                }
                let (times, every_ticks) = spec.repeat.map_or((1, 0), |r| (r.times.max(1), ticks(r.every_ms)));
                // Something that flies takes longer to reach the far ones.
                let flight = spec.travel_ms_per_hex * self.u(source).pos.distance(self.u(target).pos);
                let delay_ms = spec.delay_ms + flight;
                if delay_ms == 0 {
                    self.apply_effect(source, target, other, &spec.effect, direct);
                    if times > 1 {
                        self.delayed.push(Delayed {
                            at_tick: self.tick + every_ticks,
                            source,
                            target,
                            other,
                            effect: spec.effect.clone(),
                            direct,
                            more: times - 2,
                            every_ticks,
                        });
                    }
                } else {
                    self.delayed.push(Delayed {
                        at_tick: self.tick + ticks(delay_ms),
                        source,
                        target,
                        other,
                        effect: spec.effect.clone(),
                        direct,
                        more: times - 1,
                        every_ticks,
                    });
                }
            }
        }
    }

    /// Lands the delayed effects that are due, in the order they were
    /// started, and queues the next landing of the ones that repeat.
    fn land_delayed(&mut self) {
        if self.delayed.is_empty() {
            return;
        }
        let tick = self.tick;
        let (due, waiting): (Vec<Delayed>, Vec<Delayed>) = std::mem::take(&mut self.delayed).into_iter().partition(|d| d.at_tick <= tick);
        self.delayed = waiting;
        for mut d in due {
            self.apply_effect(d.source, d.target, d.other, &d.effect, d.direct);
            self.drain_triggers();
            if d.more > 0 {
                d.more -= 1;
                d.at_tick = tick + d.every_ticks;
                self.delayed.push(d);
            }
        }
    }

    /// Evaluates a condition. `target` is `None` when there is no other unit
    /// (a passive triggered without one); conditions about it are false.
    fn check(&mut self, condition: &Condition, source: UnitId, target: Option<UnitId>) -> bool {
        let who = |w: &Who| match w {
            Who::Source => Some(source),
            Who::Target => target,
        };
        let health_permille = |u: &CombatUnit| u.health * 1000 / u.stat(Stat::MaxHealth).max(1);
        match condition {
            Condition::HealthBelow { who: w, permille } => {
                who(w).is_some_and(|id| health_permille(self.u(id)) <= *permille as i64)
            }
            Condition::HealthAbove { who: w, permille } => {
                who(w).is_some_and(|id| health_permille(self.u(id)) > *permille as i64)
            }
            Condition::Stunned(w) => who(w).is_some_and(|id| self.u(id).is_stunned()),
            Condition::HasDebuff(w) => who(w).is_some_and(|id| self.u(id).has_status(StatusKind::is_debuff)),
            Condition::Shielded(w) => who(w).is_some_and(|id| self.u(id).shield() > 0),
            Condition::HasTag { who: w, tag } => who(w).is_some_and(|id| self.u(id).has_tag(tag)),
            Condition::StacksAtLeast { who: w, stack, count } => {
                who(w).is_some_and(|id| self.u(id).stacks(stack) >= *count)
            }
            Condition::AlliesWithTagAtLeast { tag, count } => self.allies_with_tag(source, tag) >= *count,
            Condition::ResourceAtLeast { who: w, resource, permille } => who(w)
                .and_then(|id| self.u(id).pool(resource))
                .is_some_and(|p| p.amount * 1000 >= p.max * *permille as i64),
            Condition::OnTile { who: w, tile } => who(w).is_some_and(|id| self.u(id).alive && self.has_tile(self.u(id).pos, tile)),
            Condition::InFrontRows { who: w, rows } => who(w).is_some_and(|id| {
                // The first side holds the lower half of the rows (as its
                // player sees the board), the second side the upper half.
                let middle = self.board.rows() as i32 / 2;
                let row = self.u(id).pos.to_offset().1;
                let depth = if self.u(id).team.0 % 2 == 0 { row - middle } else { middle - 1 - row };
                depth < *rows as i32
            }),
            Condition::HasStatus { who: w, status } => {
                who(w).is_some_and(|id| self.u(id).has_status(|s| s.name() == status))
            }
            Condition::Chance { permille } => self.rng.chance(*permille as u64, 1000),
            Condition::Not(c) => !self.check(c, source, target),
            Condition::All(cs) => cs.iter().all(|c| self.check(c, source, target)),
            Condition::Any(cs) => cs.iter().any(|c| self.check(c, source, target)),
            Condition::Custom(name) => match self.custom_conditions.get(name).cloned() {
                Some(predicate) => predicate(&ConditionContext { combat: self, source, target }),
                None => false,
            },
        }
    }

    fn select(&self, selector: &Selector, source: UnitId, other: Option<UnitId>) -> Vec<UnitId> {
        let me = self.u(source);
        let enemies = || self.units.iter().filter(|o| o.is_targetable() && o.team != me.team);
        let allies = || self.units.iter().filter(|o| o.alive && o.team == me.team);
        let nearest_enemy = || enemies().min_by_key(|o| (me.pos.distance(o.pos), o.id));
        let current_target = || {
            me.target
                .map(|t| self.u(t))
                .filter(|t| t.is_targetable() || (t.alive && t.team == me.team))
                .or_else(nearest_enemy)
        };
        let anchor_pos = |anchor: &Anchor| match anchor {
            Anchor::Source => Some(me.pos),
            Anchor::CurrentTarget => current_target().map(|t| t.pos),
        };
        fn take_sorted<'a, K: Ord>(
            units: impl Iterator<Item = &'a CombatUnit>,
            n: u32,
            key: impl Fn(&CombatUnit) -> K,
        ) -> Vec<UnitId> {
            let mut v: Vec<&CombatUnit> = units.collect();
            v.sort_by_key(|o| (key(o), o.id));
            v.into_iter().take(n as usize).map(|o| o.id).collect()
        }
        let ids = |it: &mut dyn Iterator<Item = &CombatUnit>| it.map(|o| o.id).collect::<Vec<_>>();

        match selector {
            Selector::Source => vec![source],
            Selector::CurrentTarget => current_target().map(|t| t.id).into_iter().collect(),
            Selector::EventOther => other.into_iter().collect(),
            Selector::NearestEnemies(n) => take_sorted(enemies(), *n, |o| me.pos.distance(o.pos)),
            Selector::FarthestEnemies(n) => {
                take_sorted(enemies(), *n, |o| std::cmp::Reverse(me.pos.distance(o.pos)))
            }
            Selector::LowestHealthEnemies(n) => take_sorted(enemies(), *n, |o| o.health),
            Selector::LowestHealthAllies(n) => take_sorted(allies(), *n, |o| o.health),
            Selector::AllEnemies => ids(&mut enemies()),
            Selector::AllAllies => ids(&mut allies()),
            Selector::EnemiesWithin { anchor, radius } => match anchor_pos(anchor) {
                Some(c) => ids(&mut enemies().filter(|o| c.distance(o.pos) <= *radius)),
                None => Vec::new(),
            },
            Selector::AlliesWithin { anchor, radius } => match anchor_pos(anchor) {
                Some(c) => ids(&mut allies().filter(|o| c.distance(o.pos) <= *radius)),
                None => Vec::new(),
            },
            Selector::AlliesWithTag(tag) => {
                ids(&mut allies().filter(|o| o.template.tags.iter().any(|t| t == tag)))
            }
            Selector::RandomEnemies(n) => {
                // The fight's dice, forked so choosing does not move them.
                let mut dice = self.rng.clone().fork((self.tick as u64) << 16 | source.0 as u64);
                let mut pool = ids(&mut enemies());
                dice.shuffle(&mut pool);
                pool.truncate(*n as usize);
                pool
            }
            Selector::HighestStatEnemies { stat, n } => {
                take_sorted(enemies(), *n, |o| std::cmp::Reverse(o.stat(*stat)))
            }
            Selector::HighestStatAllies { stat, n } => {
                take_sorted(allies(), *n, |o| std::cmp::Reverse(o.stat(*stat)))
            }
            Selector::LowestStatEnemies { stat, n } => take_sorted(enemies(), *n, |o| o.stat(*stat)),
            Selector::LowestStatAllies { stat, n } => take_sorted(allies(), *n, |o| o.stat(*stat)),
            Selector::RandomAllies(n) => {
                let mut dice = self.rng.clone().fork((self.tick as u64) << 16 | source.0 as u64);
                let mut pool = ids(&mut allies());
                dice.shuffle(&mut pool);
                pool.truncate(*n as usize);
                pool
            }
            Selector::OtherAllies => ids(&mut allies().filter(|o| o.id != source)),
            Selector::FallenAllies(n) => {
                // The most recently fallen first.
                let fallen = self.units.iter().filter(|o| !o.alive && o.team == me.team && o.id != source);
                take_sorted(fallen, *n, |o| std::cmp::Reverse(o.died_at.unwrap_or(0)))
            }
            Selector::EnemiesInLine { length } => {
                let Some(aim) = current_target().map(|t| t.pos).filter(|p| *p != me.pos) else { return Vec::new() };
                // The line from the source through its target, carried on.
                let dist = me.pos.distance(aim);
                let reach = length.div_ceil(dist).max(1) as i32;
                let line = me.pos.line_to(me.pos.add(aim.sub(me.pos).scale(reach)));
                let mut hit = Vec::new();
                for hex in line.into_iter().skip(1).take(*length as usize) {
                    if let Some(o) = self.unit_at(hex).map(|id| self.u(id)).filter(|o| o.is_targetable() && o.team != me.team) {
                        hit.push(o.id);
                    }
                }
                hit
            }
            Selector::EnemiesInCone { length } => {
                let Some(aim) = current_target().map(|t| t.pos).filter(|p| *p != me.pos) else { return Vec::new() };
                // Hex steps as flat vectors, scaled so the numbers stay whole:
                // x = 2q + r, y = r * sqrt(3). The cone is 30 degrees to each
                // side of the aim, so cos^2 >= 3/4.
                let flat = |h: Hex| ((2 * h.q + h.r) as i64, h.r as i64);
                let dot = |a: (i64, i64), b: (i64, i64)| a.0 * b.0 + 3 * a.1 * b.1;
                let to_aim = flat(aim.sub(me.pos));
                take_sorted(
                    enemies().filter(|o| {
                        let to = flat(o.pos.sub(me.pos));
                        let d = dot(to, to_aim);
                        me.pos.distance(o.pos) <= *length && d > 0 && 4 * d * d >= 3 * dot(to, to) * dot(to_aim, to_aim)
                    }),
                    u32::MAX,
                    |o| me.pos.distance(o.pos),
                )
            }
            Selector::AlliesInRow => {
                let row = me.pos.to_offset().1;
                ids(&mut allies().filter(|o| o.pos.to_offset().1 == row))
            }
            Selector::EnemyCluster { radius } => {
                let around = |c: &CombatUnit| enemies().filter(|o| c.pos.distance(o.pos) <= *radius).count();
                let center = enemies().min_by_key(|c| (std::cmp::Reverse(around(c)), me.pos.distance(c.pos), c.id));
                match center {
                    Some(c) => ids(&mut enemies().filter(|o| c.pos.distance(o.pos) <= *radius)),
                    None => Vec::new(),
                }
            }
            Selector::Chain { count, range } => {
                let mut chain: Vec<UnitId> = Vec::new();
                let mut last = current_target().filter(|t| t.team != me.team);
                while let Some(at) = last {
                    if chain.len() as u32 >= *count {
                        break;
                    }
                    chain.push(at.id);
                    last = enemies()
                        .filter(|o| !chain.contains(&o.id) && at.pos.distance(o.pos) <= *range)
                        .min_by_key(|o| (at.pos.distance(o.pos), o.id));
                }
                chain
            }
            Selector::Custom(name) => match self.custom_selectors.get(name) {
                Some(hook) => {
                    let mut picked = hook(&SelectContext { combat: self, source, other });
                    picked.retain(|id| self.units.get(id.0 as usize).is_some_and(|u| u.alive));
                    picked
                }
                None => Vec::new(),
            },
        }
    }

    fn eval(&self, amount: &Amount, source: UnitId, target: UnitId) -> i64 {
        let mut total = amount.flat;
        for s in &amount.scalings {
            let unit = match s.who {
                Who::Source => self.u(source),
                Who::Target => self.u(target),
            };
            let value = match &s.by {
                ScaleBy::Stat(stat) => unit.stat(*stat),
                ScaleBy::CurrentHealth => unit.health.max(0),
                ScaleBy::MissingHealth => (unit.stat(Stat::MaxHealth) - unit.health).max(0),
                ScaleBy::Stacks(stack) => unit.stacks(stack) as i64,
                ScaleBy::AlliesWithTag(tag) => self.allies_with_tag(unit.id, tag) as i64,
                ScaleBy::Resource(resource) => unit.resource(resource),
            };
            total += value * s.permille / 1000;
        }
        if amount.power_scaled {
            total = total * self.u(source).stat(Stat::AbilityPower) / 100;
        }
        total
    }

    fn apply_effect(&mut self, source: UnitId, target: UnitId, other: Option<UnitId>, effect: &Effect, direct: bool) {
        // Summons may be anchored on a unit that just died, and custom hooks
        // decide for themselves; everything else needs a living target.
        if !self.u(target).alive
            && !matches!(
                effect,
                Effect::Summon { .. } | Effect::Custom { .. } | Effect::Revive { .. } | Effect::PlaceTile { .. } | Effect::Reward { .. }
            )
        {
            return;
        }
        match effect {
            Effect::Reward { grant } => {
                let team = self.u(target).team;
                self.rewards.push((team, (**grant).clone()));
                self.emit(EventKind::Rewarded { unit: target, team, grant: (**grant).clone() });
            }
            Effect::Grow { mods } => {
                self.change_stats(target, mods, true);
                self.growth.push((target, mods.clone()));
                self.emit(EventKind::Grew { unit: target, mods: mods.clone() });
            }
            Effect::Transform { unit } => {
                let Some(form) = self.library.get(unit).cloned() else {
                    return;
                };
                let old = self.u(target).template.clone();
                if old.id == *unit {
                    return;
                }
                // What the unit was before items and traits, to tell what
                // they added.
                let plain = self.library.get(&old.id).cloned();
                let plain_stats = plain.as_ref().map_or(&old.stats, |p| &p.stats);
                let own_passives = plain.as_ref().map_or(old.passives.len(), |p| p.passives.len().min(old.passives.len()));
                let mut mods: Vec<StatMod> = Stat::ALL
                    .iter()
                    .copied()
                    .chain(form.stats.custom().map(|(s, _)| s))
                    .chain(plain_stats.custom().map(|(s, _)| s))
                    .map(|s| StatMod::flat(s, form.stats.get(s) - plain_stats.get(s)))
                    .filter(|m| m.flat != 0)
                    .collect();
                mods.sort_by_key(|m| m.stat);
                mods.dedup_by_key(|m| m.stat);
                let mut template = (*form).clone();
                template.id = unit.clone();
                template.passives.extend(old.passives[own_passives..].iter().cloned());
                template.tags = old.tags.clone();
                let (health, most) = (self.u(target).health, self.u(target).stat(Stat::MaxHealth).max(1));
                let changing = self.u_mut(target);
                changing.passive_state = vec![PassiveState::default(); template.passives.len()];
                changing.cooldowns = vec![0; template.abilities.len()];
                changing.cast_remaining_ms = None;
                changing.casting = 0;
                changing.template = Arc::new(template);
                self.change_stats(target, &mods, true);
                // The same share of health in the new form.
                let now_most = self.u(target).stat(Stat::MaxHealth).max(1);
                let now = (health * now_most / most).clamp(1, now_most);
                self.u_mut(target).health = now;
                self.emit(EventKind::Transformed { unit: target, into: unit.clone(), name: form.name.clone() });
                self.emit(EventKind::HealthSet { unit: target, health: now, max_health: now_most });
            }
            Effect::Copy { health_permille } => {
                let template = self.u(target).template.clone();
                let team = self.u(source).team;
                let center = self.u(target).pos;
                let reach = self.board.cols() + self.board.rows();
                let Some(hex) = center.spiral(reach).into_iter().find(|hex| self.is_free(*hex)) else {
                    return;
                };
                if let Ok(copy) = self.spawn(template, team, hex) {
                    let most = self.u(copy).stat(Stat::MaxHealth).max(1);
                    let health = (most * *health_permille as i64 / 1000).clamp(1, most);
                    self.u_mut(copy).health = health;
                    self.emit(EventKind::HealthSet { unit: copy, health, max_health: most });
                }
            }
            Effect::Swap => {
                if source == target || !self.u(source).alive {
                    return;
                }
                let (a, b) = (self.u(source).pos, self.u(target).pos);
                let (ia, ib) = (self.board.index(a).unwrap(), self.board.index(b).unwrap());
                self.occupancy[ia] = Some(target);
                self.occupancy[ib] = Some(source);
                self.u_mut(source).pos = b;
                self.u_mut(target).pos = a;
                self.emit(EventKind::Displaced { unit: source, from: a, to: b });
                self.emit(EventKind::Displaced { unit: target, from: b, to: a });
                self.sync_tiles(source);
                self.sync_tiles(target);
            }
            Effect::PlaceTile { tile, radius, duration_ms } => {
                let center = self.u(target).pos;
                for hex in center.spiral(*radius) {
                    self.place_tile(tile, hex, Some(source), *duration_ms);
                }
            }
            Effect::Revive { health } => {
                if self.u(target).alive {
                    return;
                }
                // Where it fell, or as close to there as there is room.
                let fell = self.u(target).pos;
                let reach = self.board.cols() + self.board.rows();
                let Some(pos) = fell.spiral(reach).into_iter().find(|hex| self.is_free(*hex)) else {
                    return;
                };
                let health = self.eval(health, source, target).clamp(1, self.u(target).stat(Stat::MaxHealth).max(1));
                let idx = self.board.index(pos).unwrap();
                self.occupancy[idx] = Some(target);
                let unit = self.u_mut(target);
                unit.alive = true;
                unit.died_at = None;
                unit.health = health;
                unit.pos = pos;
                unit.target = None;
                self.emit(EventKind::Revived { unit: target, source, pos, health });
                self.sync_tiles(target);
            }
            Effect::Damage { amount, kind, can_crit } => {
                let raw = self.eval(amount, source, target);
                self.deal_damage(source, target, raw, *kind, *can_crit, direct);
            }
            Effect::Heal { amount } => {
                let raw = self.eval(amount, source, target);
                if self.heal(source, target, raw) > 0 && direct {
                    self.fire(target, Some(source), |t| matches!(t, Trigger::OnHealed));
                }
            }
            Effect::Signal { name } => {
                self.fire(target, Some(source), |t| matches!(t, Trigger::Signal(n) if n == name));
            }
            Effect::Shield { amount, duration_ms } => {
                let value = self.eval(amount, source, target);
                if value > 0 {
                    self.u_mut(target)
                        .shields
                        .push(ShieldInstance { amount: value, remaining_ms: *duration_ms });
                    self.emit(EventKind::Shielded { source, target, amount: value });
                }
            }
            Effect::GainResource { resource, amount } => {
                if let Some(index) = self.resources.iter().position(|d| d.id == *resource) {
                    let amount = self.eval(amount, source, target);
                    self.add_resource(target, index, amount);
                }
            }
            Effect::ApplyStatus { status, duration_ms } => {
                let t = self.u(target);
                if status.is_debuff() && t.has_status(|s| matches!(s, StatusKind::Unstoppable)) {
                    return;
                }
                let crowd_control = matches!(
                    status,
                    StatusKind::Stun | StatusKind::Silence | StatusKind::Disarm | StatusKind::Root | StatusKind::Taunt
                );
                let duration_ms = &if crowd_control {
                    let tenacity = t.stat(Stat::Tenacity).clamp(0, 800) as u32;
                    duration_ms - duration_ms * tenacity / 1000
                } else {
                    *duration_ms
                };
                if *duration_ms == 0 {
                    return;
                }
                if let StatusKind::StatMods(mods) = status {
                    self.change_stats(target, mods, true);
                }
                self.u_mut(target).statuses.push(StatusInstance {
                    kind: status.clone(),
                    remaining_ms: *duration_ms,
                    source,
                });
                self.emit(EventKind::StatusApplied {
                    source,
                    target,
                    status: status.clone(),
                    duration_ms: *duration_ms,
                });
            }
            Effect::ModifyStats { mods } => self.change_stats(target, mods, true),
            Effect::Execute { permille } => {
                let t = self.u(target);
                if t.health * 1000 <= t.stat(Stat::MaxHealth) * *permille as i64 {
                    let all = t.health + t.shield();
                    self.deal_damage(source, target, all.max(1) * 10, DamageKind::True, false, direct);
                }
            }
            Effect::Pull { hexes } => {
                if source == target {
                    return;
                }
                let origin = self.u(source).pos;
                let mut pos = self.u(target).pos;
                for _ in 0..*hexes {
                    if pos.distance(origin) <= 1 {
                        break;
                    }
                    let line = pos.line_to(origin);
                    match line.get(1) {
                        Some(&next) if self.is_free(next) => pos = next,
                        _ => break,
                    }
                }
                self.displace(target, pos);
            }
            Effect::Cleanse => {
                let mut removed = Vec::new();
                self.u_mut(target).statuses.retain(|s| {
                    let debuff = s.kind.is_debuff();
                    if debuff {
                        removed.push(s.kind.clone());
                    }
                    !debuff
                });
                for kind in removed {
                    if let StatusKind::StatMods(mods) = &kind {
                        self.change_stats(target, mods, false);
                    }
                    self.emit(EventKind::StatusExpired { target, status: kind });
                }
            }
            Effect::Summon { unit, count } => {
                let Some(template) = self.library.get(unit).cloned() else {
                    return;
                };
                let team = self.u(target).team;
                let center = self.u(target).pos;
                let reach = self.board.cols() + self.board.rows();
                let mut remaining = *count;
                for hex in center.spiral(reach) {
                    if remaining == 0 {
                        break;
                    }
                    if self.is_free(hex) {
                        let _ = self.spawn(template.clone(), team, hex);
                        remaining -= 1;
                    }
                }
            }
            Effect::AddStacks { stack, count, max, mods_per_stack } => {
                let current = self.u(target).stacks(stack);
                let wanted = current.saturating_add(*count);
                let new = max.map_or(wanted, |m| wanted.min(m)).max(current);
                let gained = (new - current) as i64;
                if gained == 0 {
                    return;
                }
                let granted: Vec<StatMod> = mods_per_stack
                    .iter()
                    .map(|m| StatMod { stat: m.stat, flat: m.flat * gained, permille: m.permille * gained })
                    .collect();
                self.change_stats(target, &granted, true);
                let state = self.u_mut(target).stacks.entry(stack.clone()).or_default();
                state.count = new;
                state.granted.extend(granted);
                self.emit(EventKind::StacksChanged { unit: target, stack: stack.clone(), count: new });
            }
            Effect::ClearStacks { stack } => {
                if let Some(state) = self.u_mut(target).stacks.remove(stack) {
                    self.change_stats(target, &state.granted, false);
                    self.emit(EventKind::StacksChanged { unit: target, stack: stack.clone(), count: 0 });
                }
            }
            Effect::Knockback { hexes } => {
                if source == target {
                    return;
                }
                // Follow the straight line from the source through the
                // target, stopping at the board edge or another unit.
                let origin = self.u(source).pos;
                let start = self.u(target).pos;
                let dist = origin.distance(start);
                let reach = 1 + hexes.div_ceil(dist) as i32;
                let line = origin.line_to(origin.add(start.sub(origin).scale(reach)));
                let mut pos = start;
                for &h in line.iter().skip(dist as usize + 1).take(*hexes as usize) {
                    if !self.is_free(h) {
                        break;
                    }
                    pos = h;
                }
                self.displace(target, pos);
            }
            Effect::Leap => {
                if source == target || !self.u(source).alive {
                    return;
                }
                let (from, center) = (self.u(source).pos, self.u(target).pos);
                let reach = self.board.cols() + self.board.rows();
                let best = center
                    .spiral(reach)
                    .into_iter()
                    .filter(|h| *h == from || self.is_free(*h))
                    .min_by_key(|h| (h.distance(center), h.distance(from)));
                if let Some(to) = best {
                    self.displace(source, to);
                }
                if self.u(target).team != self.u(source).team {
                    self.u_mut(source).target = Some(target);
                }
            }
            Effect::Custom { name, args } => {
                let Some(hook) = self.custom_effects.get(name).cloned() else {
                    return;
                };
                if self.hook_depth >= MAX_HOOK_DEPTH {
                    return;
                }
                self.hook_depth += 1;
                hook(&mut EffectContext { combat: self, source, target, other, args });
                self.hook_depth -= 1;
            }
        }
    }

    /// Applies or reverts stat modifiers, keeping health consistent with any
    /// change to maximum health.
    fn change_stats(&mut self, id: UnitId, mods: &[StatMod], apply: bool) {
        let unit = self.u_mut(id);
        let before = unit.stats.get(Stat::MaxHealth);
        for m in mods {
            if apply {
                unit.stats.apply(m);
            } else {
                unit.stats.remove(m);
            }
        }
        let after = unit.stats.get(Stat::MaxHealth).max(1);
        if after > before {
            unit.health += after - before;
        }
        unit.health = unit.health.min(after);
        // A change of capacity is news for whoever draws the bar.
        for index in 0..self.resources.len() {
            if mods.iter().any(|m| m.stat == self.resources[index].max) {
                let amount = self.u(id).resources[index];
                self.set_resource(id, index, amount, true);
            }
        }
    }

    /// Heals a unit and returns how much health it got back.
    fn heal(&mut self, source: UnitId, id: UnitId, amount: i64) -> i64 {
        let unit = self.u_mut(id);
        if !unit.alive || amount <= 0 {
            return 0;
        }
        let wound = unit
            .statuses
            .iter()
            .filter_map(|s| match s.kind {
                StatusKind::Wound { permille } => Some(permille.min(1000) as i64),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let amount = amount * (1000 - wound) / 1000;
        let max = unit.stats.get(Stat::MaxHealth);
        let healed = amount.min(max - unit.health).max(0);
        unit.health += healed;
        if healed > 0 {
            self.emit(EventKind::Healed { source, target: id, amount: healed });
        }
        healed
    }

    /// Runs the full damage pipeline: crit, amplification, resistance,
    /// reduction, shields, then health. `direct` is true for attacks and
    /// ability hits, which can trigger the target's `OnDamaged` passives.
    fn deal_damage(
        &mut self,
        source: UnitId,
        target: UnitId,
        raw: i64,
        kind: DamageKind,
        can_crit: bool,
        direct: bool,
    ) {
        if !self.u(target).alive || raw <= 0 {
            return;
        }
        if self.u(target).has_status(|s| matches!(s, StatusKind::Invulnerable)) {
            return;
        }
        // A kind with no rules behaves like true damage that shields soak.
        let rules = self.config.damage_types.iter().find(|t| t.kind == kind);
        let (resist, amp, reducible, shieldable) =
            rules.map_or((None, None, false, true), |t| (t.resist, t.amp, t.reducible, t.shieldable));
        let (crit_chance, crit_damage, amp, lifesteal) = {
            let s = self.u(source);
            (
                s.stat(Stat::CritChance),
                s.stat(Stat::CritDamage),
                s.stat(Stat::DamageAmp) + amp.map_or(0, |stat| s.stat(stat)),
                s.stat(Stat::Lifesteal),
            )
        };
        let mut amount = raw;
        let crit = can_crit && crit_chance > 0 && self.rng.chance(crit_chance as u64, 1000);
        if crit {
            amount = amount * crit_damage.max(1000) / 1000;
        }
        amount = amount * (1000 + amp) / 1000;
        let overtime = self.config.overtime_after_ms;
        if overtime > 0 && self.time_ms() > overtime {
            let seconds = ((self.time_ms() - overtime) / 1000 + 1) as i64;
            amount = amount * (1000 + self.config.overtime_damage_per_second * seconds) / 1000;
        }

        let k = self.config.resist_constant.max(1);
        let t = self.u(target);
        let resist = resist.map_or(0, |stat| t.stat(stat));
        amount = amount * k / (k + resist);
        if reducible {
            let reduction = t.stat(Stat::DamageReduction).min(self.config.max_damage_reduction);
            amount = amount * (1000 - reduction) / 1000;
        }
        if amount <= 0 {
            return;
        }

        // Shields soak damage first, oldest first.
        let unit = self.u_mut(target);
        let mut left = amount;
        for shield in unit.shields.iter_mut().filter(|_| shieldable) {
            let soak = shield.amount.min(left);
            shield.amount -= soak;
            left -= soak;
            if left == 0 {
                break;
            }
        }
        let shields_before = unit.shields.len();
        unit.shields.retain(|s| s.amount > 0);
        let broken = shields_before - unit.shields.len();
        let absorbed = amount - left;
        unit.health -= left;
        let health = unit.health;
        let max_health = unit.stats.get(Stat::MaxHealth);
        self.emit(EventKind::Damaged { source, target, amount: left, absorbed, kind, crit });
        if crit && source != target {
            self.fire(source, Some(target), |t| matches!(t, Trigger::OnCrit));
        }
        for _ in 0..broken {
            self.fire(target, Some(source), |t| matches!(t, Trigger::OnShieldBroken));
        }

        if lifesteal > 0 && source != target {
            self.heal(source, source, amount * lifesteal / 1000);
        }
        if source != target {
            self.gain_resources(source, |_, def| def.from_damage_dealt.of(amount));
        }

        if health <= 0 {
            self.kill(target, source);
            return;
        }

        self.gain_resources(target, |_, def| def.from_damage_taken.of(amount));
        if direct {
            self.fire(target, Some(source), |t| matches!(t, Trigger::OnDamaged));
        }
        self.fire(target, Some(source), |t| match t {
            Trigger::HealthBelow { permille } => health * 1000 <= *permille as i64 * max_health,
            _ => false,
        });
    }

    fn kill(&mut self, id: UnitId, killer: UnitId) {
        let idx = self.board.index(self.u(id).pos).unwrap();
        self.occupancy[idx] = None;
        let tick = self.tick;
        let unit = self.u_mut(id);
        unit.alive = false;
        unit.health = 0;
        unit.died_at = Some(tick);
        unit.shields.clear();
        unit.statuses.clear();
        unit.cast_remaining_ms = None;
        let team = unit.team;
        self.sync_tiles(id);
        self.emit(EventKind::Died { unit: id, killer });
        self.fire(id, Some(killer), |t| matches!(t, Trigger::OnDeath));
        if killer != id {
            self.gain_resources(killer, |c, def| if def.per_kill.is_zero() { 0 } else { c.eval(&def.per_kill, killer, id) });
            self.fire(killer, Some(id), |t| matches!(t, Trigger::OnKill));
        }
        for i in 0..self.units.len() {
            let other = &self.units[i];
            if other.alive && other.team == team {
                self.fire(other.id, Some(id), |t| matches!(t, Trigger::OnAllyDeath));
            } else if other.alive {
                self.fire(other.id, Some(id), |t| matches!(t, Trigger::OnEnemyDeath));
            }
        }
    }

    fn check_end(&mut self) {
        let mut teams: Vec<Team> = self.units.iter().filter(|u| u.alive).map(|u| u.team).collect();
        teams.sort();
        teams.dedup();
        let timed_out = self.time_ms() >= self.config.max_duration_ms;
        if teams.len() > 1 && !timed_out {
            return;
        }
        let winner = if teams.len() == 1 { Some(teams[0]) } else { None };
        let timed_out = winner.is_none() && timed_out && !teams.is_empty();
        self.emit(EventKind::Ended { winner, timed_out });
        self.outcome = Some(Outcome {
            winner,
            ticks: self.tick,
            duration_ms: self.time_ms(),
            timed_out,
            survivors: self.units.iter().filter(|u| u.alive).map(|u| u.id).collect(),
        });
    }
}

/// What a [`CustomEffectFn`] can see and do.
///
/// Reads go through [`EffectContext::combat`]. Changes go through
/// [`EffectContext::apply`], which runs any [`Effect`] (built-in or custom)
/// exactly as content would, so events, triggers and stat bookkeeping stay
/// consistent.
pub struct EffectContext<'a> {
    combat: &'a mut Combat,
    source: UnitId,
    target: UnitId,
    other: Option<UnitId>,
    args: &'a [i64],
}

impl EffectContext<'_> {
    /// The unit producing the effect.
    pub fn source(&self) -> UnitId {
        self.source
    }
    /// The selected unit this call is for.
    pub fn target(&self) -> UnitId {
        self.target
    }
    /// The other unit in the triggering event, if any (see
    /// [`Selector::EventOther`]).
    pub fn other(&self) -> Option<UnitId> {
        self.other
    }
    /// The arguments given in [`Effect::Custom`].
    pub fn args(&self) -> &[i64] {
        self.args
    }
    /// One argument, or zero if it was not given.
    pub fn arg(&self, index: usize) -> i64 {
        self.args.get(index).copied().unwrap_or(0)
    }
    /// The whole fight, read-only.
    pub fn combat(&self) -> &Combat {
        self.combat
    }
    /// The combat's seeded generator; the only allowed source of randomness.
    pub fn rng(&mut self) -> &mut Rng {
        &mut self.combat.rng
    }
    /// Units chosen by a selector, relative to the source.
    pub fn select(&self, selector: &Selector) -> Vec<UnitId> {
        self.combat.select(selector, self.source, self.other)
    }
    /// Computes an amount from the source to `target`.
    pub fn eval(&self, amount: &Amount, target: UnitId) -> i64 {
        self.combat.eval(amount, self.source, target)
    }
    /// Checks a condition from the source's point of view against `target`.
    pub fn check(&mut self, condition: &Condition, target: UnitId) -> bool {
        self.combat.check(condition, self.source, Some(target))
    }
    /// Applies an effect from the source to `target`.
    pub fn apply(&mut self, target: UnitId, effect: &Effect) {
        self.apply_from(self.source, target, effect);
    }
    /// Applies an effect as if `source` produced it.
    pub fn apply_from(&mut self, source: UnitId, target: UnitId, effect: &Effect) {
        self.combat.apply_effect(source, target, self.other, effect, false);
    }
    /// Moves a living unit to a free hex on the board. Returns whether it
    /// moved.
    pub fn move_unit(&mut self, unit: UnitId, to: Hex) -> bool {
        if !self.combat.u(unit).alive || !self.combat.is_free(to) {
            return false;
        }
        self.combat.displace(unit, to);
        true
    }
}

/// What a [`CustomSelectorFn`] can see.
pub struct SelectContext<'a> {
    combat: &'a Combat,
    source: UnitId,
    other: Option<UnitId>,
}

impl SelectContext<'_> {
    /// The unit producing the effect.
    pub fn source(&self) -> UnitId {
        self.source
    }
    /// The other unit in the triggering event, if any.
    pub fn other(&self) -> Option<UnitId> {
        self.other
    }
    pub fn combat(&self) -> &Combat {
        self.combat
    }
    /// The fight's dice as they stand, to roll without moving them: the
    /// same call gives the same rolls until the fight rolls again.
    pub fn rng(&self) -> Rng {
        self.combat.rng.clone().fork((self.combat.tick as u64) << 16 | self.source.0 as u64)
    }
}

/// What a [`CustomConditionFn`] can see.
pub struct ConditionContext<'a> {
    combat: &'a Combat,
    source: UnitId,
    target: Option<UnitId>,
}

impl ConditionContext<'_> {
    pub fn source(&self) -> UnitId {
        self.source
    }
    /// The unit being checked, or `None` for a passive triggered without
    /// another unit.
    pub fn target(&self) -> Option<UnitId> {
        self.target
    }
    pub fn combat(&self) -> &Combat {
        self.combat
    }
}
