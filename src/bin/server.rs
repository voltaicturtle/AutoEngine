//! The headless game server for the sample game: the same as
//! `autoengine serve`, kept as its own binary for hosting.
//!
//! Usage: `autoengine-server [--port 7777] [--bind ADDR] [--seats 8] [--seed N]
//!         [--planning-secs 30] [--record DIR] [--dev]
//!         [--max-connections 64] [--idle-secs 30] [--stats-secs N]`
//!
//! A game built on the engine serves its own content with its own binary's
//! `serve` command.

fn main() -> std::process::ExitCode {
    let args = std::iter::once("serve".to_string()).chain(std::env::args().skip(1)).collect();
    autoengine::tools::cli::run_args(&autoengine::sample::spec(), args)
}
