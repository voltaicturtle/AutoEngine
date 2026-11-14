//! Fights with an expected result, behind `autoengine test`.
//!
//! A game lists fights and what is expected of them. They pin down what the
//! content is supposed to do, so a later change that breaks it is caught.
//! They run from the command line and work just as well in a game's own
//! `cargo test`:
//!
//! ```
//! use autoengine::content::{BoardUnit, TeamSetup};
//! use autoengine::tools::scenario::{Expect, Scenario, run_scenario};
//!
//! let test = Scenario::new(
//!     "two stars beat one star",
//!     [
//!         TeamSetup::new().unit(BoardUnit::new("ironbark_warden", (3, 4)).star(2)),
//!         TeamSetup::new().unit(BoardUnit::new("ironbark_warden", (3, 4))),
//!     ],
//! )
//! .runs(5)
//! .expect([Expect::Winner(0), Expect::Survives(0, "ironbark_warden".into())]);
//! assert!(run_scenario(&autoengine::sample::content(), &test).passed());
//! ```

use serde::{Deserialize, Serialize};

use crate::combat::{Combat, EventKind, Team};
use crate::content::{ContentSet, TeamSetup};

/// Something a [`Scenario`] expects of its fight. Teams are numbered from 0
/// in the order they are listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expect {
    /// This team wins every run.
    Winner(u8),
    /// This team wins at least this share of the runs.
    WinsAtLeast { team: u8, percent: u32 },
    /// Nobody wins in any run.
    Draw,
    /// Every run is over within this many seconds.
    LastsAtMost { seconds: u32 },
    /// Every run goes on for at least this many seconds.
    LastsAtLeast { seconds: u32 },
    /// A unit with this id on this team is alive at the end of every run.
    Survives(u8, String),
    /// Every unit with this id on this team dies in every run.
    Dies(u8, String),
    /// A unit with this id on this team casts its ability in every run.
    Casts(u8, String),
}

fn default_runs() -> u32 {
    1
}

fn default_seed() -> u64 {
    1
}

/// One fight and what is expected of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub teams: Vec<TeamSetup>,
    /// How many times to fight, each with different dice.
    #[serde(default = "default_runs")]
    pub runs: u32,
    /// The seed of the first run; later runs count up from it.
    #[serde(default = "default_seed")]
    pub seed: u64,
    pub expect: Vec<Expect>,
}

impl Scenario {
    /// One run with seed 1 and no expectations yet.
    pub fn new(name: impl Into<String>, teams: impl IntoIterator<Item = TeamSetup>) -> Self {
        Scenario { name: name.into(), teams: teams.into_iter().collect(), runs: 1, seed: 1, expect: Vec::new() }
    }

    /// How many times to fight, each with different dice.
    pub fn runs(mut self, runs: u32) -> Self {
        self.runs = runs;
        self
    }

    /// The seed of the first run; later runs count up from it.
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn expect(mut self, expect: impl IntoIterator<Item = Expect>) -> Self {
        self.expect.extend(expect);
        self
    }
}

/// How one scenario went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScenarioResult {
    pub name: String,
    pub runs: u32,
    /// One line for each expectation that was not met. Empty means passed.
    pub failures: Vec<String>,
}

impl ScenarioResult {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// What one run of a fight came to, in the terms expectations use.
struct Run {
    seed: u64,
    winner: Option<Team>,
    seconds: u32,
    combat: Combat,
}

impl Run {
    /// Units on `team` whose template is called `name`.
    fn units<'a>(&'a self, team: u8, name: &'a str) -> impl Iterator<Item = &'a crate::combat::CombatUnit> {
        self.combat.units().iter().filter(move |u| u.team() == Team(team) && u.name() == name)
    }
}

/// Fights one scenario and checks its expectations.
pub fn run_scenario(content: &ContentSet, scenario: &Scenario) -> ScenarioResult {
    let mut result = ScenarioResult { name: scenario.name.clone(), runs: scenario.runs.max(1), failures: Vec::new() };
    let mut runs = Vec::new();
    for i in 0..result.runs {
        let seed = scenario.seed.wrapping_add(i as u64);
        match content.setup_combat(seed, &scenario.teams) {
            Ok(mut combat) => {
                let outcome = combat.run().clone();
                runs.push(Run { seed, winner: outcome.winner, seconds: outcome.duration_ms / 1000, combat });
            }
            Err(e) => {
                result.failures.push(format!("the fight could not be set up: {e}"));
                return result;
            }
        }
    }

    // The display name of a unit id, which is what fighting units carry.
    let name_of = |id: &str| content.unit(id).map(|u| u.name.clone());
    let who = |winner: Option<Team>| winner.map_or("nobody".to_string(), |t| format!("team {}", t.0));
    for expect in &scenario.expect {
        // The first run that breaks the expectation, described.
        let mut broken: Option<String> = None;
        let mut fail = |run: &Run, what: String| {
            if broken.is_none() {
                broken = Some(format!("{what} (seed {})", run.seed));
            }
        };
        match expect {
            Expect::Winner(team) => {
                for run in runs.iter().filter(|r| r.winner != Some(Team(*team))) {
                    fail(run, format!("expected team {team} to win, but {} won", who(run.winner)));
                }
            }
            Expect::WinsAtLeast { team, percent } => {
                let wins = runs.iter().filter(|r| r.winner == Some(Team(*team))).count() as u32;
                let got = wins * 100 / result.runs;
                if got < *percent {
                    result.failures.push(format!(
                        "expected team {team} to win at least {percent}% of runs, but it won {got}% ({wins} of {})",
                        result.runs
                    ));
                }
            }
            Expect::Draw => {
                for run in runs.iter().filter(|r| r.winner.is_some()) {
                    fail(run, format!("expected a draw, but {} won", who(run.winner)));
                }
            }
            Expect::LastsAtMost { seconds } => {
                for run in runs.iter().filter(|r| r.seconds > *seconds) {
                    fail(run, format!("expected the fight to end within {seconds}s, but it took {}s", run.seconds));
                }
            }
            Expect::LastsAtLeast { seconds } => {
                for run in runs.iter().filter(|r| r.seconds < *seconds) {
                    fail(run, format!("expected the fight to last {seconds}s, but it ended after {}s", run.seconds));
                }
            }
            Expect::Survives(team, id) | Expect::Dies(team, id) | Expect::Casts(team, id) => {
                let Some(name) = name_of(id) else {
                    result.failures.push(format!("there is no unit with the id \"{id}\""));
                    continue;
                };
                for run in &runs {
                    if run.units(*team, &name).next().is_none() {
                        fail(run, format!("team {team} has no {id}"));
                        continue;
                    }
                    match expect {
                        Expect::Survives(..) if !run.units(*team, &name).any(|u| u.is_alive()) => {
                            fail(run, format!("expected {id} on team {team} to survive, but it died"));
                        }
                        Expect::Dies(..) if run.units(*team, &name).any(|u| u.is_alive()) => {
                            fail(run, format!("expected {id} on team {team} to die, but it survived"));
                        }
                        Expect::Casts(..) => {
                            let cast = run.combat.events().iter().any(|e| {
                                matches!(&e.kind, EventKind::CastStarted { unit }
                                    if run.units(*team, &name).any(|u| u.id() == *unit))
                            });
                            if !cast {
                                fail(run, format!("expected {id} on team {team} to cast, but it never did"));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        result.failures.extend(broken);
    }
    result
}

/// Runs every scenario.
pub fn run_all(content: &ContentSet, scenarios: &[Scenario]) -> Vec<ScenarioResult> {
    scenarios.iter().map(|s| run_scenario(content, s)).collect()
}
