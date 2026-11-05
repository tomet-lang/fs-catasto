use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use catasto_core::SyncMode;

pub fn sync(
    path: &Path,
    all: bool,
    update: bool,
    only: &[PathBuf],
    dry_run: bool,
) -> Result<ExitCode> {
    if all {
        sync_all(path, update, only, dry_run)
    } else {
        sync_single(path, update, only, dry_run)
    }
}

fn sync_single(
    path: &Path,
    update: bool,
    only: &[PathBuf],
    dry_run: bool,
) -> Result<ExitCode> {
    let mode = if update {
        SyncMode::Update
    } else {
        SyncMode::Locked
    };
    if dry_run {
        let report = catasto_core::sync_dry_run(path, mode, None, only)?;
        print!("{}", report.output);
        return Ok(ExitCode::SUCCESS);
    }
    let lock = catasto_core::sync_only(path, mode, None, only)?;
    if only.is_empty() {
        println!("synced {} file(s)", lock.locked.len());
    } else {
        println!("synced {} of {} file(s)", only.len(), lock.locked.len());
    }
    Ok(ExitCode::SUCCESS)
}

fn sync_all(
    root: &Path,
    update: bool,
    only: &[PathBuf],
    dry_run: bool,
) -> Result<ExitCode> {
    let workspaces = catasto_core::discover_workspaces(root)?;
    if workspaces.is_empty() {
        println!("catasto: no workspaces discovered under {}", root.display());
        return Ok(ExitCode::SUCCESS);
    }

    let total = workspaces.len();
    let mut succeeded = 0;
    let mut failed = 0;
    let mode = if update {
        SyncMode::Update
    } else {
        SyncMode::Locked
    };

    for (idx, ws) in workspaces.iter().enumerate() {
        let num = idx + 1;
        let ws_display = ws.display();
        if dry_run {
            match catasto_core::sync_dry_run(ws, mode, None, only) {
                Ok(report) => {
                    succeeded += 1;
                    if report.output.is_empty() {
                        println!("[{num}/{total}] {ws_display} ... clean");
                    } else {
                        println!("[{num}/{total}] {ws_display}");
                        print!("{}", report.output);
                    }
                }
                Err(e) => {
                    failed += 1;
                    println!("[{num}/{total}] {ws_display} ... error: {e:#}");
                }
            }
        } else {
            match catasto_core::sync_only(ws, mode, None, only) {
                Ok(lock) => {
                    succeeded += 1;
                    if only.is_empty() {
                        println!(
                            "[{num}/{total}] {ws_display} ... synced {} file(s)",
                            lock.locked.len()
                        );
                    } else {
                        println!(
                            "[{num}/{total}] {ws_display} ... synced {} of {} file(s)",
                            only.len(),
                            lock.locked.len()
                        );
                    }
                }
                Err(e) => {
                    failed += 1;
                    println!("[{num}/{total}] {ws_display} ... error: {e:#}");
                }
            }
        }
    }

    println!("Summary: {succeeded} succeeded, {failed} failed ({total} total)");
    if failed > 0 {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}
