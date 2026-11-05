use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;

pub fn lock(path: &Path, all: bool) -> Result<ExitCode> {
    if all {
        lock_all(path)
    } else {
        lock_single(path)
    }
}

fn lock_single(path: &Path) -> Result<ExitCode> {
    let lock = catasto_core::lock(path)?;
    println!("locked {} file(s)", lock.locked.len());
    Ok(ExitCode::SUCCESS)
}

fn lock_all(root: &Path) -> Result<ExitCode> {
    let workspaces = catasto_core::discover_workspaces(root)?;
    if workspaces.is_empty() {
        println!("catasto: no workspaces discovered under {}", root.display());
        return Ok(ExitCode::SUCCESS);
    }

    let total = workspaces.len();
    let mut succeeded = 0;
    let mut failed = 0;

    for (idx, ws) in workspaces.iter().enumerate() {
        let num = idx + 1;
        let ws_display = ws.display();
        match catasto_core::lock(ws) {
            Ok(lock) => {
                succeeded += 1;
                println!(
                    "[{num}/{total}] {ws_display} ... locked {} file(s)",
                    lock.locked.len()
                );
            }
            Err(e) => {
                failed += 1;
                println!("[{num}/{total}] {ws_display} ... error: {e:#}");
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
