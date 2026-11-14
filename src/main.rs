//! The `autoengine` command-line tool: everything a game's developers do
//! without opening the client. Run `autoengine help` for the list of
//! commands; each one is a small function in this file built on the
//! library's public API, so it doubles as an example of using it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use autoengine::bot;
use autoengine::combat::{Combat, EventKind, Team, UnitId};
use autoengine::content::{ContentSet, MatchSetup, parse_ron};
use autoengine::effect::StatusKind;
use autoengine::game::{Game, GameEvent, Opponent, PlayerId};
use autoengine::rng::Rng;
use autoengine::stats::Stat;
use autoengine::tools;

const BOT_NAMES: [&str; 8] = ["Aldra", "Brask", "Cyrene", "Dovik", "Elowen", "Fenn", "Garrick", "Hesper"];

/// One subcommand: its name, its arguments, what it does and the function
/// that runs it.
struct Command {
    name: &'static str,
    args: &'static str,
    about: &'static str,
    run: fn(&mut Args) -> Result<(), String>,
}

const COMMANDS: &[Command] = &[
    Command { name: "new", args: "DIR", about: "write a small starter game to build on", run: new },
    Command { name: "check", args: "[DIR] [--watch]", about: "look for mistakes in a content folder", run: check },
    Command { name: "test", args: "[DIR] [--filter TEXT]", about: "run the fights in tests.ron and check their results", run: test },
    Command { name: "fight", args: "[SEED] [--quiet] [--content DIR] [--match FILE | --test NAME]", about: "run one fight and print what happens", run: fight },
    Command { name: "lobby", args: "[SEED] [--players N] [--quiet] [--content DIR]", about: "bots play a whole game", run: lobby },
    Command { name: "sim", args: "[--games N] [--players N] [--seed N] [--content DIR] [--json]", about: "bots play many games; prints a balance table", run: sim },
    Command { name: "bench", args: "[--fights N] [--games N] [--content DIR]", about: "measure how fast fights and games run", run: bench },
    Command { name: "replay", args: "FILE...", about: "fight saved replays again and report any that changed", run: replay },
    Command { name: "api", args: "", about: "print what scripts can do", run: api },
];

fn help() -> String {
    let mut out = String::from("usage: autoengine COMMAND [ARGS]\n\n");
    for c in COMMANDS {
        out.push_str(&format!("  {:<7} {}\n          autoengine {} {}\n", c.name, c.about, c.name, c.args));
    }
    out.push_str("\nDIR and --content default to ./content.");
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
        format!("usage: autoengine {} {}", self.command.name, self.command.args)
    }

    /// Whether a flag such as `--quiet` was given.
    fn flag(&mut self, name: &str) -> bool {
        let found = self.rest.iter().position(|a| a == name);
        found.map(|i| self.rest.remove(i)).is_some()
    }

    /// The value after a flag such as `--content DIR`.
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

    fn content_dir(&mut self) -> Result<String, String> {
        Ok(self.value("--content")?.unwrap_or_else(|| "content".to_string()))
    }

    /// Fails if anything was not understood.
    fn done(&self) -> Result<(), String> {
        match self.rest.first() {
            None => Ok(()),
            Some(extra) => Err(format!("did not understand \"{extra}\"\n{}", self.usage())),
        }
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "help".to_string());
    if matches!(name.as_str(), "help" | "--help" | "-h") {
        println!("{}", help());
        return ExitCode::SUCCESS;
    }
    let Some(command) = COMMANDS.iter().find(|c| c.name == name) else {
        eprintln!("there is no command called \"{name}\"\n\n{}", help());
        return ExitCode::from(2);
    };
    let mut args = Args { command, rest: args.collect() };
    match (command.run)(&mut args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}

fn load(dir: &str) -> Result<ContentSet, String> {
    ContentSet::load_dir(dir).map_err(|e| format!("error: {e}"))
}

fn new(args: &mut Args) -> Result<(), String> {
    let dir = args.positional().ok_or_else(|| args.usage())?;
    args.done()?;
    let written = tools::scaffold::write_starter(Path::new(&dir))?;
    println!("created {dir} with {} files:", written.len());
    for file in &written {
        println!("  {file}");
    }
    println!("\nNext: read {dir}/README.md, then try\n  autoengine check {dir}\n  autoengine test {dir}");
    Ok(())
}

/// Loads a folder and prints errors and likely mistakes. Returns whether it
/// was clean.
fn check_once(dir: &str) -> bool {
    let content = match load(dir) {
        Ok(content) => content,
        Err(message) => {
            println!("{message}");
            return false;
        }
    };
    println!(
        "{dir}: {} units, {} traits, {} items, {} augments, {} rounds, {} scripts",
        content.units().count(),
        content.traits().count(),
        content.items().count(),
        content.augments().count(),
        content.schedule().rounds.len(),
        content.scripts().len()
    );
    let warnings = tools::lint(&content, Some(Path::new(dir)));
    for warning in &warnings {
        println!("warning: {warning}");
    }
    println!("{}", if warnings.is_empty() { "no problems found".to_string() } else { format!("{} warnings", warnings.len()) });
    warnings.is_empty()
}

/// The newest change time of anything in a folder, for `--watch`.
fn newest_change(dir: &Path) -> Option<std::time::SystemTime> {
    let mut newest = None;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).ok()?.flatten() {
            let path: PathBuf = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(time) = entry.metadata().and_then(|m| m.modified()) {
                newest = newest.max(Some(time));
            }
        }
    }
    newest
}

