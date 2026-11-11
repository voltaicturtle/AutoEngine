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
}

impl UnitView {
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
}

impl Playback {
    pub fn new(record: &CombatRecord) -> Self {
        let mut playback = Playback {
            events: record.events.clone(),
            tick_ms: record.tick_ms.max(1),
            duration_ms: record.duration_ms,
            cursor: 0,
            time_ms: 0,
            units: BTreeMap::new(),
            flashes: Vec::new(),
            winner: record.winner,
            ended: false,
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
                    },
                );
            }
            EventKind::Moved { unit, from, to } => {
                if let Some(u) = self.units.get_mut(&unit) {
                    u.prev_pos = from;
                    u.pos = to;
                    u.moved_at_ms = at;
                }
            }
            EventKind::Attacked { source, target } => self.flashes.push((at, Flash::Attack { source, target })),
            EventKind::Damaged { target, amount, absorbed, kind, crit, .. } => {
                if let Some(u) = self.units.get_mut(&target) {
                    u.health = (u.health - amount).max(0);
                    u.shield = (u.shield - absorbed).max(0);
                }
                self.flashes.push((at, Flash::Damage { target, amount, absorbed, kind, crit }));
            }
            EventKind::Healed { target, amount } => {
                if let Some(u) = self.units.get_mut(&target) {
                    u.health = (u.health + amount).min(u.max_health.max(u.health + amount));
                    u.max_health = u.max_health.max(u.health);
                }
                self.flashes.push((at, Flash::Heal { target, amount }));
            }
            EventKind::Shielded { target, amount } => {
                if let Some(u) = self.units.get_mut(&target) {
                    u.shield += amount;
                }
            }
            EventKind::CastStarted { unit } => {
                if let Some(u) = self.units.get_mut(&unit) {
                    u.casting = true;
                }
            }
            EventKind::CastFinished { unit } => {
                if let Some(u) = self.units.get_mut(&unit) {
                    u.casting = false;
                }
                self.flashes.push((at, Flash::Cast { unit }));
            }
            EventKind::StatusApplied { target, status, duration_ms, .. } => {
                if let (Some(u), crate::effect::StatusKind::Stun) = (self.units.get_mut(&target), status) {
                    u.stunned_until_ms = u.stunned_until_ms.max(at + duration_ms);
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
            EventKind::Ended { .. } => self.ended = true,
            EventKind::PassiveTriggered { .. } | EventKind::StatusExpired { .. } => {}
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
