//! Replays a finished fight for display.
//!
//! The server sends a [`CombatRecord`]: the full list of events of a fight
//! it already simulated. A client does not simulate anything; it feeds the
//! record to a [`Playback`] and asks, frame by frame, what the board looks
//! like at a given time. This module has no rendering in it, so any client
//! (graphical, terminal, web) can share it.

use std::collections::BTreeMap;

use crate::combat::{Event, EventKind, Team, UnitId};
use crate::effect::DamageKind;
use crate::game::CombatRecord;
use crate::hex::Hex;

/// How long a unit takes to glide between two hexes on screen.
pub const MOVE_ANIMATION_MS: u32 = 250;

/// A unit as it should be drawn right now.
#[derive(Clone, Debug, PartialEq)]
pub struct UnitView {
    pub id: UnitId,
    pub team: Team,
    pub name: String,
    pub pos: Hex,
    /// Where the unit was before its last move, and when it moved.
    pub prev_pos: Hex,
    pub moved_at_ms: u32,
    pub health: i64,
    pub max_health: i64,
    pub shield: i64,
    pub alive: bool,
    pub casting: bool,
    pub stunned_until_ms: u32,
    /// Statuses on the unit by [`StatusKind::name`](crate::effect::StatusKind::name),
    /// each with when it ends; see [`UnitView::statuses_at`].
    pub statuses: Vec<(String, u32)>,
    /// Named stack counters (see `Effect::AddStacks`), without the empty ones.
    pub stacks: BTreeMap<String, u32>,
    /// The unit's resource bars (mana and whatever else the game defines),
    /// in the order they first appeared.
    pub resources: Vec<ResourceView>,
}

/// One resource bar of a unit in a replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceView {
    /// The resource's id, as in [`ResourceDef::id`](crate::resource::ResourceDef).
    pub id: String,
    pub amount: i64,
    pub max: i64,
}

impl UnitView {
    /// Names of the statuses the unit has at `time_ms`, each once, in the
    /// order they were applied.
    pub fn statuses_at(&self, time_ms: u32) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for (name, until) in &self.statuses {
            if *until > time_ms && !names.contains(&name.as_str()) {
                names.push(name);
            }
        }
        names
    }

    /// How much of a resource the unit has, by resource id.
    pub fn resource(&self, id: &str) -> Option<&ResourceView> {
        self.resources.iter().find(|r| r.id == id)
    }

    /// Position in fractional axial coordinates `(q, r)`, easing between
    /// hexes while a move is being animated.
    pub fn position_at(&self, time_ms: u32) -> (f32, f32) {
        let t = (time_ms.saturating_sub(self.moved_at_ms) as f32 / MOVE_ANIMATION_MS as f32).clamp(0.0, 1.0);
        let lerp = |a: i32, b: i32| a as f32 + (b - a) as f32 * t;
        (lerp(self.prev_pos.q, self.pos.q), lerp(self.prev_pos.r, self.pos.r))
    }
}

/// A short-lived thing to draw: an attack line, a damage number, and so on.
#[derive(Clone, Debug, PartialEq)]
pub enum Flash {
    Attack { source: UnitId, target: UnitId },
    Damage { target: UnitId, amount: i64, absorbed: i64, kind: DamageKind, crit: bool },
    Heal { target: UnitId, amount: i64 },
    Cast { unit: UnitId },
    Death { unit: UnitId },
    /// A basic attack on `target` missed.
    Miss { target: UnitId },
}

/// The state of a replay at its current time.
#[derive(Clone, Debug)]
pub struct Playback {
    events: Vec<Event>,
    tick_ms: u32,
    duration_ms: u32,
    cursor: usize,
    time_ms: u32,
    units: BTreeMap<UnitId, UnitView>,
    flashes: Vec<(u32, Flash)>,
    winner: Option<Team>,
    ended: bool,
    /// When overtime began, if it has.
    overtime_at_ms: Option<u32>,
}

impl Playback {
    pub fn new(record: &CombatRecord) -> Self {
        let mut playback = Playback::from_events(record.events.clone(), record.tick_ms);
        playback.duration_ms = record.duration_ms;
        playback.winner = record.winner;
        playback
    }

    /// Replays any fight from its events, such as
    /// [`Combat::events`](crate::combat::Combat::events) of one run outside
    /// a game. `tick_ms` is the fight's
    /// [`CombatConfig::tick_ms`](crate::combat::CombatConfig).
    pub fn from_events(events: Vec<Event>, tick_ms: u32) -> Self {
        let tick_ms = tick_ms.max(1);
        let end = events.iter().rev().find_map(|e| match e.kind {
            EventKind::Ended { winner, .. } => Some((e.tick * tick_ms, winner)),
            _ => None,
        });
        let last = events.last().map_or(0, |e| e.tick * tick_ms);
        let mut playback = Playback {
            events,
            tick_ms,
            duration_ms: end.map_or(last, |(at, _)| at),
            cursor: 0,
            time_ms: 0,
            units: BTreeMap::new(),
            flashes: Vec::new(),
            winner: end.and_then(|(_, winner)| winner),
            ended: false,
            overtime_at_ms: None,
        };
        playback.advance_to(0);
        playback
    }

