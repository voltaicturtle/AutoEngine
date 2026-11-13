//! The headless game server.
//!
//! Hosts one game: waits for players to join, fills empty seats with bots
//! when someone starts the game (or every seat is taken), and runs it to the
//! end. It has no graphics; clients connect over TCP to play or watch.
//!
//! Usage: `autoengine-server [--port 7777] [--bind ADDR] [--seats 8] [--seed N]
//!         [--planning-secs 30] [--content DIR] [--record DIR] [--dev]
//!         [--max-connections 64] [--idle-secs 30] [--stats-secs N]`
//!
//! `--stats-secs N` prints connection and traffic counters every N seconds.

use std::process::ExitCode;
use std::time::Duration;

use autoengine::content::ContentSet;
use autoengine::net::{Server, ServerConfig};
use autoengine::session::{Session, SessionConfig};

const USAGE: &str = "usage: autoengine-server [--port 7777] [--bind ADDR] [--seats 8] [--seed N] \
[--planning-secs 30] [--content DIR] [--record DIR] [--dev] [--max-connections 64] [--idle-secs 30] [--stats-secs N]";

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
    let mut port = 7777u16;
    // Local connections only unless asked otherwise.
    let mut bind = "127.0.0.1".to_string();
    let mut content_dir = "content".to_string();
    let mut config = SessionConfig {
        seed: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1),
        ..SessionConfig::default()
    };
    let mut args = std::env::args().skip(1);
    let mut record: Option<String> = None;
    let mut dev = false;
    let mut net = ServerConfig::default();
    let mut stats_secs = 0u64;
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(USAGE.to_string());
        match arg.as_str() {
            "--port" => port = value()?.parse().map_err(|_| USAGE)?,
            "--bind" => bind = value()?,
            "--record" => record = Some(value()?),
            "--dev" => dev = true,
            "--max-connections" => net.max_connections = value()?.parse().ok().filter(|n| *n >= 1).ok_or(USAGE)?,
            "--idle-secs" => {
                let secs = value()?.parse::<u64>().ok().filter(|n| *n >= 2).ok_or(USAGE)?;
                net.idle_timeout = Duration::from_secs(secs);
                net.ping_interval = Duration::from_secs((secs / 4).max(1)).min(net.ping_interval);
            }
            "--stats-secs" => stats_secs = value()?.parse().map_err(|_| USAGE)?,
            "--seats" => {
                config.seats = value()?.parse().ok().filter(|n| (2..=8).contains(n)).ok_or(USAGE)?;
            }
            "--seed" => config.seed = value()?.parse().map_err(|_| USAGE)?,
            "--planning-secs" => config.planning_ms = value()?.parse::<u64>().map_err(|_| USAGE)? * 1000,
            "--content" => content_dir = value()?,
            _ => return Err(USAGE.to_string()),
        }
    }

    let content = ContentSet::load_dir(&content_dir).map_err(|e| e.to_string())?;
    let server = Server::bind((bind.as_str(), port))
        .map_err(|e| format!("cannot listen on {bind}:{port}: {e}"))?
        .with_config(net);
    println!(
        "autoengine-server listening on {bind}:{port} ({} seats, seed {}, {}s planning)",
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
