#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod cli;
mod commands;

use std::process::ExitCode;

use clap::Parser;
use cli::{Cli, Command};

fn main() -> ExitCode {
    jobserver_gate::init_from_env();
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("catasto: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> anyhow::Result<ExitCode> {
    match command {
        Command::Check { path } => commands::check::check(&path),
        Command::Lock { path } => commands::lock::lock(&path),
        Command::Sync { path } => commands::sync::sync(&path),
    }
}
