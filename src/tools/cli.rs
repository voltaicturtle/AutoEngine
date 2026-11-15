//! The developer command line: everything a game's developers do without
//! opening the client, for a game defined in Rust.
//!
//! A game hands its content to [`run`] from its own `main`, and gets every
//! command: `check`, `test`, `fight`, `lobby`, `sim`, `bench`, `serve`,
//! `replay`, `api` and `new`. The engine's own `autoengine` binary does this
//! for the sample game.
//!
//! ```no_run
//! use autoengine::tools::cli::{self, GameSpec};
//!
//! fn main() -> std::process::ExitCode {
//!     cli::run(&GameSpec::new("sample", autoengine::sample::content))
//! }
//! ```
//!
//! Each command is a small function in this file built on the library's
//! public API, so it doubles as an example of using it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use crate::bot;
use crate::combat::{Combat, EventKind, Team, UnitId};
use crate::content::{ContentSet, MatchSetup};
use crate::game::{Game, GameEvent, Opponent, PlayerId};
use crate::net::{Server, ServerConfig};
use crate::rng::Rng;
use crate::session::{Session, SessionConfig};
use crate::stats::Stat;
use crate::tools;
use crate::tools::scenario::Scenario;

/// A game, as the command line needs it. Everything is code: the content,
/// the fights it is tested with, and an optional folder of art to check.
#[derive(Clone, Debug)]
pub struct GameSpec {
    pub name: String,
    pub content: fn() -> ContentSet,
    /// The fight `fight` and `bench` use when no test is named.
    pub sample_match: Option<fn() -> MatchSetup>,
    /// Fights with expected results, run by `test`.
    pub tests: fn() -> Vec<Scenario>,
    /// Where model and icon files are, so `check` can confirm they exist.
    pub assets: Option<PathBuf>,
}

impl GameSpec {
    pub fn new(name: impl Into<String>, content: fn() -> ContentSet) -> Self {
        GameSpec { name: name.into(), content, sample_match: None, tests: Vec::new, assets: None }
    }

    pub fn sample_match(mut self, f: fn() -> MatchSetup) -> Self {
        self.sample_match = Some(f);
        self
    }

    pub fn tests(mut self, f: fn() -> Vec<Scenario>) -> Self {
        self.tests = f;
        self
    }

    pub fn assets(mut self, dir: impl Into<PathBuf>) -> Self {
        self.assets = Some(dir.into());
        self
    }

    /// Builds the content, catching a panic from an `expect` in the
    /// game's own code so the command can report it.
    fn load(&self) -> Result<ContentSet, String> {
        std::panic::catch_unwind(self.content).map_err(|e| {
            let message = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "building the content panicked".into());
            format!("error: {message}")
        })
    }
}

const BOT_NAMES: [&str; 8] = ["Aldra", "Brask", "Cyrene", "Dovik", "Elowen", "Fenn", "Garrick", "Hesper"];

/// One subcommand: its name, its arguments, what it does and the function
/// that runs it.
struct Command {
    name: &'static str,
    args: &'static str,
    about: &'static str,
    run: fn(&GameSpec, &mut Args) -> Result<(), String>,
}

const COMMANDS: &[Command] = &[
    Command { name: "new", args: "DIR", about: "write a small starter game (a Rust crate) to build on", run: new },
    Command { name: "check", args: "", about: "look for mistakes in the content", run: check },
    Command { name: "test", args: "[--filter TEXT]", about: "run the content tests and check their results", run: test },
    Command { name: "fight", args: "[SEED] [--quiet] [--test NAME]", about: "run one fight and print what happens", run: fight },
    Command { name: "lobby", args: "[SEED] [--players N] [--bots easy|normal|hard|mixed] [--quiet] [--save FILE]", about: "bots play a whole game", run: lobby },
    Command { name: "sim", args: "[--games N] [--players N] [--seed N] [--bots LEVEL] [--json]", about: "bots play many games; prints a balance table", run: sim },
    Command { name: "bench", args: "[--fights N] [--games N]", about: "measure how fast fights and games run", run: bench },
    Command {
        name: "serve",
        args: "[--port 7777] [--bind ADDR] [--seats 8] [--seed N] [--planning-secs 30] [--record DIR [--resume]] [--dev] [--bots LEVEL] [--rooms N [--ladder FILE]] [--spectator-delay-secs N] \
               [--max-connections 64] [--max-per-ip 8] [--idle-secs 30] [--stats-secs N]",
        about: "host a game (or with --rooms, up to N games at once) for clients to join over TCP",
        run: serve,
    },
    Command { name: "replay", args: "FILE...", about: "fight saved replays again and report any that changed", run: replay },
    Command {
        name: "match",
        args: "FILE [--rounds]",
        about: "play a saved match again, check it ends the same, and summarize it",
        run: match_log,
    },
    Command { name: "api", args: "", about: "print what scripts can do", run: api },
];

