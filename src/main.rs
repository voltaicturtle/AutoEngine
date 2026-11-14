//! The `autoengine` command-line tool for the sample game. Run
//! `autoengine help` for the list of commands.
//!
//! A game built on the engine has a `main` just like this one that passes
//! its own [`GameSpec`](autoengine::tools::cli::GameSpec); `autoengine new`
//! writes one.

fn main() -> std::process::ExitCode {
    autoengine::tools::cli::run(&autoengine::sample::spec())
}
