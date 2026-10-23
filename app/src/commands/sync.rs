use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use catasto_core::SyncMode;

pub fn sync(path: &Path, update: bool) -> Result<ExitCode> {
    let mode = if update {
        SyncMode::Update
    } else {
        SyncMode::Locked
    };
    let lock = catasto_core::sync(path, mode)?;
    println!("synced {} file(s)", lock.locked.len());
    Ok(ExitCode::SUCCESS)
}
