//! Whole matches saved to files.
//!
//! A game is fully described by its content, its seed, its players' names and
//! the list of [`Input`]s that moved it along. A [`MatchLog`] holds the last
//! three, so a match can be:
//!
//! * **watched again** from the start, round by round,
//! * **checked**: replaying it must give the same [`Game::checksum`], which
//!   catches anything in the engine or the content that changed behaviour,
//! * **resumed**: a server that stops mid-game picks up where it was with
//!   [`crate::session::Session::resume`].
//!
//! ```
//! use autoengine::prelude::*;
//! use autoengine::matchlog::MatchLog;
//!
//! let content = std::sync::Arc::new(autoengine::sample::content());
//! let mut game = Game::new(content.clone(), 3, &["Ann", "Bo"]);
//! let mut rng = Rng::new(1);
//! for _ in 0..3 {
//!     bot::play_round(&mut game, &mut rng).unwrap();
//! }
//! let log = MatchLog::of(&game);
//! let again = log.replay(content, |_| {}).unwrap();
//! assert_eq!(again.checksum(), game.checksum());
//! ```

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::content::ContentSet;
use crate::game::{Game, Input};
use crate::protocol::ContentInfo;

pub const MATCH_LOG_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchLog {
    pub version: u32,
    pub seed: u64,
    pub names: Vec<String>,
    /// A fingerprint of the content the match was played with, so a replay
    /// with different content is caught before it goes wrong.
    pub content: u64,
    pub inputs: Vec<Input>,
    /// [`Game::checksum`] when the log was taken.
    pub checksum: u64,
}

/// Why a match log could not be replayed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatchLogError {
    Version { found: u32 },
    /// The content is not the content the match was played with.
    Content,
    /// An input was refused; the engine or the setup behaves differently now.
    Refused { input: usize, error: String },
    /// Every input was accepted, but the game ended up in another state.
    Diverged,
}

impl std::fmt::Display for MatchLogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchLogError::Version { found } => {
                write!(f, "match log format {found}, this build reads {MATCH_LOG_VERSION}")
            }
            MatchLogError::Content => write!(f, "the match was played with different content"),
            MatchLogError::Refused { input, error } => write!(f, "input {input} was refused on replay: {error}"),
            MatchLogError::Diverged => write!(f, "the replayed game ended in a different state"),
        }
    }
}

impl std::error::Error for MatchLogError {}

/// A fingerprint of a content set: everything a client is sent about it.
pub fn content_fingerprint(content: &ContentSet) -> u64 {
    let mut info = ContentInfo::from_content(content);
    // Translations change no rules, so they do not count.
    info.translations.clear();
    let mut bytes = serde_json::to_vec(&info).expect("content serializes");
    // Scripts change how a game plays, so they count too.
    #[cfg(feature = "scripting")]
    bytes.extend(serde_json::to_vec(content.scripts()).expect("scripts serialize"));
    bytes.extend(serde_json::to_vec(content.schedule()).expect("schedules serialize"));
    crate::rng::fnv1a(&bytes)
}

impl MatchLog {
    /// Everything needed to play `game` again up to where it is now.
    pub fn of(game: &Game) -> MatchLog {
        MatchLog {
            version: MATCH_LOG_VERSION,
            seed: game.seed(),
            names: game.players().iter().map(|p| p.name.clone()).collect(),
            content: content_fingerprint(game.content()),
            inputs: game.inputs().to_vec(),
            checksum: game.checksum(),
        }
    }

    /// A fresh game at the start of the match, after `setup` has run on it.
    /// `setup` must do what was done to the original when it was created:
    /// register the same round types and hooks.
    pub fn start(
        &self,
        content: impl Into<Arc<ContentSet>>,
        setup: impl FnOnce(&mut Game),
    ) -> Result<Game, MatchLogError> {
        if self.version != MATCH_LOG_VERSION {
            return Err(MatchLogError::Version { found: self.version });
        }
        let content = content.into();
        if content_fingerprint(&content) != self.content {
            return Err(MatchLogError::Content);
        }
        let names: Vec<&str> = self.names.iter().map(String::as_str).collect();
        let mut game = Game::new(content, self.seed, &names);
        setup(&mut game);
        Ok(game)
    }

    /// Applies one input to a game.
    pub fn step(game: &mut Game, input: &Input) -> Result<(), String> {
        let result = match input {
            Input::BeginRound => game.begin_round(),
            Input::FinishRound => game.finish_round(),
            Input::Command { player, command } => game.apply(*player, command.clone()),
            Input::Grant { player, grant } => game.grant(*player, grant.clone()),
        };
        result.map_err(|e| e.to_string())
    }

    /// Plays the whole match again and checks it ends where the original
    /// did.
    pub fn replay(
        &self,
        content: impl Into<Arc<ContentSet>>,
        setup: impl FnOnce(&mut Game),
    ) -> Result<Game, MatchLogError> {
        let mut game = self.start(content, setup)?;
        for (i, input) in self.inputs.iter().enumerate() {
            Self::step(&mut game, input).map_err(|error| MatchLogError::Refused { input: i, error })?;
        }
        if game.checksum() != self.checksum {
            return Err(MatchLogError::Diverged);
        }
        Ok(game)
    }

    /// The match as it stood after each round: the game is handed to
    /// `visit` after every [`Input::FinishRound`], and once more at the end
    /// if the log stops mid-round.
    pub fn rounds(
        &self,
        content: impl Into<Arc<ContentSet>>,
        setup: impl FnOnce(&mut Game),
        mut visit: impl FnMut(&Game),
    ) -> Result<(), MatchLogError> {
        let mut game = self.start(content, setup)?;
        for (i, input) in self.inputs.iter().enumerate() {
            Self::step(&mut game, input).map_err(|error| MatchLogError::Refused { input: i, error })?;
            if *input == Input::FinishRound {
                visit(&game);
            }
        }
        if self.inputs.last() != Some(&Input::FinishRound) {
            visit(&game);
        }
        Ok(())
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string(self).map_err(std::io::Error::other)?;
        // Write then rename, so a crash never leaves half a file behind.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, path)
    }

    pub fn load(path: &Path) -> Result<MatchLog, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}
