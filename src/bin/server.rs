//! The headless game server.
//!
//! Hosts one game: waits for players to join, fills empty seats with bots
//! when someone starts the game (or every seat is taken), and runs it to the
//! end. It has no graphics; clients connect over TCP to play or watch.
//!
//! Usage: `autoengine-server [--port 7777] [--bind ADDR] [--seats 8] [--seed N]
//!         [--planning-secs 30] [--content DIR]`

use std::net::TcpListener;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use autoengine::content::ContentSet;
use autoengine::net::serve;
use autoengine::session::{Session, SessionConfig};

const USAGE: &str = "usage: autoengine-server [--port 7777] [--bind ADDR] [--seats 8] [--seed N] \
[--planning-secs 30] [--content DIR]";

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
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(USAGE.to_string());
        match arg.as_str() {
            "--port" => port = value()?.parse().map_err(|_| USAGE)?,
            "--bind" => bind = value()?,
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
    let listener = TcpListener::bind((bind.as_str(), port)).map_err(|e| format!("cannot listen on {bind}:{port}: {e}"))?;
    println!(
        "autoengine-server listening on {bind}:{port} ({} seats, seed {}, {}s planning)",
        config.seats,
        config.seed,
        config.planning_ms / 1000
    );
    let stop = AtomicBool::new(false);
    serve(listener, Session::new(content, config), &stop).map_err(|e| e.to_string())?;
    Ok(())
}
