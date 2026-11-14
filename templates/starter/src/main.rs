//! The game's command line: `cargo run -- help` lists the commands.

fn main() -> std::process::ExitCode {
    autoengine::tools::cli::run(&mygame::spec())
}