fn help(spec: &GameSpec) -> String {
    let mut out = format!("usage: COMMAND [ARGS]   (game: {})\n\n", spec.name);
    for c in COMMANDS {
        out.push_str(&format!("  {:<7} {}\n          {} {}\n", c.name, c.about, c.name, c.args));
    }
    out
}

/// The arguments after the subcommand. Commands take out the flags they
/// know; anything left over is a mistake.
struct Args {
    command: &'static Command,
    rest: Vec<String>,
}

impl Args {
    fn usage(&self) -> String {
        format!("usage: {} {}", self.command.name, self.command.args)
    }

    /// Whether a flag such as `--quiet` was given.
    fn flag(&mut self, name: &str) -> bool {
        let found = self.rest.iter().position(|a| a == name);
        found.map(|i| self.rest.remove(i)).is_some()
    }

    /// The value after a flag such as `--seats 4`.
    fn value(&mut self, name: &str) -> Result<Option<String>, String> {
        let Some(i) = self.rest.iter().position(|a| a == name) else { return Ok(None) };
        if i + 1 >= self.rest.len() {
            return Err(format!("{name} needs a value\n{}", self.usage()));
        }
        self.rest.remove(i);
        Ok(Some(self.rest.remove(i)))
    }

    fn number(&mut self, name: &str, default: u64, range: std::ops::RangeInclusive<u64>) -> Result<u64, String> {
        match self.value(name)? {
            None => Ok(default),
            Some(text) => text
                .parse()
                .ok()
                .filter(|n| range.contains(n))
                .ok_or(format!("{name} must be a number from {} to {}", range.start(), range.end())),
        }
    }

    /// The next argument that is not a flag.
    fn positional(&mut self) -> Option<String> {
        let i = self.rest.iter().position(|a| !a.starts_with("--"))?;
        Some(self.rest.remove(i))
    }

    /// Fails if anything was not understood.
    fn done(&self) -> Result<(), String> {
        match self.rest.first() {
            None => Ok(()),
            Some(extra) => Err(format!("did not understand \"{extra}\"\n{}", self.usage())),
        }
    }
}

/// Runs the command named by the process's arguments for a game.
pub fn run(spec: &GameSpec) -> ExitCode {
    run_args(spec, std::env::args().skip(1).collect())
}

