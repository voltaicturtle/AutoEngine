//! Runs a fight described in data files, headlessly, and prints the event log.
//!
//! Usage: `autoengine [seed] [--quiet] [--content DIR] [--match FILE]`

use std::process::ExitCode;

use autoengine::combat::{Combat, EventKind, UnitId};
use autoengine::content::{ContentSet, MatchSetup, parse_ron};
use autoengine::effect::StatusKind;
use autoengine::stats::Stat;

const USAGE: &str = "usage: autoengine [seed] [--quiet] [--content DIR] [--match FILE]";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let mut seed = 1u64;
    let mut quiet = false;
    let mut content_dir = "content".to_string();
    let mut match_file: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--quiet" => quiet = true,
            "--content" => content_dir = args.next().ok_or(USAGE)?,
            "--match" => match_file = Some(args.next().ok_or(USAGE)?),
            other => seed = other.parse().map_err(|_| USAGE.to_string())?,
        }
    }
    let match_file = match_file.unwrap_or_else(|| format!("{content_dir}/sample_match.ron"));

    let content = ContentSet::load_dir(&content_dir).map_err(|e| e.to_string())?;
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
