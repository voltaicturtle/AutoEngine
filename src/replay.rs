//! Fights saved to files.
//!
//! A replay file holds one fight: the content it was fought with, both
//! teams, the seed and the full event log. That is enough to watch it again
//! without a server, and, because fights are deterministic, to re-run it and
//! check that the engine still produces exactly the same events.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::combat::Event;
use crate::content::ContentSet;
use crate::game::CombatRecord;
use crate::protocol::ContentInfo;

pub const REPLAY_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayFile {
    pub version: u32,
    /// For people: who fought whom, and when in the game.
    pub title: String,
    pub content: ContentInfo,
    pub record: CombatRecord,
}

/// Why a replay no longer matches the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    /// The file is from a different replay format.
    Version { found: u32 },
    /// The record does not say who fought, so it cannot be re-run.
    NoTeams,
    Content(String),
    /// Re-running the fight gave different events, starting at this index.
    Diverged { event: usize },
    /// Same events but a different result.
    Outcome,
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::Version { found } => write!(f, "replay format {found}, this build reads {REPLAY_VERSION}"),
            ReplayError::NoTeams => write!(f, "the replay does not record the teams"),
            ReplayError::Content(e) => write!(f, "the replay's content is invalid: {e}"),
            ReplayError::Diverged { event } => write!(f, "re-running the fight differs from event {event} on"),
            ReplayError::Outcome => write!(f, "re-running the fight gives a different result"),
        }
    }
}

impl ReplayFile {
    pub fn new(title: impl Into<String>, content: ContentInfo, record: CombatRecord) -> Self {
        ReplayFile { version: REPLAY_VERSION, title: title.into(), content, record }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let replay: ReplayFile = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if replay.version != REPLAY_VERSION {
            return Err(format!("{}: {}", path.display(), ReplayError::Version { found: replay.version }));
        }
        Ok(replay)
    }

    /// Fights the recorded teams again with the recorded seed and compares
    /// every event. Passing means this build of the engine, with the
    /// replay's content, still plays the fight out identically.
    pub fn verify(&self) -> Result<(), ReplayError> {
        if self.version != REPLAY_VERSION {
            return Err(ReplayError::Version { found: self.version });
        }
        if self.record.teams.len() < 2 {
            return Err(ReplayError::NoTeams);
        }
        let content = self.content.clone().into_content().map_err(|e| ReplayError::Content(format!("{e:?}")))?;
        let mut combat = content
            .setup_combat(self.record.seed, &self.record.teams)
            .map_err(|e| ReplayError::Content(format!("{e:?}")))?;
        let outcome = combat.run().clone();
        let events = combat.drain_events();
        if let Some(event) = events.iter().zip(&self.record.events).position(|(a, b)| a != b) {
            return Err(ReplayError::Diverged { event });
        }
        if events.len() != self.record.events.len() {
            return Err(ReplayError::Diverged { event: events.len().min(self.record.events.len()) });
        }
        if outcome.winner != self.record.winner || outcome.duration_ms != self.record.duration_ms {
            return Err(ReplayError::Outcome);
        }
        Ok(())
    }
}

/// Fights a recorded fight again from its teams and seed and returns its
/// events. With the same content and engine the result is always the same,
/// which is how clients turn a [`ServerMsg::CombatSeed`](crate::protocol::ServerMsg::CombatSeed)
/// back into a full record. Fails if the result does not match what the
/// record says about the fight.
pub fn fight_again(content: &ContentSet, record: &CombatRecord, expected_events: usize) -> Result<Vec<Event>, String> {
    if record.teams.len() < 2 {
        return Err("the record does not say who fought".into());
    }
    let mut combat = content.setup_combat(record.seed, &record.teams).map_err(|e| e.to_string())?;
    let outcome = combat.run().clone();
    let events = combat.drain_events();
    if outcome.winner != record.winner || outcome.duration_ms != record.duration_ms || events.len() != expected_events {
        return Err("the fight played out differently here".into());
    }
    Ok(events)
}

/// A file name for a fight: round number, then the title made safe.
pub fn file_name(round: usize, title: &str) -> PathBuf {
    let safe: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    PathBuf::from(format!("round-{:03}-{safe}.json", round + 1))
}