fn check(args: &mut Args) -> Result<(), String> {
    let watch = args.flag("--watch");
    let dir = args.positional().unwrap_or_else(|| "content".to_string());
    args.done()?;
    if !watch {
        // Mistakes that stop the content loading are errors; warnings are not.
        load(&dir)?;
        check_once(&dir);
        return Ok(());
    }
    // Check again, and run the tests, every time a file is saved.
    let mut seen = None;
    loop {
        let newest = newest_change(Path::new(&dir));
        if newest != seen {
            seen = newest;
            println!("\n--- checking {dir} ---");
            if check_once(&dir) || load(&dir).is_ok() {
                let _ = run_tests(&dir, None);
            }
            println!("watching {dir} for changes (Ctrl+C to stop)");
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}

/// Runs the fights in a folder's tests.ron. `Ok(true)` means all passed.
fn run_tests(dir: &str, filter: Option<&str>) -> Result<bool, String> {
    let content = load(dir)?;
    let file = format!("{dir}/tests.ron");
    let Ok(text) = std::fs::read_to_string(&file) else {
        println!("{file} does not exist, so there are no tests to run");
        return Ok(true);
    };
    let mut scenarios = tools::scenario::ScenarioFile::parse(&file, &text).map_err(|e| format!("error: {e}"))?;
    if let Some(filter) = filter {
        scenarios.tests.retain(|t| t.name.contains(filter));
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

fn test(args: &mut Args) -> Result<(), String> {
    let filter = args.value("--filter")?;
    let dir = args.positional().unwrap_or_else(|| "content".to_string());
    args.done()?;
    if run_tests(&dir, filter.as_deref())? { Ok(()) } else { Err("some tests failed".to_string()) }
}

fn sim(args: &mut Args) -> Result<(), String> {
    let games = args.number("--games", 50, 1..=100_000)? as u32;
    let players = args.number("--players", 8, 2..=8)? as usize;
    let seed = args.number("--seed", 1, 0..=u64::MAX)?;
    let json = args.flag("--json");
    let dir = args.content_dir()?;
    args.done()?;
    let content = Arc::new(load(&dir)?);
    let report = tools::simulate(content.clone(), games, players, seed);
    if json {
        println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
    } else {
        print!("{}", report.to_text(&content));
    }
    Ok(())
}

fn bench(args: &mut Args) -> Result<(), String> {
    let fights = args.number("--fights", 200, 1..=1_000_000)? as u32;
    let games = args.number("--games", 5, 0..=10_000)? as u32;
    let dir = args.content_dir()?;
    args.done()?;
    let content = Arc::new(load(&dir)?);
    if cfg!(debug_assertions) {
        println!("note: this is a debug build; use `cargo run --release -- bench` for real numbers");
    }
    let file = format!("{dir}/sample_match.ron");
    match std::fs::read_to_string(&file) {
        Ok(text) => {
            let setup: MatchSetup = parse_ron(&file, &text).map_err(|e| e.to_string())?;
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
        Err(_) => println!("fights: skipped, {file} does not exist"),
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

fn api(args: &mut Args) -> Result<(), String> {
    args.done()?;
    #[cfg(feature = "scripting")]
    print!("{}", autoengine::script::api_reference());
    #[cfg(not(feature = "scripting"))]
    println!("this build of the engine has scripting turned off");
    Ok(())
}

/// Re-runs saved fights and reports whether the engine still plays them
/// out exactly as recorded.
fn replay(args: &mut Args) -> Result<(), String> {
    let files = std::mem::take(&mut args.rest);
    if files.is_empty() {
        return Err(args.usage());
    }
    let mut failed = 0;
    for file in &files {
        let replay = autoengine::replay::ReplayFile::load(Path::new(file))?;
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

fn lobby(args: &mut Args) -> Result<(), String> {
    let quiet = args.flag("--quiet");
    let players = args.number("--players", 8, 2..=8)? as usize;
    let dir = args.content_dir()?;
    let seed = seed_argument(args)?;
    args.done()?;
    play_lobby(load(&dir)?, seed, players, quiet)
}

/// An optional seed given as a bare number.
fn seed_argument(args: &mut Args) -> Result<u64, String> {
    match args.positional() {
        None => Ok(1),
        Some(text) => text.parse().map_err(|_| format!("the seed must be a number, not \"{text}\"")),
    }
}

fn fight(args: &mut Args) -> Result<(), String> {
    let quiet = args.flag("--quiet");
    let content_dir = args.content_dir()?;
    let match_file = args.value("--match")?;
    let test_name = args.value("--test")?;
    let seed = seed_argument(args)?;
    args.done()?;
    let content = load(&content_dir)?;
    let setup = match test_name {
        // Watch the fight of one of the tests in tests.ron.
        Some(name) => {
            let file = format!("{content_dir}/tests.ron");
            let text = std::fs::read_to_string(&file).map_err(|e| format!("cannot read {file}: {e}"))?;
            let tests = tools::scenario::ScenarioFile::parse(&file, &text).map_err(|e| e.to_string())?.tests;
            let found = tests.into_iter().find(|t| t.name.contains(&name));
            let scenario = found.ok_or(format!("no test in {file} has \"{name}\" in its name"))?;
            println!("test: {}", scenario.name);
            MatchSetup { teams: scenario.teams }
        }
        None => {
            let file = match_file.unwrap_or_else(|| format!("{content_dir}/sample_match.ron"));
            let text = std::fs::read_to_string(&file).map_err(|e| format!("cannot read {file}: {e}"))?;
            parse_ron(&file, &text).map_err(|e| e.to_string())?
        }
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
    let mut standings: Vec<&autoengine::game::Player> = game.players().iter().collect();
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
