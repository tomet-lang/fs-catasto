//! `init`, `add`, `remove`, `upstream` and `list`.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Result, bail};
use catasto_core::{Lock, SourceText, UpstreamText};

use crate::cli::{AddArgs, UpstreamCommand};

pub fn init(root: &Path) -> Result<ExitCode> {
    let file = catasto_core::init(root)?;
    println!("created {}", file.display());
    println!("name a place with `catasto upstream add`, then take a file with `catasto add`");
    Ok(ExitCode::SUCCESS)
}

pub fn add(root: &Path, args: &AddArgs) -> Result<ExitCode> {
    let dest = args.dest.strip_prefix("./").unwrap_or(&args.dest);
    let from = match &args.from {
        Some(from) => from.clone(),
        None => {
            if args.local {
                bail!("--local needs the path of the file to take it from");
            }
            dest.to_owned()
        }
    };
    let source = if let Some(name) = &args.upstream {
        SourceText::Upstream {
            name: name.clone(),
            path: from,
        }
    } else if let Some(git) = &args.git {
        SourceText::Git {
            git: git.clone(),
            rev: args.rev.clone(),
            path: from,
        }
    } else {
        SourceText::Local { path: from }
    };
    catasto_core::add(root, &args.dest, &source, None)?;
    let lock = Lock::load(root)?;
    match lock
        .get(Path::new(dest))
        .and_then(|locked| locked.commit.as_deref())
    {
        Some(commit) => println!("added {dest}, locked at {}", short(commit)),
        None => println!("added {dest}"),
    }
    Ok(ExitCode::SUCCESS)
}

pub fn remove(root: &Path, dest: &str, delete: bool) -> Result<ExitCode> {
    catasto_core::remove(root, dest, delete)?;
    if delete {
        println!("removed {dest} and deleted the file");
    } else {
        println!("removed {dest}; the file is still on disk");
    }
    Ok(ExitCode::SUCCESS)
}

pub fn upstream(root: &Path, command: &UpstreamCommand) -> Result<ExitCode> {
    match command {
        UpstreamCommand::Add {
            name,
            git,
            rev,
            root: dir,
        } => {
            catasto_core::upstream_add(
                root,
                name,
                &UpstreamText {
                    git: git.clone(),
                    rev: rev.clone(),
                    root: dir.clone(),
                },
            )?;
            println!("added upstream {name}");
        }
        UpstreamCommand::Remove { name } => {
            catasto_core::upstream_remove(root, name)?;
            println!("removed upstream {name}");
        }
        UpstreamCommand::List => {
            for upstream in catasto_core::upstream_list(root)? {
                let dir = upstream
                    .root
                    .map(|root| format!("  root {}", root.display()))
                    .unwrap_or_default();
                println!(
                    "{}  {} @ {}{dir}",
                    upstream.name, upstream.git, upstream.rev
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn list(root: &Path, json: bool) -> Result<ExitCode> {
    let rows = catasto_core::list(root)?;
    if json {
        let rows: Vec<_> = rows
            .iter()
            .map(|row| {
                let mut map = serde_json::json!({
                    "path": row.path,
                    "from": row.from,
                    "state": row.state.map_or("ok", |state| state.label()),
                    "commit": row.commit,
                });
                if let Some(symlink) = row.symlink {
                    map["symlink"] = serde_json::to_value(symlink).unwrap();
                }
                map
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(ExitCode::SUCCESS);
    }

    let width = |f: fn(&catasto_core::Row) -> usize| rows.iter().map(f).max().unwrap_or(0);
    let path_width = width(|row| row.path.display().to_string().chars().count());
    let from_width = width(|row| row.from.chars().count());
    for row in &rows {
        println!(
            "{:path_width$}  {:from_width$}  {:8}  {}",
            row.path.display(),
            row.from,
            row.state.map_or("ok", |state| state.label()),
            row.commit.as_deref().map(short).unwrap_or_default(),
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}
