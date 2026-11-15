//! Ratings across games.
//!
//! A [`Ladder`] keeps a rating per player name. After each game every pair
//! of rated players counts as one Elo match that the better-placed player
//! won, scaled so a whole game moves a rating about as much as one
//! head-to-head match would. Bots are left out. A [`Hub`](crate::hub::Hub)
//! keeps one with [`Hub::keep_ladder`](crate::hub::Hub::keep_ladder).
//!
//! Names are not accounts: anyone can join under any name. A ladder suits a
//! community server where people know each other, not prize play.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Where everyone starts.
pub const STARTING_RATING: i32 = 1500;

/// How far one game can move a rating, at most.
const K: f64 = 32.0;

/// One player's standing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rating {
    pub rating: i32,
    pub games: u32,
    /// Games won outright.
    pub wins: u32,
    /// Games finished in the top half.
    pub top_half: u32,
}

impl Default for Rating {
    fn default() -> Self {
        Rating { rating: STARTING_RATING, games: 0, wins: 0, top_half: 0 }
    }
}

/// One row of the standings, as sent to clients.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    pub name: String,
    #[serde(flatten)]
    pub rating: Rating,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ladder {
    players: BTreeMap<String, Rating>,
}

impl Ladder {
    pub fn new() -> Self {
        Ladder::default()
    }

    /// A player's standing; the starting one if they have not played.
    pub fn rating(&self, name: &str) -> Rating {
        self.players.get(name).cloned().unwrap_or_default()
    }

    /// Records a finished game from its results: each player's name and
    /// final place (1 is the winner). Places may tie. Fewer than two players
    /// change nothing.
    pub fn record(&mut self, results: &[(String, u32)]) {
        let n = results.len();
        if n < 2 {
            return;
        }
        let before: Vec<f64> = results.iter().map(|(name, _)| self.rating(name).rating as f64).collect();
        let k = K / (n - 1) as f64;
        let mut change = vec![0.0; n];
        for a in 0..n {
            for b in 0..n {
                if a == b {
                    continue;
                }
                let expected = 1.0 / (1.0 + 10f64.powf((before[b] - before[a]) / 400.0));
                let score = match results[a].1.cmp(&results[b].1) {
                    std::cmp::Ordering::Less => 1.0,
                    std::cmp::Ordering::Equal => 0.5,
                    std::cmp::Ordering::Greater => 0.0,
                };
                change[a] += k * (score - expected);
            }
        }
        for (i, (name, place)) in results.iter().enumerate() {
            let entry = self.players.entry(name.clone()).or_default();
            entry.rating = (before[i] + change[i]).round() as i32;
            entry.games += 1;
            entry.wins += (*place == 1) as u32;
            entry.top_half += (*place as usize * 2 <= n) as u32;
        }
    }

    /// Everyone, best rating first (ties by name).
    pub fn standings(&self) -> Vec<Standing> {
        let mut rows: Vec<Standing> =
            self.players.iter().map(|(name, rating)| Standing { name: name.clone(), rating: rating.clone() }).collect();
        rows.sort_by(|a, b| b.rating.rating.cmp(&a.rating.rating).then_with(|| a.name.cmp(&b.name)));
        rows
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, path)
    }

    /// Loads a ladder file; a file that does not exist yet is an empty ladder.
    pub fn load(path: &Path) -> Result<Ladder, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ladder::new()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}
