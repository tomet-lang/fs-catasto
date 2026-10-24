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
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("catasto: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    let root = &cli.workspace;
    match cli.command {
        Command::Init => commands::manage::init(&commands::root(root, None)?),
        Command::Add(args) => commands::manage::add(&commands::root(root, None)?, &args),
        Command::Remove { dest, delete } => {
            commands::manage::remove(&commands::root(root, None)?, &dest, delete)
        }
        Command::Upstream(command) => {
            commands::manage::upstream(&commands::root(root, None)?, &command)
        }
        Command::List { json } => commands::manage::list(&commands::root(root, None)?, json),
        Command::Check { path } => commands::check::check(&commands::root(root, path)?),
        Command::Lock { path } => commands::lock::lock(&commands::root(root, path)?),
        Command::Sync { path, update, only } => {
            commands::sync::sync(&commands::root(root, path)?, update, &only)
        }
    }
}