    /// Applies every event up to `time_ms`. Time only moves forward.
    pub fn advance_to(&mut self, time_ms: u32) {
        self.time_ms = self.time_ms.max(time_ms);
        while let Some(event) = self.events.get(self.cursor) {
            let at = event.tick * self.tick_ms;
            if at > self.time_ms {
                break;
            }
            let kind = event.kind.clone();
            self.cursor += 1;
            self.apply(at, kind);
        }
    }

    fn apply(&mut self, at: u32, kind: EventKind) {
        match kind {
            EventKind::Spawned { unit, team, pos, name, max_health } => {
                self.units.insert(
                    unit,
                    UnitView {
                        id: unit,
                        team,
                        name,
                        pos,
                        prev_pos: pos,
                        moved_at_ms: 0,
                        health: max_health,
                        max_health,
                        shield: 0,
                        alive: true,
                        casting: false,
                        stunned_until_ms: 0,
                        statuses: Vec::new(),
                        stacks: BTreeMap::new(),
                        resources: Vec::new(),
                    },
                );
            }
            EventKind::Moved { unit, from, to } | EventKind::Displaced { unit, from, to } => {
                if let Some(u) = self.units.get_mut(&unit) {
                    u.prev_pos = from;
                    u.pos = to;
                    u.moved_at_ms = at;
                }
            }
            EventKind::Attacked { source, target } => self.flashes.push((at, Flash::Attack { source, target })),
            EventKind::Dodged { target, .. } => self.flashes.push((at, Flash::Miss { target })),
            EventKind::Overtime => self.overtime_at_ms = Some(at),
            EventKind::Damaged { target, amount, absorbed, kind, crit, .. } => {
                if let Some(u) = self.units.get_mut(&target) {
                    u.health = (u.health - amount).max(0);
                    u.shield = (u.shield - absorbed).max(0);
                }
                self.flashes.push((at, Flash::Damage { target, amount, absorbed, kind, crit }));
            }
            EventKind::Healed { target, amount, .. } => {
                if let Some(u) = self.units.get_mut(&target) {
                    u.health = (u.health + amount).min(u.max_health.max(u.health + amount));
                    u.max_health = u.max_health.max(u.health);
                }
                self.flashes.push((at, Flash::Heal { target, amount }));
            }
            EventKind::Shielded { target, amount, .. } => {
                if let Some(u) = self.units.get_mut(&target) {
                    u.shield += amount;
                }
            }
            EventKind::CastStarted { unit, .. } => {
                if let Some(u) = self.units.get_mut(&unit) {
                    u.casting = true;
                }
            }
            EventKind::CastFinished { unit, .. } => {
                if let Some(u) = self.units.get_mut(&unit) {
                    u.casting = false;
                }
                self.flashes.push((at, Flash::Cast { unit }));
            }
            EventKind::StatusApplied { target, status, duration_ms, .. } => {
                if let Some(u) = self.units.get_mut(&target) {
                    if status == crate::effect::StatusKind::Stun {
                        u.stunned_until_ms = u.stunned_until_ms.max(at + duration_ms);
                    }
                    u.statuses.push((status.name().to_string(), at + duration_ms));
                }
            }
            EventKind::StatusExpired { target, status } => {
                // The one ending soonest goes; it ends now (a cleanse can
                // end it early).
                if let Some(u) = self.units.get_mut(&target) {
                    let name = status.name();
                    let soonest = u.statuses.iter().enumerate().filter(|(_, (n, _))| n == name).min_by_key(|(_, (_, until))| *until);
                    if let Some((i, _)) = soonest {
                        u.statuses.remove(i);
                    }
                    if status == crate::effect::StatusKind::Stun {
                        u.stunned_until_ms = u.statuses.iter().filter(|(n, _)| n == name).map(|(_, until)| *until).max().unwrap_or(at).min(u.stunned_until_ms);
                    }
                }
            }
            EventKind::Died { unit, .. } => {
                if let Some(u) = self.units.get_mut(&unit) {
                    u.alive = false;
                    u.health = 0;
                    u.shield = 0;
                    u.casting = false;
                }
                self.flashes.push((at, Flash::Death { unit }));
            }
            EventKind::Revived { unit, pos, health, .. } => {
                if let Some(u) = self.units.get_mut(&unit) {
                    u.alive = true;
                    u.health = health;
                    u.pos = pos;
                    u.prev_pos = pos;
                    u.statuses.clear();
                    u.stunned_until_ms = 0;
                }
            }
            EventKind::StacksChanged { unit, stack, count } => {
                if let Some(u) = self.units.get_mut(&unit) {
                    if count == 0 {
                        u.stacks.remove(&stack);
                    } else {
                        u.stacks.insert(stack, count);
                    }
                }
            }
            EventKind::ResourceChanged { unit, resource, amount, max } => {
                if let Some(u) = self.units.get_mut(&unit) {
                    match u.resources.iter_mut().find(|r| r.id == resource) {
                        Some(bar) => (bar.amount, bar.max) = (amount, max),
                        None => u.resources.push(ResourceView { id: resource, amount, max }),
                    }
                    u.resources.retain(|r| r.max > 0);
                }
            }
            EventKind::Ended { .. } => self.ended = true,
            EventKind::PassiveTriggered { .. } => {}
        }
    }

