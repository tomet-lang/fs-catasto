use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;

pub fn sync(path: &Path) -> Result<ExitCode> {
    let lock = catasto_core::sync(path)?;
    println!("synced {} file(s)", lock.locked.len());
    Ok(ExitCode::SUCCESS)
}
