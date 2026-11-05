use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;

pub fn diff(root: &Path, all: bool, only: &[PathBuf]) -> Result<ExitCode> {
    if all {
        diff_all(root, only)
    } else {
        diff_single(root, only)
    }
}

fn diff_single(root: &Path, only: &[PathBuf]) -> Result<ExitCode> {
    let output = catasto_core::diff(root, None, only)?;
    if output.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        print!("{output}");
        Ok(ExitCode::from(1))
    }
}

fn diff_all(root: &Path, only: &[PathBuf]) -> Result<ExitCode> {
    let workspaces = catasto_core::discover_workspaces(root)?;
    if workspaces.is_empty() {
        println!("catasto: no workspaces discovered under {}", root.display());
        return Ok(ExitCode::SUCCESS);
    }

    let total = workspaces.len();
    let mut clean = 0;
    let mut drifted = 0;
    let mut failed = 0;

    for (idx, ws) in workspaces.iter().enumerate() {
        let num = idx + 1;
        let ws_display = ws.display();
        match catasto_core::diff(ws, None, only) {
            Ok(output) => {
                if output.is_empty() {
                    clean += 1;
                    println!("[{num}/{total}] {ws_display} ... clean");
                } else {
                    drifted += 1;
                    println!("[{num}/{total}] {ws_display} ... diff detected");
                    print!("{output}");
                }
            }
            Err(e) => {
                failed += 1;
                println!("[{num}/{total}] {ws_display} ... error: {e:#}");
            }
        }
    }

    println!("Summary: {clean} clean, {drifted} drifted, {failed} failed ({total} total)");
    if drifted > 0 || failed > 0 {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}