/// Runs a command given as arguments, such as `["sim", "--games", "10"]`.
pub fn run_args(spec: &GameSpec, args: Vec<String>) -> ExitCode {
    let mut args = args.into_iter();
    let name = args.next().unwrap_or_else(|| "help".to_string());
    if matches!(name.as_str(), "help" | "--help" | "-h") {
        println!("{}", help(spec));
        return ExitCode::SUCCESS;
    }
    let Some(command) = COMMANDS.iter().find(|c| c.name == name) else {
        eprintln!("there is no command called \"{name}\"\n\n{}", help(spec));
        return ExitCode::from(2);
    };
    let mut args = Args { command, rest: args.collect() };
    match (command.run)(spec, &mut args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}

fn new(_spec: &GameSpec, args: &mut Args) -> Result<(), String> {
    let dir = args.positional().ok_or_else(|| args.usage())?;
    args.done()?;
    let written = tools::scaffold::write_starter(Path::new(&dir))?;
    println!("created {dir} with {} files:", written.len());
    for file in &written {
        println!("  {file}");
    }
    println!("\nNext: read {dir}/README.md, then try\n  cd {dir}\n  cargo run -- check\n  cargo run -- test");
    Ok(())
}

/// Builds the content and prints errors and likely mistakes.
fn check(spec: &GameSpec, args: &mut Args) -> Result<(), String> {
    args.done()?;
    // Mistakes that stop the content building are errors; warnings are not.
    let content = spec.load()?;
    println!(
        "{}: {} units, {} traits, {} items, {} augments, {} rounds, {} scripts",
        spec.name,
        content.units().count(),
        content.traits().count(),
        content.items().count(),
        content.augments().count(),
        content.schedule().rounds.len(),
        content.scripts().len()
    );
    let warnings = tools::lint(&content, spec.assets.as_deref());
    for warning in &warnings {
        println!("warning: {warning}");
    }
    println!("{}", if warnings.is_empty() { "no problems found".to_string() } else { format!("{} warnings", warnings.len()) });
    Ok(())
}

/// Runs the game's content tests. `Ok(true)` means all passed.
fn run_tests(spec: &GameSpec, filter: Option<&str>) -> Result<bool, String> {
    let content = spec.load()?;
    let mut scenarios = (spec.tests)();
    if scenarios.is_empty() {
        println!("{} has no content tests", spec.name);
        return Ok(true);
    }
    if let Some(filter) = filter {
        scenarios.retain(|t| t.name.contains(filter));
    }
    let results = tools::scenario::run_all(&content, &scenarios);
    for result in &results {
        println!("{}  {} ({} runs)", if result.passed() { "ok    " } else { "FAILED" }, result.name, result.runs);
        for failure in &result.failures {
            println!("          {failure}");
        }
    }
    let failed = results.iter().filter(|r| !r.passed()).count();
    println!("{} tests, {} passed, {failed} failed", results.len(), results.len() - failed);
    Ok(failed == 0)
}

fn test(spec: &GameSpec, args: &mut Args) -> Result<(), String> {
    let filter = args.value("--filter")?;
    args.done()?;
    if run_tests(spec, filter.as_deref())? { Ok(()) } else { Err("some tests failed".to_string()) }
}

fn sim(spec: &GameSpec, args: &mut Args) -> Result<(), String> {
    let games = args.number("--games", 50, 1..=100_000)? as u32;
    let players = args.number("--players", 8, 2..=8)? as usize;
    let seed = args.number("--seed", 1, 0..=u64::MAX)?;
    let json = args.flag("--json");
    let bots: bot::Difficulty = args.value("--bots")?.map(|b| b.parse()).transpose()?.unwrap_or_default();
    args.done()?;
    let content = Arc::new(spec.load()?);
    let report = tools::sim::simulate_with(content.clone(), games, players, seed, bots);
    if json {
        println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
    } else {
        print!("{}", report.to_text(&content));
    }
    Ok(())
}

fn bench(spec: &GameSpec, args: &mut Args) -> Result<(), String> {
    let fights = args.number("--fights", 200, 1..=1_000_000)? as u32;
    let games = args.number("--games", 5, 0..=10_000)? as u32;
    args.done()?;
    let content = Arc::new(spec.load()?);
    if cfg!(debug_assertions) {
        println!("note: this is a debug build; use `cargo run --release -- bench` for real numbers");
    }
    match spec.sample_match {
        Some(sample_match) => {
            let setup = sample_match();
            let t = tools::bench::fights(&content, &setup, fights)?;
            println!(
                "fights: {} in {:.2}s, {:.0} per second, {} microseconds each, {} ticks each",
                t.count,
                t.elapsed.as_secs_f64(),
                t.per_second(),
                t.micros_each(),
                t.steps / t.count.max(1) as u64
            );
        }
        None => println!("fights: skipped, the game has no sample match"),
    }
    if games > 0 {
        let t = tools::bench::games(content, games, 8);
        println!(
            "games:  {} in {:.2}s, {:.1} per second, {} rounds each",
            t.count,
            t.elapsed.as_secs_f64(),
            t.per_second(),
            t.steps / t.count.max(1) as u64
        );
    }
    Ok(())
}

fn api(_spec: &GameSpec, args: &mut Args) -> Result<(), String> {
    args.done()?;
    #[cfg(feature = "scripting")]
    print!("{}", crate::script::api_reference());
    #[cfg(not(feature = "scripting"))]
    println!("this build of the engine has scripting turned off");
    Ok(())
}

/// Plays a saved match again and prints how each round left the players.
fn match_log(spec: &GameSpec, args: &mut Args) -> Result<(), String> {
    let rounds = args.flag("--rounds");
    let file = args.positional().ok_or_else(|| args.usage())?;
    args.done()?;
    let path = Path::new(&file);
    // Either a bare match log or what a server saves with --record.
    let log = match crate::matchlog::MatchLog::load(path) {
        Ok(log) => log,
        Err(_) => {
            let dir = if path.is_dir() { path } else { path.parent().unwrap_or(Path::new(".")) };
            Session::load_match(dir)?.log
        }
    };
    let content = Arc::new(spec.load()?);
    if rounds {
        let mut n = 0;
        log.rounds(content.clone(), |_| {}, |game| {
            n += 1;
            let line: Vec<String> = game
                .players()
                .iter()
                .map(|p| if p.alive { format!("{} {}hp {}g L{}", p.name, p.health, p.gold, p.level) } else { format!("{} out", p.name) })
                .collect();
            println!("round {n:>2}: {}", line.join(" | "));
        })
        .map_err(|e| format!("{file}: {e}"))?;
    }
    let game = log.replay(content, |_| {}).map_err(|e| format!("{file}: {e}"))?;
    let mut standings: Vec<_> = game.players().iter().collect();
    standings.sort_by_key(|p| (p.placement.unwrap_or(0) == 0, p.placement, std::cmp::Reverse(p.health)));
    println!("ok: {} inputs replay to the same game (seed {})", log.inputs.len(), log.seed);
    for p in standings {
        match p.placement {
            Some(place) => println!("  #{place} {}", p.name),
            None => println!("  -- {} ({} health)", p.name, p.health),
        }
    }
    Ok(())
}

/// Re-runs saved fights and reports whether the engine still plays them
/// out exactly as recorded.
fn replay(_spec: &GameSpec, args: &mut Args) -> Result<(), String> {
    let files = std::mem::take(&mut args.rest);
    if files.is_empty() {
        return Err(args.usage());
    }
    let mut failed = 0;
    for file in &files {
        let replay = crate::replay::ReplayFile::load(Path::new(file))?;
        let record = &replay.record;
        let result = match record.winner {
            Some(team) => format!("team {} wins", team.0 + 1),
            None => "draw".to_string(),
        };
        let summary = format!(
            "{}: {}, {:.1}s, {} events",
            replay.title,
            result,
            record.duration_ms as f64 / 1000.0,
            record.events.len()
        );
        match replay.verify() {
            Ok(()) => println!("ok       {file}  ({summary})"),
            Err(e) => {
                failed += 1;
                println!("MISMATCH {file}  ({summary}): {e}");
            }
        }
    }
    if failed > 0 { Err(format!("{failed} of {} replays no longer match", files.len())) } else { Ok(()) }
}

fn lobby(spec: &GameSpec, args: &mut Args) -> Result<(), String> {
    let quiet = args.flag("--quiet");
    let players = args.number("--players", 8, 2..=8)? as usize;
    let save = args.value("--save")?;
    let bots = args.value("--bots")?.unwrap_or_else(|| "normal".into());
    let levels: Vec<bot::Difficulty> = if bots == "mixed" {
        (0..players).map(|i| bot::Difficulty::ALL[i % 3]).collect()
    } else {
        vec![bots.parse()?; players]
    };
    let seed = seed_argument(args)?;
    args.done()?;
    let game = play_lobby(spec.load()?, seed, &levels, quiet)?;
    if let Some(file) = save {
        crate::matchlog::MatchLog::of(&game).save(Path::new(&file)).map_err(|e| format!("{file}: {e}"))?;
        println!("saved the match to {file}; `match {file} --rounds` plays it again");
    }
    Ok(())
}

/// An optional seed given as a bare number.
fn seed_argument(args: &mut Args) -> Result<u64, String> {
    match args.positional() {
        None => Ok(1),
        Some(text) => text.parse().map_err(|_| format!("the seed must be a number, not \"{text}\"")),
    }
}

fn fight(spec: &GameSpec, args: &mut Args) -> Result<(), String> {
    let quiet = args.flag("--quiet");
    let test_name = args.value("--test")?;
    let seed = seed_argument(args)?;
    args.done()?;
    let content = spec.load()?;
    let setup = match test_name {
        // Watch the fight of one of the content tests.
        Some(name) => {
            let found = (spec.tests)().into_iter().find(|t| t.name.contains(&name));
            let scenario = found.ok_or(format!("no content test has \"{name}\" in its name"))?;
            println!("test: {}", scenario.name);
            MatchSetup { teams: scenario.teams }
        }
        None => match spec.sample_match {
            Some(sample_match) => sample_match(),
            None => return Err("the game has no sample match; name a content test with --test".into()),
        },
    };

    for (i, team) in setup.teams.iter().enumerate() {
        let traits = content.active_traits(&team.units).map_err(|e| e.to_string())?;
        let summary: Vec<String> = traits
            .iter()
            .map(|t| {
                let name = &content.trait_def(&t.id).unwrap().name;
                let state = if t.tier.is_some() { "active" } else { "inactive" };
                format!("{name} {} ({state})", t.count)
            })
            .collect();
        println!("team {i} traits: {}", summary.join(", "));
    }

    let mut combat = content.setup_combat(seed, &setup.teams).map_err(|e| e.to_string())?;
    combat.run();

    if !quiet {
        println!();
        print_log(&combat);
    }
    println!();

    let label = |id: UnitId| label(&combat, id);
    let outcome = combat.outcome().unwrap();
    println!(
        "seed {seed}: {} after {:.2}s ({} events)",
        match outcome.winner {
            Some(team) => format!("team {} wins", team.0),
            None => "draw".to_string(),
        },
        outcome.duration_ms as f64 / 1000.0,
        combat.events().len()
    );
    for id in &outcome.survivors {
        let u = combat.unit(*id);
        println!("  survivor: {} with {}/{} health", label(*id), u.health(), u.stat(Stat::MaxHealth));
    }
    Ok(())
}

/// Plays a full game between bots, printing a summary of every round.
fn play_lobby(content: ContentSet, seed: u64, levels: &[bot::Difficulty], quiet: bool) -> Result<Game, String> {
    let players = levels.len();
    let mut game = Game::new(content, seed, &BOT_NAMES[..players]);
    let mut bot_rng = Rng::new(seed ^ 0xB07);
    let name = |game: &Game, id: PlayerId| game.player(id).name.clone();

    while !game.is_over() {
        if game.round_index() >= 500 {
            return Err("the game did not finish within 500 rounds".into());
        }
        bot::play_round_with(&mut game, &mut bot_rng, |id| levels[id.0 as usize]).map_err(|e| e.to_string())?;
        for event in game.drain_events() {
            let line = match event {
                GameEvent::RoundStarted { index, name, kind, stage } => {
                    format!("\nRound {} ({name}, {kind}, stage {stage})", index + 1)
                }
                GameEvent::ChoiceMade { player, choice, .. } => {
                    format!("  {} picks {choice}", name(&game, player))
                }
                GameEvent::UnitUpgraded { player, unit, star, .. } => {
                    format!("  {} upgrades {unit} to {star} stars", name(&game, player))
                }
                GameEvent::CombatFinished { combat, home, away, winner } => {
                    let record = &game.combats()[combat];
                    let away = match away {
                        Opponent::Player(p) => name(&game, p),
                        Opponent::Ghost(p) => format!("a copy of {}", name(&game, p)),
                        Opponent::Neutral => "the neutral camp".to_string(),
                    };
                    let verb = match winner {
                        Some(Team(0)) => "beats",
                        Some(_) => "loses to",
                        None => "draws with",
                    };
                    format!("  {} {verb} {away} in {:.1}s", name(&game, home), record.duration_ms as f64 / 1000.0)
                }
                GameEvent::PlayerDamaged { player, amount, health } => {
                    format!("    {} takes {amount} damage ({health} left)", name(&game, player))
                }
                GameEvent::PlayerEliminated { player, placement } => {
                    format!("  {} is eliminated in place {placement}", name(&game, player))
                }
                GameEvent::GameOver { winner } => format!("\n{} wins the game", name(&game, winner)),
                GameEvent::Script { script, message } => format!("  [{script}] {message}"),
                _ => continue,
            };
            if !quiet {
                println!("{line}");
            }
        }
    }

    println!("\nFinal standings after {} rounds (seed {seed}):", game.round_index() + 1);
    let mut standings: Vec<&crate::game::Player> = game.players().iter().collect();
    standings.sort_by_key(|p| p.placement);
    for p in standings {
        println!(
            "  {}. {:<8} {:<6} level {}  health {:>3}  augments: {}",
            p.placement.unwrap_or(0),
            p.name,
            levels[p.id.0 as usize].name(),
            p.level,
            p.health,
            if p.augments.is_empty() { "none".to_string() } else { p.augments.join(", ") },
        );
    }
    Ok(game)
}

fn label(combat: &Combat, id: UnitId) -> String {
    let u = combat.unit(id);
    format!("{}#{}[T{}]", u.name(), id.0, u.team().0)
}

fn print_log(combat: &Combat) {
    let label = |id: UnitId| label(combat, id);
    for e in combat.events() {
        let t = e.tick * combat.config().tick_ms;
        let line = match &e.kind {
            EventKind::Spawned { unit, pos, .. } => {
                format!("{} appears at {:?}", label(*unit), pos.to_offset())
            }
            EventKind::Moved { unit, to, .. } => {
                format!("{} moves to {:?}", label(*unit), to.to_offset())
            }
            EventKind::Displaced { unit, to, .. } => {
                format!("{} is moved to {:?}", label(*unit), to.to_offset())
            }
            EventKind::StacksChanged { unit, stack, count } => {
                format!("{} has {} {} stacks", label(*unit), count, stack)
            }
            EventKind::Attacked { .. } => continue, // the damage line follows
            EventKind::Dodged { source, target } => format!("{} dodges {}", label(*target), label(*source)),
            EventKind::Overtime => "overtime: damage grows every second from now on".to_string(),
            EventKind::Damaged { source, target, amount, absorbed, kind, crit } => format!(
                "{} hits {} for {} {:?}{}{}",
                label(*source),
                label(*target),
                amount,
                kind,
                if *crit { " (crit)" } else { "" },
                if *absorbed > 0 { format!(" ({absorbed} shielded)") } else { String::new() },
            ),
            EventKind::Healed { target, amount, .. } => format!("{} heals {}", label(*target), amount),
            EventKind::Shielded { target, amount, .. } => {
                format!("{} gains a {} shield", label(*target), amount)
            }
            EventKind::CastStarted { unit } => {
                let name = combat.unit(*unit).template().ability.as_ref().map(|a| a.name.as_str());
                format!("{} begins casting {}", label(*unit), name.unwrap_or("?"))
            }
            EventKind::CastFinished { unit } => format!("{} finishes casting", label(*unit)),
            EventKind::PassiveTriggered { unit, passive } => format!(
                "{} triggers {}",
                label(*unit),
                combat.unit(*unit).template().passives[*passive].name
            ),
            EventKind::StatusApplied { target, status, duration_ms, .. } => {
                let what = status.name();
                format!("{} is {} for {}ms", label(*target), what, duration_ms)
            }
            EventKind::StatusExpired { .. } => continue,
            EventKind::Died { unit, killer } => {
                format!("{} dies (killed by {})", label(*unit), label(*killer))
            }
            EventKind::Ended { winner, timed_out } => match (winner, timed_out) {
                (Some(team), _) => format!("Team {} wins", team.0),
                (None, true) => "Time ran out: draw".to_string(),
                (None, false) => "Everyone fell: draw".to_string(),
            },
        };
        println!("[{:>6.2}s] {}", t as f64 / 1000.0, line);
    }
}

/// Hosts one game: waits for players to join, fills empty seats with bots
/// when someone starts the game (or every seat is taken), and runs it to
/// the end. Clients connect over TCP to play or watch.
fn serve(spec: &GameSpec, args: &mut Args) -> Result<(), String> {
    let usage = args.usage();
    let bad = |what: &str| format!("invalid {what}\n{usage}");
    let port: u16 = match args.value("--port")? {
        Some(v) => v.parse().map_err(|_| bad("--port"))?,
        None => 7777,
    };
    // Local connections only unless asked otherwise.
    let bind = args.value("--bind")?.unwrap_or_else(|| "127.0.0.1".to_string());
    let record = args.value("--record")?;
    let resume = args.flag("--resume");
    if resume && record.is_none() {
        return Err(format!("--resume needs --record DIR, the folder the match was saved in\n{usage}"));
    }
    let dev = args.flag("--dev");
    let rooms = args.number("--rooms", 0, 0..=1000)? as usize;
    let spectator_delay = args.number("--spectator-delay-secs", 0, 0..=3600)? * 1000;
    let ladder = args.value("--ladder")?;
    if ladder.is_some() && rooms == 0 {
        return Err(format!("--ladder needs --rooms: ratings are kept across the games of a hub\n{usage}"));
    }

    let bots: bot::Difficulty = args.value("--bots")?.map(|b| b.parse()).transpose()?.unwrap_or_default();
    let mut config = SessionConfig {
        seed: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1),
        ..SessionConfig::default()
    };
    let mut net = ServerConfig::default();
    if let Some(v) = args.value("--max-connections")? {
        net.max_connections = v.parse().ok().filter(|n| *n >= 1).ok_or_else(|| bad("--max-connections"))?;
    }
    if let Some(v) = args.value("--max-per-ip")? {
        net.max_connections_per_ip = v.parse().ok().filter(|n| *n >= 1).ok_or_else(|| bad("--max-per-ip"))?;
    }
    if let Some(v) = args.value("--idle-secs")? {
        let secs = v.parse::<u64>().ok().filter(|n| *n >= 2).ok_or_else(|| bad("--idle-secs"))?;
        net.idle_timeout = Duration::from_secs(secs);
        net.ping_interval = Duration::from_secs((secs / 4).max(1)).min(net.ping_interval);
    }
    let stats_secs = args.number("--stats-secs", 0, 0..=86_400)?;
    config.seats = args.number("--seats", config.seats as u64, 2..=8)? as usize;
    if let Some(v) = args.value("--seed")? {
        config.seed = v.parse().map_err(|_| bad("--seed"))?;
    }
    config.planning_ms = args.number("--planning-secs", config.planning_ms / 1000, 0..=3600)? * 1000;
    args.done()?;

    let content = spec.load()?;
    let server = Server::bind((bind.as_str(), port))
        .map_err(|e| format!("cannot listen on {bind}:{port}: {e}"))?
        .with_config(net);
    println!(
        "{} server listening on {bind}:{port} ({} seats, seed {}, {}s planning)",
        spec.name,
        config.seats,
        config.seed,
        config.planning_ms / 1000
    );
    if rooms > 0 {
        let mut hub = crate::hub::Hub::new(content, crate::hub::HubConfig { room: config, max_rooms: rooms, ..Default::default() });
        hub.on_new_room(move |_, session| {
            session.set_bot_difficulty(bots);
            session.allow_dev(dev);
            session.set_spectator_delay(spectator_delay);
        });
        if dev {
            println!("developer commands are on");
        }
        if let Some(dir) = record {
            println!("saving each game's fights and match in {dir}/room-N");
            if resume {
                let count = hub.resume(Path::new(&dir), 0)?;
                println!("resumed {count} unfinished game(s) from {dir}");
            }
            hub.record_to(dir);
        }
        if let Some(file) = ladder {
            let path = PathBuf::from(&file);
            let loaded = crate::ladder::Ladder::load(&path)?;
            println!("rating players across games in {file} ({} rated so far)", loaded.standings().len());
            hub.keep_ladder(loaded, Some(path));
        }
        println!("hosting up to {rooms} games; players join the first with a free seat, or pick one");
        return report_stats(server.spawn(hub), stats_secs);
    }
    let mut session = Session::new(content, config);
    session.set_bot_difficulty(bots);
    session.set_spectator_delay(spectator_delay);
    if dev {
        println!("developer commands are on");
        session.allow_dev(true);
    }
    if let Some(dir) = record {
        if resume {
            let saved = Session::load_match(Path::new(&dir))?;
            let round = saved.log.inputs.iter().filter(|i| **i == crate::game::Input::FinishRound).count();
            session.resume(saved, 0).map_err(|e| format!("cannot resume the match in {dir}: {e}"))?;
            println!("resumed the match in {dir} after {round} rounds; players rejoin under their names");
        }
        println!("saving every fight as a replay, and the match as match.json, in {dir}");
        session.record_to(dir);
    }
    report_stats(server.spawn(session), stats_secs)
}

