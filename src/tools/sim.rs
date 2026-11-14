//! The balance simulator behind `autoengine sim`.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::bot;
use crate::combat::Team;
use crate::content::ContentSet;
use crate::game::{Game, Opponent};
use crate::rng::Rng;

/// How one unit did across simulated games.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct UnitRecord {
    /// Player-versus-player fights the unit was on the board for.
    pub fights: u32,
    pub wins: u32,
    /// Times the unit was on a player's board when that player's game ended.
    pub final_boards: u32,
    /// Sum of those players' final placements (1 is the winner).
    pub placement_sum: u32,
}

impl UnitRecord {
    pub fn win_percent(&self) -> u32 {
        (self.wins * 100).checked_div(self.fights).unwrap_or(0)
    }

    /// Average final placement, times ten (25 means 2.5).
    pub fn placement_tenths(&self) -> u32 {
        (self.placement_sum * 10).checked_div(self.final_boards).unwrap_or(0)
    }
}

/// The result of [`simulate`].
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct SimReport {
    pub games: u32,
    /// Games that did not finish within the round limit.
    pub unfinished: u32,
    pub rounds: u32,
    pub fights: u32,
    /// Fights that ran out of time without a winner.
    pub draws: u32,
    pub fight_ms: u64,
    /// By unit id.
    pub units: BTreeMap<String, UnitRecord>,
    /// How often each trait was active on a board entering a PvP fight, and
    /// how often that board won.
    pub traits: BTreeMap<String, (u32, u32)>,
}

/// Plays whole games between bots and tallies how units and traits did.
/// The same arguments always give the same report.
pub fn simulate(content: impl Into<Arc<ContentSet>>, games: u32, players: usize, seed: u64) -> SimReport {
    const NAMES: [&str; 8] = ["A", "B", "C", "D", "E", "F", "G", "H"];
    let content = content.into();
    let mut report = SimReport { games, ..Default::default() };
    for game_index in 0..games {
        let game_seed = seed.wrapping_add(game_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut game = Game::new(content.clone(), game_seed, &NAMES[..players.clamp(2, 8)]);
        let mut bot_rng = Rng::new(game_seed ^ 0xB07);
        // Boards as they were the last time each player fought.
        let mut last_board: BTreeMap<u8, Vec<String>> = BTreeMap::new();
        while !game.is_over() {
            if game.round_index() >= 300 || bot::play_round(&mut game, &mut bot_rng).is_err() {
                report.unfinished += 1;
                break;
            }
            report.rounds += 1;
            for record in game.combats() {
                if record.teams.len() < 2 {
                    continue;
                }
                let sides = [Some(record.home), match record.away {
                    Opponent::Player(p) => Some(p),
                    _ => None,
                }];
                for (side, owner) in sides.iter().enumerate() {
                    if let Some(owner) = owner {
                        last_board.insert(owner.0, record.teams[side].units.iter().map(|u| u.unit.clone()).collect());
                    }
                }
                if record.away == Opponent::Neutral {
                    continue;
                }
                report.fights += 1;
                report.fight_ms += record.duration_ms as u64;
                if record.winner.is_none() {
                    report.draws += 1;
                }
                for (side, team) in record.teams.iter().take(2).enumerate() {
                    let won = record.winner == Some(Team(side as u8));
                    let mut seen: Vec<&str> = team.units.iter().map(|u| u.unit.as_str()).collect();
                    seen.sort();
                    seen.dedup();
                    for unit in seen {
                        let entry = report.units.entry(unit.to_string()).or_default();
                        entry.fights += 1;
                        entry.wins += won as u32;
                    }
                    for active in content.active_traits(&team.units).unwrap_or_default() {
                        if active.tier.is_some() {
                            let entry = report.traits.entry(active.id.clone()).or_default();
                            entry.0 += 1;
                            entry.1 += won as u32;
                        }
                    }
                }
            }
            game.drain_events();
        }
        for player in game.players() {
            let placement = player.placement.unwrap_or(1);
            let mut board = last_board.remove(&player.id.0).unwrap_or_default();
            board.sort();
            board.dedup();
            for unit in board {
                let entry = report.units.entry(unit).or_default();
                entry.final_boards += 1;
                entry.placement_sum += placement;
            }
        }
    }
    report
}

impl SimReport {
    /// The report as a table for the terminal.
    pub fn to_text(&self, content: &ContentSet) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let finished = self.games - self.unfinished;
        let _ = writeln!(out, "{} games ({} finished), {:.1} rounds each", self.games, finished, self.rounds as f64 / self.games.max(1) as f64);
        let _ = writeln!(
            out,
            "{} player fights, {:.1}s on average, {} draws",
            self.fights,
            self.fight_ms as f64 / 1000.0 / self.fights.max(1) as f64,
            self.draws
        );
        let _ = writeln!(out, "\n{:<26} {:>4} {:>7} {:>6} {:>10}", "unit", "cost", "fights", "win %", "avg place");
        let mut units: Vec<(&String, &UnitRecord)> = self.units.iter().collect();
        units.sort_by(|a, b| b.1.win_percent().cmp(&a.1.win_percent()).then(a.0.cmp(b.0)));
        for (id, record) in units {
            let def = content.unit(id);
            let place = record.placement_tenths();
            let _ = writeln!(
                out,
                "{:<26} {:>4} {:>7} {:>6} {:>8}.{}",
                def.map_or(id.as_str(), |d| d.name.as_str()),
                def.map_or(0, |d| d.cost),
                record.fights,
                record.win_percent(),
                place / 10,
                place % 10
            );
        }
        let _ = writeln!(out, "\n{:<26} {:>7} {:>6}", "active trait", "fights", "win %");
        let mut traits: Vec<(&String, &(u32, u32))> = self.traits.iter().collect();
        traits.sort_by(|a, b| (b.1.1 * 100 / b.1.0.max(1)).cmp(&(a.1.1 * 100 / a.1.0.max(1))).then(a.0.cmp(b.0)));
        for (id, (fights, wins)) in traits {
            let name = content.trait_def(id).map_or(id.as_str(), |t| t.name.as_str());
            let _ = writeln!(out, "{:<26} {:>7} {:>6}", name, fights, wins * 100 / fights.max(&1));
        }
        out
    }
}
