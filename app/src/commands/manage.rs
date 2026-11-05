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
    if args.local && args.keep_symlink {
        bail!("--local has no --keep-symlink option; a local source has no such option");
    }
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
            keep_symlink: args.keep_symlink,
        }
    } else if let Some(git) = &args.git {
        SourceText::Git {
            git: git.clone(),
            rev: args.rev.clone(),
            path: from,
            keep_symlink: args.keep_symlink,
        }
    } else {
        SourceText::Local { path: from }
    };
    let outcome = catasto_core::add(root, &args.dest, &source, args.force, None)?;
    let lock = Lock::load(root)?;
    let suffix = match lock
        .get(Path::new(dest))
        .and_then(|locked| locked.commit.as_deref())
    {
        Some(commit) => format!(", locked at {}", short(commit)),
        None => String::new(),
    };
    match outcome {
        catasto_core::AddOutcome::Added => println!("added {dest}{suffix}"),
        catasto_core::AddOutcome::Replaced => println!("replaced {dest}{suffix}"),
        catasto_core::AddOutcome::Unchanged => println!("{dest}: declared as given"),
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
            force,
        } => {
            let (outcome, used) = catasto_core::upstream_add(
                root,
                name,
                &UpstreamText {
                    git: git.clone(),
                    rev: rev.clone(),
                    root: dir.clone(),
                },
                *force,
            )?;
            match outcome {
                catasto_core::AddOutcome::Added => println!("added upstream {name}"),
                catasto_core::AddOutcome::Replaced => {
                    println!("replaced upstream {name}");
                    if !used.is_empty() {
                        let count = used.len();
                        println!(
                            "{count} file(s) use it; run `catasto sync` to take them from the new place"
                        );
                    }
                }
                catasto_core::AddOutcome::Unchanged => {
                    println!("upstream {name}: declared as given");
                }
            }
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

pub fn list(root: &Path, all: bool, json: bool) -> Result<ExitCode> {
    if all {
        list_all(root, json)
    } else {
        list_single(root, json)
    }
}

fn list_single(root: &Path, json: bool) -> Result<ExitCode> {
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

    print_rows(&rows);
    Ok(ExitCode::SUCCESS)
}

fn list_all(root: &Path, json: bool) -> Result<ExitCode> {
    let workspaces = catasto_core::discover_workspaces(root)?;
    if workspaces.is_empty() {
        if json {
            let obj = serde_json::json!({
                "workspaces": [],
                "summary": {
                    "total": 0,
                }
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        } else {
            println!("catasto: no workspaces discovered under {}", root.display());
        }
        return Ok(ExitCode::SUCCESS);
    }

    let total = workspaces.len();
    let mut failed = 0;
    let mut ws_json = Vec::new();

    for (idx, ws) in workspaces.iter().enumerate() {
        let num = idx + 1;
        let ws_display = ws.display();
        match catasto_core::list(ws) {
            Ok(rows) => {
                if json {
                    let files: Vec<_> = rows
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
                    ws_json.push(serde_json::json!({
                        "root": ws,
                        "files": files,
                    }));
                } else {
                    println!("[{num}/{total}] {ws_display}");
                    if rows.is_empty() {
                        println!("  (no managed files declared)");
                    } else {
                        print_rows_indented(&rows);
                    }
                    if num < total {
                        println!();
                    }
                }
            }
            Err(e) => {
                failed += 1;
                if json {
                    ws_json.push(serde_json::json!({
                        "root": ws,
                        "error": format!("{e:#}"),
                    }));
                } else {
                    println!("[{num}/{total}] {ws_display} ... error: {e:#}");
                    if num < total {
                        println!();
                    }
                }
            }
        }
    }

    if json {
        let obj = serde_json::json!({
            "workspaces": ws_json,
            "summary": {
                "total": total,
            }
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    }

    if failed > 0 {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn print_rows(rows: &[catasto_core::Row]) {
    let width = |f: fn(&catasto_core::Row) -> usize| rows.iter().map(f).max().unwrap_or(0);
    let path_width = width(|row| row.path.display().to_string().chars().count());
    let from_width = width(|row| row.from.chars().count());
    for row in rows {
        println!(
            "{:path_width$}  {:from_width$}  {:8}  {}",
            row.path.display(),
            row.from,
            row.state.map_or("ok", |state| state.label()),
            row.commit.as_deref().map(short).unwrap_or_default(),
        );
    }
}

fn print_rows_indented(rows: &[catasto_core::Row]) {
    let width = |f: fn(&catasto_core::Row) -> usize| rows.iter().map(f).max().unwrap_or(0);
    let path_width = width(|row| row.path.display().to_string().chars().count());
    let from_width = width(|row| row.from.chars().count());
    for row in rows {
        println!(
            "  {:path_width$}  {:from_width$}  {:8}  {}",
            row.path.display(),
            row.from,
            row.state.map_or("ok", |state| state.label()),
            row.commit.as_deref().map(short).unwrap_or_default(),
        );
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}