/// What the server terminal's commands need from what is hosted.
trait Admin: Send + 'static {
    fn announce(&mut self, text: &str);
    fn kick(&mut self, name: &str, now: u64) -> usize;
    fn status(&self) -> String;
    /// Pauses (or unpauses) the game, or on a hub the room named by `arg`.
    fn pause(&mut self, arg: &str, pause: bool, now: u64) -> String;
    fn ladder(&self) -> String {
        "this server keeps no ladder (serve --rooms N --ladder FILE)".into()
    }
}

fn pause_session(session: &mut Session, pause: bool, now: u64) -> String {
    let done = if pause { session.pause(now) } else { session.unpause(now) };
    match (done, pause) {
        (true, true) => "paused".into(),
        (true, false) => "running again".into(),
        (false, true) => "nothing to pause (not running, or already paused)".into(),
        (false, false) => "not paused".into(),
    }
}

impl Admin for Session {
    fn announce(&mut self, text: &str) {
        Session::announce(self, text);
    }
    fn kick(&mut self, name: &str, now: u64) -> usize {
        Session::kick(self, name, now) as usize
    }
    fn pause(&mut self, _arg: &str, pause: bool, now: u64) -> String {
        pause_session(self, pause, now)
    }
    fn status(&self) -> String {
        let seats: Vec<String> = self.seat_names().into_iter().map(|s| s.unwrap_or_else(|| "(open)".into())).collect();
        let phase = match self.game() {
            None => "waiting to start".to_string(),
            Some(g) if g.is_over() => "over".to_string(),
            Some(g) => format!("round {} ({})", g.round_name(), g.round_kind()),
        };
        format!("{phase}; {} connected; seats: {}", self.connections(), seats.join(", "))
    }
}

