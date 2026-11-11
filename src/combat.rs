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

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::effect::{
    Amount, Anchor, DamageKind, Effect, EffectSpec, ScaleBy, Selector, StatusKind, Trigger,
    UnitTemplate, Who,
};
use crate::hex::{Hex, HexBoard};
use crate::rng::Rng;
use crate::stats::{Stat, StatMod, Stats};

/// Identifies a unit within one combat. Ids are assigned in spawn order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UnitId(pub u32);

/// Identifies a side. Units on different teams are enemies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Team(pub u8);

/// Progress needed for one attack or one step: speeds are in thousandths per
/// second and time is in milliseconds.
const PROGRESS_FULL: i64 = 1_000_000;

/// Upper bound on triggered passives resolved in one go, as a guard against
/// content that triggers itself forever.
const MAX_TRIGGER_CHAIN: usize = 10_000;

/// Tunable rules of the simulation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CombatConfig {
    /// Length of one simulation step.
    pub tick_ms: u32,
    /// The fight ends in a draw after this long.
    pub max_duration_ms: u32,
    /// Mana gained per point of damage taken, in permille.
    pub mana_per_damage_permille: i64,
    /// Most mana one instance of damage can grant.
    pub mana_from_damage_cap: i64,
    /// Damage multiplier from resistance is `k / (k + resist)`.
    pub resist_constant: i64,
    /// Cap on the `DamageReduction` stat, in permille.
    pub max_damage_reduction: i64,
}

impl Default for CombatConfig {
    fn default() -> Self {
        CombatConfig {
            tick_ms: 50,
            max_duration_ms: 60_000,
            mana_per_damage_permille: 30,
            mana_from_damage_cap: 40,
            resist_constant: 100,
            max_damage_reduction: 900,
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

/// A unit taking part in a combat.
#[derive(Clone, Debug)]
pub struct CombatUnit {
    id: UnitId,
    team: Team,
    template: Arc<UnitTemplate>,
    pos: Hex,
    stats: Stats,
    health: i64,
    mana: i64,
    alive: bool,
    target: Option<UnitId>,
    attack_progress: i64,
    move_progress: i64,
    cast_remaining_ms: Option<u32>,
    shields: Vec<ShieldInstance>,
    statuses: Vec<StatusInstance>,
    passive_state: Vec<PassiveState>,
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
    pub fn mana(&self) -> i64 {
        self.mana
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
        self.statuses.iter().any(|s| matches!(s.kind, StatusKind::Stun))
    }
}

/// Something that happened during combat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    Spawned { unit: UnitId, team: Team, pos: Hex, name: String, max_health: i64 },
    Moved { unit: UnitId, from: Hex, to: Hex },
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
    Healed { target: UnitId, amount: i64 },
    Shielded { target: UnitId, amount: i64 },
    CastStarted { unit: UnitId },
    CastFinished { unit: UnitId },
    PassiveTriggered { unit: UnitId, passive: usize },
    StatusApplied { source: UnitId, target: UnitId, status: StatusKind, duration_ms: u32 },
    StatusExpired { target: UnitId, status: StatusKind },
    Died { unit: UnitId, killer: UnitId },
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
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::OffBoard(h) => write!(f, "hex {:?} is off the board", h.to_offset()),
            SpawnError::Occupied(h) => write!(f, "hex {:?} is occupied", h.to_offset()),
        }
    }
}

impl std::error::Error for SpawnError {}

struct PendingTrigger {
    owner: UnitId,
    passive: usize,
    other: Option<UnitId>,
}

/// One fight, from setup to outcome.
pub struct Combat {
    board: HexBoard,
    config: CombatConfig,
    rng: Rng,
    units: Vec<CombatUnit>,
    library: HashMap<String, Arc<UnitTemplate>>,
    occupancy: Vec<Option<UnitId>>,
    tick: u32,
    started: bool,
    events: Vec<Event>,
    pending: VecDeque<PendingTrigger>,
    outcome: Option<Outcome>,
}

impl Combat {
    pub fn new(board: HexBoard, seed: u64) -> Self {
        Combat::with_config(board, seed, CombatConfig::default())
    }

