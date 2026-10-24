use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use catasto_core::SyncMode;

pub fn sync(path: &Path, update: bool, only: &[PathBuf]) -> Result<ExitCode> {
    let mode = if update {
        SyncMode::Update
    } else {
        SyncMode::Locked
    };
    let lock = catasto_core::sync_only(path, mode, None, only)?;
    if only.is_empty() {
        println!("synced {} file(s)", lock.locked.len());
    } else {
        println!("synced {} of {} file(s)", only.len(), lock.locked.len());
    }
    Ok(ExitCode::SUCCESS)
}