impl Admin for crate::hub::Hub {
    fn announce(&mut self, text: &str) {
        crate::hub::Hub::announce(self, text);
    }
    fn kick(&mut self, name: &str, now: u64) -> usize {
        crate::hub::Hub::kick(self, name, now)
    }
    fn ladder(&self) -> String {
        let Some(ladder) = crate::hub::Hub::ladder(self) else {
            return "this server keeps no ladder (serve --rooms N --ladder FILE)".into();
        };
        let rows = ladder.standings();
        if rows.is_empty() {
            return "nobody is rated yet".into();
        }
        rows.iter()
            .take(10)
            .enumerate()
            .map(|(i, r)| format!("{:>2}. {:<16} {:>5}  {} games, {} wins", i + 1, r.name, r.rating.rating, r.rating.games, r.rating.wins))
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn pause(&mut self, arg: &str, pause: bool, now: u64) -> String {
        match arg.trim_start_matches('#').parse().ok().and_then(|id| self.room_mut(id)) {
            Some(session) => pause_session(session, pause, now),
            None => "name a game by its number, such as `pause 2` (see `status`)".into(),
        }
    }
    fn status(&self) -> String {
        let rooms = self.rooms();
        let mut out = format!("{} games", rooms.len());
        for r in rooms {
            let state = if r.over { "over" } else if r.started { "playing" } else { "waiting" };
            out.push_str(&format!("\n  #{} {}: {state}, {}/{} players, {} connected", r.id, r.name, r.players, r.seats, r.connected));
        }
        out
    }
}

const ADMIN_HELP: &str = "commands: status, say TEXT, kick NAME, pause [GAME], resume [GAME], ladder, quit";

/// Reads commands typed into the server's terminal and runs them on the
/// hosted game.
fn admin_console<H: Admin>(server: &crate::net::ServerHandle<H>) {
    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        line.clear();
        match std::io::BufRead::read_line(&mut stdin.lock(), &mut line) {
            Ok(0) | Err(_) => return, // no terminal: keep serving quietly
            Ok(_) => {}
        }
        let line = line.trim().to_string();
        let (command, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
        let rest = rest.trim().to_string();
        let reply = match command {
            "" => continue,
            "help" => ADMIN_HELP.to_string(),
            "status" => server.with_host(|h, _| h.status()).unwrap_or_default(),
            "say" | "announce" if !rest.is_empty() => {
                server.with_host(move |h, _| h.announce(&rest));
                "sent".to_string()
            }
            "kick" if !rest.is_empty() => {
                let name = rest.clone();
                match server.with_host(move |h, now| h.kick(&name, now)) {
                    Some(0) | None => format!("nobody called {rest} has a seat"),
                    Some(n) => format!("removed {rest} from {n} game(s)"),
                }
            }
            "ladder" => server.with_host(|h, _| h.ladder()).unwrap_or_default(),
            "pause" | "resume" => {
                let pause = command == "pause";
                server.with_host(move |h, now| h.pause(&rest, pause, now)).unwrap_or_default()
            }
            "quit" | "exit" | "stop" => std::process::exit(0),
            _ => format!("did not understand \"{line}\"; {ADMIN_HELP}"),
        };
        println!("{reply}");
    }
}

/// Prints the server's counters every `stats_secs` seconds (if not zero)
/// while it runs, which is forever, and takes commands from the terminal.
fn report_stats<H: Admin>(server: crate::net::ServerHandle<H>, stats_secs: u64) -> Result<(), String> {
    println!("type `help` for server commands");
    let server = Arc::new(server);
    {
        let server = server.clone();
        std::thread::spawn(move || admin_console(&server));
    }
    // The game runs on the server's own thread; this one only reports.
    let mut last = server.stats();
    loop {
        std::thread::sleep(Duration::from_secs(stats_secs.max(1)));
        let stats = server.stats();
        if stats_secs > 0 && stats != last {
            println!(
                "{} connected ({} accepted, {} refused, {} dropped), in {} messages / {} bytes, out {} messages / {} bytes",
                stats.connections,
                stats.accepted,
                stats.refused,
                stats.dropped_slow + stats.dropped_idle + stats.dropped_flood,
                stats.messages_in,
                stats.bytes_in,
                stats.messages_out,
                stats.bytes_out
            );
            last = stats;
        }
    }
}