    pub fn time_ms(&self) -> u32 {
        self.time_ms
    }

    /// Length of the fight itself.
    pub fn duration_ms(&self) -> u32 {
        self.duration_ms
    }

    /// True once every event has been applied.
    pub fn is_finished(&self) -> bool {
        self.ended && self.cursor >= self.events.len()
    }

    pub fn winner(&self) -> Option<Team> {
        self.winner
    }

    /// Every unit that has appeared so far, including the fallen.
    /// Every unit's numbers for the whole fight (see [`fight_stats`]).
    pub fn stats(&self) -> Vec<UnitFightStats> {
        fight_stats(&self.events)
    }

    /// Whether the fight is in overtime by now (see
    /// [`CombatConfig::overtime_after_ms`](crate::combat::CombatConfig)).
    pub fn in_overtime(&self) -> bool {
        self.overtime_at_ms.is_some()
    }

    pub fn units(&self) -> impl Iterator<Item = &UnitView> {
        self.units.values()
    }

    pub fn unit(&self, id: UnitId) -> Option<&UnitView> {
        self.units.get(&id)
    }

    /// Flashes that started within the last `window_ms`, with their age.
    pub fn recent_flashes(&self, window_ms: u32) -> impl Iterator<Item = (u32, &Flash)> {
        let now = self.time_ms;
        self.flashes.iter().filter_map(move |(at, flash)| {
            let age = now.saturating_sub(*at);
            (age <= window_ms).then_some((age, flash))
        })
    }

    /// Drops flashes older than `window_ms` to keep long replays light.
    pub fn forget_flashes_older_than(&mut self, window_ms: u32) {
        let now = self.time_ms;
        self.flashes.retain(|(at, _)| now.saturating_sub(*at) <= window_ms);
    }
}

/// How one unit did in a fight, for damage meters after the fight.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnitFightStats {
    pub unit: UnitId,
    pub team: Team,
    pub name: String,
    /// Damage dealt, shields included.
    pub damage_dealt: i64,
    /// Damage taken, shields included.
    pub damage_taken: i64,
    /// Health restored to anyone, itself included.
    pub healing_done: i64,
    /// Shields given to anyone, itself included.
    pub shielding_done: i64,
    pub kills: u32,
    pub casts: u32,
    /// Basic attacks it dodged.
    pub dodges: u32,
    pub survived: bool,
}

/// Every unit's numbers for a fight, from its events: spawn order, which
/// puts the home team first.
pub fn fight_stats(events: &[Event]) -> Vec<UnitFightStats> {
    let mut stats: BTreeMap<UnitId, UnitFightStats> = BTreeMap::new();
    for event in events {
        match &event.kind {
            EventKind::Spawned { unit, team, name, .. } => {
                stats.insert(*unit, UnitFightStats { unit: *unit, team: *team, name: name.clone(), survived: true, ..Default::default() });
            }
            EventKind::Damaged { source, target, amount, absorbed, .. } => {
                let total = amount + absorbed;
                if let Some(s) = stats.get_mut(source).filter(|_| source != target) {
                    s.damage_dealt += total;
                }
                if let Some(s) = stats.get_mut(target) {
                    s.damage_taken += total;
                }
            }
            EventKind::Healed { source, amount, .. } => {
                if let Some(s) = stats.get_mut(source) {
                    s.healing_done += amount;
                }
            }
            EventKind::Shielded { source, amount, .. } => {
                if let Some(s) = stats.get_mut(source) {
                    s.shielding_done += amount;
                }
            }
            EventKind::CastFinished { unit, .. } => {
                if let Some(s) = stats.get_mut(unit) {
                    s.casts += 1;
                }
            }
            EventKind::Dodged { target, .. } => {
                if let Some(s) = stats.get_mut(target) {
                    s.dodges += 1;
                }
            }
            EventKind::Died { unit, killer } => {
                if let Some(s) = stats.get_mut(unit) {
                    s.survived = false;
                }
                if let Some(s) = stats.get_mut(killer).filter(|_| killer != unit) {
                    s.kills += 1;
                }
            }
            _ => {}
        }
    }
    stats.into_values().collect()
}
