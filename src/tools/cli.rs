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
use crate::effect::StatusKind;
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
    Command { name: "lobby", args: "[SEED] [--players N] [--quiet]", about: "bots play a whole game", run: lobby },
    Command { name: "sim", args: "[--games N] [--players N] [--seed N] [--json]", about: "bots play many games; prints a balance table", run: sim },
    Command { name: "bench", args: "[--fights N] [--games N]", about: "measure how fast fights and games run", run: bench },
    Command {
        name: "serve",
        args: "[--port 7777] [--bind ADDR] [--seats 8] [--seed N] [--planning-secs 30] [--record DIR] [--dev] \
               [--max-connections 64] [--idle-secs 30] [--stats-secs N]",
        about: "host a game for clients to join over TCP",
        run: serve,
    },
    Command { name: "replay", args: "FILE...", about: "fight saved replays again and report any that changed", run: replay },
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
    args.done()?;
    let content = Arc::new(spec.load()?);
    let report = tools::simulate(content.clone(), games, players, seed);
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
    let seed = seed_argument(args)?;
    args.done()?;
    play_lobby(spec.load()?, seed, players, quiet)
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
fn play_lobby(content: ContentSet, seed: u64, players: usize, quiet: bool) -> Result<(), String> {
    let mut game = Game::new(content, seed, &BOT_NAMES[..players]);
    let mut bot_rng = Rng::new(seed ^ 0xB07);
    let name = |game: &Game, id: PlayerId| game.player(id).name.clone();

    while !game.is_over() {
        if game.round_index() >= 500 {
            return Err("the game did not finish within 500 rounds".into());
        }
        bot::play_round(&mut game, &mut bot_rng).map_err(|e| e.to_string())?;
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
            "  {}. {:<8} level {}  health {:>3}  augments: {}",
            p.placement.unwrap_or(0),
            p.name,
            p.level,
            p.health,
            if p.augments.is_empty() { "none".to_string() } else { p.augments.join(", ") },
        );
    }
    Ok(())
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
            EventKind::Damaged { source, target, amount, absorbed, kind, crit } => format!(
                "{} hits {} for {} {:?}{}{}",
                label(*source),
                label(*target),
                amount,
                kind,
                if *crit { " (crit)" } else { "" },
                if *absorbed > 0 { format!(" ({absorbed} shielded)") } else { String::new() },
            ),
            EventKind::Healed { target, amount } => format!("{} heals {}", label(*target), amount),
            EventKind::Shielded { target, amount } => {
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
                let what = match status {
                    StatusKind::Stun => "stunned",
                    StatusKind::StatMods(_) => "stat-modified",
                    StatusKind::DamageOverTime { .. } => "afflicted",
                };
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
    let dev = args.flag("--dev");
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
    let mut session = Session::new(content, config);
    if dev {
        println!("developer commands are on");
        session.allow_dev(true);
    }
    if let Some(dir) = record {
        println!("saving every fight as a replay in {dir}");
        session.record_to(dir);
    }
    let server = server.spawn(session);
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
