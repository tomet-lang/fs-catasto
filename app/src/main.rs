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
            if e.downcast_ref::<catasto_core::AlreadyDeclared>().is_some() {
                ExitCode::from(3)
            } else {
                ExitCode::FAILURE
            }
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
        Command::List { path, all, json } => {
            commands::manage::list(&commands::root(root, path)?, all, json)
        }
        Command::Check { path, all, json } => {
            commands::check::check(&commands::root(root, path)?, all, json)
        }
        Command::Lock { path, all } => {
            commands::lock::lock(&commands::root(root, path)?, all)
        }
        Command::Sync {
            path,
            all,
            update,
            only,
            dry_run,
        } => commands::sync::sync(&commands::root(root, path)?, all, update, &only, dry_run),
        Command::Diff { path, all, only } => {
            commands::diff::diff(&commands::root(root, path)?, all, &only)
        }
        Command::Spec {
            target,
            all,
            out,
            check,
        } => {
            commands::spec::spec_cmd(target, all, out.as_deref(), check)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}
