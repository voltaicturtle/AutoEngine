//! Headless command-line runner.
//!
//! * `autoengine fight [seed] [--quiet] [--content DIR] [--match FILE]` runs
//!   one fight described in data files and prints its event log.
//! * `autoengine lobby [seed] [--players N] [--quiet] [--content DIR]` plays
//!   a whole game between bots and prints what happens each round.
//!
//! With no subcommand, `fight` is assumed.

use std::process::ExitCode;

use autoengine::bot;
use autoengine::combat::{Combat, EventKind, Team, UnitId};
use autoengine::game::{Game, GameEvent, Opponent, PlayerId};
use autoengine::rng::Rng;
use autoengine::content::{ContentSet, MatchSetup, parse_ron};
use autoengine::effect::StatusKind;
use autoengine::stats::Stat;

const USAGE: &str = "usage: autoengine fight [seed] [--quiet] [--content DIR] [--match FILE]
       autoengine lobby [seed] [--players N] [--quiet] [--content DIR]
       autoengine replay FILE...
       autoengine check [DIR]
       autoengine sim [--games N] [--players N] [--seed N] [--content DIR]";

const BOT_NAMES: [&str; 8] = ["Aldra", "Brask", "Cyrene", "Dovik", "Elowen", "Fenn", "Garrick", "Hesper"];

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}

/// Loads a content folder and reports errors and likely mistakes.
fn check_content(dir: &str) -> Result<(), String> {
    let content = ContentSet::load_dir(dir).map_err(|e| format!("error: {e}"))?;
    println!(
        "{dir}: {} units, {} traits, {} items, {} augments, {} rounds, {} scripts",
        content.units().count(),
        content.traits().count(),
        content.items().count(),
        content.augments().count(),
        content.schedule().rounds.len(),
        content.scripts().len()
    );
    let warnings = autoengine::tools::lint(&content, Some(std::path::Path::new(dir)));
    for warning in &warnings {
        println!("warning: {warning}");
    }
    println!("{}", if warnings.is_empty() { "no problems found".to_string() } else { format!("{} warnings", warnings.len()) });
    Ok(())
}

/// Plays bot games and prints how units and traits did.
fn run_sim(args: Vec<String>) -> Result<(), String> {
    let (mut games, mut players, mut seed, mut dir) = (50u32, 8usize, 1u64, "content".to_string());
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut number = || args.next().and_then(|v| v.parse::<u64>().ok()).ok_or(USAGE.to_string());
        match arg.as_str() {
            "--games" => games = number()?.clamp(1, 100_000) as u32,
            "--players" => players = number()?.clamp(2, 8) as usize,
            "--seed" => seed = number()?,
            "--content" => dir = args.next().ok_or(USAGE)?,
            _ => return Err(USAGE.to_string()),
        }
    }
    let content = std::sync::Arc::new(ContentSet::load_dir(&dir).map_err(|e| e.to_string())?);
    let report = autoengine::tools::simulate(content.clone(), games, players, seed);
    print!("{}", report.to_text(&content));
    Ok(())
}

/// Re-runs saved fights and reports whether the engine still plays them
/// out exactly as recorded.
fn check_replays(files: Vec<String>) -> Result<(), String> {
    if files.is_empty() {
        return Err(USAGE.to_string());
    }
    let mut failed = 0;
    for file in &files {
        let replay = autoengine::replay::ReplayFile::load(std::path::Path::new(file))?;
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

fn run() -> Result<(), String> {
    let mut seed = 1u64;
    let mut quiet = false;
    let mut content_dir = "content".to_string();
    let mut match_file: Option<String> = None;
    let mut players = 8usize;
    let mut lobby = false;
    let mut args = std::env::args().skip(1).peekable();
    match args.peek().map(String::as_str) {
        Some("lobby") => {
            lobby = true;
            args.next();
        }
        Some("fight") => {
            args.next();
        }
        Some("replay") => {
            args.next();
            return check_replays(args.collect());
        }
        Some("check") => {
            args.next();
            return check_content(&args.next().unwrap_or_else(|| "content".to_string()));
        }
        Some("sim") => {
            args.next();
            return run_sim(args.collect());
        }
        _ => {}
    }
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--quiet" => quiet = true,
            "--players" => {
                players = args.next().and_then(|n| n.parse().ok()).filter(|n| (2..=8).contains(n)).ok_or(USAGE)?
            }
            "--content" => content_dir = args.next().ok_or(USAGE)?,
            "--match" => match_file = Some(args.next().ok_or(USAGE)?),
            other => seed = other.parse().map_err(|_| USAGE.to_string())?,
        }
    }
    let content = ContentSet::load_dir(&content_dir).map_err(|e| e.to_string())?;
    if lobby {
        return run_lobby(content, seed, players, quiet);
    }
    let match_file = match_file.unwrap_or_else(|| format!("{content_dir}/sample_match.ron"));
    let text = std::fs::read_to_string(&match_file).map_err(|e| format!("cannot read {match_file}: {e}"))?;
    let setup: MatchSetup = parse_ron(&match_file, &text).map_err(|e| e.to_string())?;

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
fn run_lobby(content: ContentSet, seed: u64, players: usize, quiet: bool) -> Result<(), String> {
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