    pub fn with_config(board: HexBoard, seed: u64, config: CombatConfig) -> Self {
        assert!(config.tick_ms > 0, "tick_ms must be positive");
        let cells = board.len();
        Combat {
            board,
            config,
            rng: Rng::new(seed),
            units: Vec::new(),
            library: HashMap::new(),
            occupancy: vec![None; cells],
            tick: 0,
            started: false,
            events: Vec::new(),
            pending: VecDeque::new(),
            outcome: None,
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
        let template = template.into();
        let id = UnitId(self.units.len() as u32);
        let stats = Stats::new(template.stats.clone());
        let health = stats.get(Stat::MaxHealth).max(1);
        let mana = stats.get(Stat::StartingMana).min(stats.get(Stat::MaxMana));
        let passive_state = vec![PassiveState::default(); template.passives.len()];
        self.units.push(CombatUnit {
            id,
            team,
            template,
            pos,
            stats,
            health,
            mana,
            alive: true,
            target: None,
            attack_progress: 0,
            move_progress: 0,
            cast_remaining_ms: None,
            shields: Vec::new(),
            statuses: Vec::new(),
            passive_state,
        });
        self.occupancy[idx] = Some(id);
        let name = self.u(id).template.name.clone();
        self.emit(EventKind::Spawned { unit: id, team, pos, name, max_health: health });
        if self.started {
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
            for i in 0..self.units.len() {
                self.fire(UnitId(i as u32), None, |t| matches!(t, Trigger::CombatStart));
            }
            self.drain_triggers();
        }
        self.tick += 1;
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

        // Start a cast when mana is full.
        let max_mana = self.u(id).stat(Stat::MaxMana);
        if max_mana > 0 && self.u(id).mana >= max_mana && self.u(id).template.ability.is_some() {
            let cast_time = self.u(id).template.ability.as_ref().unwrap().cast_time_ms;
            self.u_mut(id).mana = 0;
            self.emit(EventKind::CastStarted { unit: id });
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
            let speed = self.u(id).stat(Stat::MoveSpeed);
            self.u_mut(id).move_progress += speed * tick_ms as i64;
            if self.u(id).move_progress >= PROGRESS_FULL {
                let board = &self.board;
                let occupancy = &self.occupancy;
                let free = |h: Hex| board.index(h).is_some_and(|i| occupancy[i].is_none());
                let path = board.find_path(
                    pos,
                    |h| h.distance(target_pos) <= range && free(h),
                    free,
                );
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

    /// Advances shields, statuses and periodic passives by one tick.
    fn tick_timers(&mut self, id: UnitId) {
        let tick_ms = self.config.tick_ms;

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
        if let Some(t) = self.u(id).target
            && self.u(t).alive
        {
            return Some(t);
        }
        let me = self.u(id);
        let target = self
            .units
            .iter()
            .filter(|o| o.alive && o.team != me.team)
            .min_by_key(|o| (me.pos.distance(o.pos), o.id))
            .map(|o| o.id);
        self.u_mut(id).target = target;
        target
    }

    fn move_unit(&mut self, id: UnitId, to: Hex) {
        let from = self.u(id).pos;
        let from_idx = self.board.index(from).unwrap();
        let to_idx = self.board.index(to).unwrap();
        debug_assert!(self.occupancy[to_idx].is_none());
        self.occupancy[from_idx] = None;
        self.occupancy[to_idx] = Some(id);
        self.u_mut(id).pos = to;
        self.emit(EventKind::Moved { unit: id, from, to });
    }

    fn basic_attack(&mut self, id: UnitId, target: UnitId) {
        self.emit(EventKind::Attacked { source: id, target });
        let damage = self.u(id).stat(Stat::AttackDamage);
        let mana = self.u(id).stat(Stat::ManaPerAttack);
        self.add_mana(id, mana);
        self.deal_damage(id, target, damage, DamageKind::Physical, true, true);
        self.fire(id, Some(target), |t| matches!(t, Trigger::OnAttack));
    }

    fn finish_cast(&mut self, id: UnitId) {
        self.u_mut(id).cast_remaining_ms = None;
        self.emit(EventKind::CastFinished { unit: id });
        let template = self.u(id).template.clone();
        if let Some(ability) = &template.ability {
            self.resolve(id, None, &ability.effects, true);
        }
        self.fire(id, None, |t| matches!(t, Trigger::OnCast));
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
            let state = &mut self.u_mut(p.owner).passive_state[p.passive];
            if limit.is_some_and(|max| state.procs >= max) {
                continue;
            }
            state.procs += 1;
            self.emit(EventKind::PassiveTriggered { unit: p.owner, passive: p.passive });
            // Effects from passives do not themselves count as attacks or
            // ability hits, which keeps reactive passives from ping-ponging.
            self.resolve(p.owner, p.other, &passive.effects, false);
        }
    }

    // ----- effect resolution ----------------------------------------------

    fn resolve(&mut self, source: UnitId, other: Option<UnitId>, effects: &[EffectSpec], direct: bool) {
        for spec in effects {
            for target in self.select(&spec.target, source, other) {
                self.apply_effect(source, target, &spec.effect, direct);
            }
        }
    }

    fn select(&self, selector: &Selector, source: UnitId, other: Option<UnitId>) -> Vec<UnitId> {
        let me = self.u(source);
        let enemies = || self.units.iter().filter(|o| o.alive && o.team != me.team);
        let allies = || self.units.iter().filter(|o| o.alive && o.team == me.team);
        let nearest_enemy = || enemies().min_by_key(|o| (me.pos.distance(o.pos), o.id));
        let current_target = || {
            me.target
                .map(|t| self.u(t))
                .filter(|t| t.alive)
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
        }
    }

    fn eval(&self, amount: &Amount, source: UnitId, target: UnitId) -> i64 {
        let mut total = amount.flat;
        for s in &amount.scalings {
            let unit = match s.who {
                Who::Source => self.u(source),
                Who::Target => self.u(target),
            };
            let value = match s.by {
                ScaleBy::Stat(stat) => unit.stat(stat),
                ScaleBy::CurrentHealth => unit.health.max(0),
                ScaleBy::MissingHealth => (unit.stat(Stat::MaxHealth) - unit.health).max(0),
            };
            total += value * s.permille / 1000;
        }
        if amount.power_scaled {
            total = total * self.u(source).stat(Stat::AbilityPower) / 100;
        }
        total
    }

    fn apply_effect(&mut self, source: UnitId, target: UnitId, effect: &Effect, direct: bool) {
        // Summons may be anchored on a unit that just died; everything else
        // needs a living target.
        if !self.u(target).alive && !matches!(effect, Effect::Summon { .. }) {
            return;
        }
        match effect {
            Effect::Damage { amount, kind, can_crit } => {
                let raw = self.eval(amount, source, target);
                self.deal_damage(source, target, raw, *kind, *can_crit, direct);
            }
            Effect::Heal { amount } => {
                let raw = self.eval(amount, source, target);
                self.heal(target, raw);
            }
            Effect::Shield { amount, duration_ms } => {
                let value = self.eval(amount, source, target);
                if value > 0 {
                    self.u_mut(target)
                        .shields
                        .push(ShieldInstance { amount: value, remaining_ms: *duration_ms });
                    self.emit(EventKind::Shielded { target, amount: value });
                }
            }
            Effect::GainMana { amount } => self.add_mana(target, *amount),
            Effect::ApplyStatus { status, duration_ms } => {
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
                    if self.board.contains(hex) && self.unit_at(hex).is_none() {
                        let _ = self.spawn(template.clone(), team, hex);
                        remaining -= 1;
                    }
                }
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
        unit.mana = unit.mana.min(unit.stats.get(Stat::MaxMana).max(0));
    }

    fn add_mana(&mut self, id: UnitId, amount: i64) {
        let unit = self.u_mut(id);
        let max = unit.stats.get(Stat::MaxMana);
        if !unit.alive || max <= 0 || unit.cast_remaining_ms.is_some() {
            return;
        }
        unit.mana = (unit.mana + amount).clamp(0, max);
    }

    fn heal(&mut self, id: UnitId, amount: i64) {
        let unit = self.u_mut(id);
        if !unit.alive || amount <= 0 {
            return;
        }
        let max = unit.stats.get(Stat::MaxHealth);
        let healed = amount.min(max - unit.health).max(0);
        unit.health += healed;
        if healed > 0 {
            self.emit(EventKind::Healed { target: id, amount: healed });
        }
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
        let (crit_chance, crit_damage, amp, lifesteal) = {
            let s = self.u(source);
            (
                s.stat(Stat::CritChance),
                s.stat(Stat::CritDamage),
                s.stat(Stat::DamageAmp),
                s.stat(Stat::Lifesteal),
            )
        };
        let mut amount = raw;
        let crit = can_crit && crit_chance > 0 && self.rng.chance(crit_chance as u64, 1000);
        if crit {
            amount = amount * crit_damage.max(1000) / 1000;
        }
        amount = amount * (1000 + amp) / 1000;

        let k = self.config.resist_constant.max(1);
        let t = self.u(target);
        let resist = match kind {
            DamageKind::Physical => t.stat(Stat::Armor),
            DamageKind::Magic => t.stat(Stat::MagicResist),
            DamageKind::True => 0,
        };
        amount = amount * k / (k + resist);
        if kind != DamageKind::True {
            let reduction = t.stat(Stat::DamageReduction).min(self.config.max_damage_reduction);
            amount = amount * (1000 - reduction) / 1000;
        }
        if amount <= 0 {
            return;
        }

        // Shields soak damage first, oldest first.
        let unit = self.u_mut(target);
        let mut left = amount;
        for shield in unit.shields.iter_mut() {
            let soak = shield.amount.min(left);
            shield.amount -= soak;
            left -= soak;
            if left == 0 {
                break;
            }
        }
        unit.shields.retain(|s| s.amount > 0);
        let absorbed = amount - left;
        unit.health -= left;
        let health = unit.health;
        let max_health = unit.stats.get(Stat::MaxHealth);
        self.emit(EventKind::Damaged { source, target, amount: left, absorbed, kind, crit });

        if lifesteal > 0 && source != target {
            self.heal(source, amount * lifesteal / 1000);
        }

        if health <= 0 {
            self.kill(target, source);
            return;
        }

        let mana = (amount * self.config.mana_per_damage_permille / 1000)
            .min(self.config.mana_from_damage_cap);
        self.add_mana(target, mana);
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
        let unit = self.u_mut(id);
        unit.alive = false;
        unit.health = 0;
        unit.shields.clear();
        unit.statuses.clear();
        unit.cast_remaining_ms = None;
        let team = unit.team;
        self.emit(EventKind::Died { unit: id, killer });
        self.fire(id, Some(killer), |t| matches!(t, Trigger::OnDeath));
        if killer != id {
            self.fire(killer, Some(id), |t| matches!(t, Trigger::OnKill));
        }
        for i in 0..self.units.len() {
            let ally = &self.units[i];
            if ally.alive && ally.team == team {
                self.fire(ally.id, Some(id), |t| matches!(t, Trigger::OnAllyDeath));
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
