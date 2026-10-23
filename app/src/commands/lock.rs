use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;

pub fn lock(path: &Path) -> Result<ExitCode> {
    let lock = catasto_core::lock(path)?;
    println!("locked {} file(s)", lock.locked.len());
    Ok(ExitCode::SUCCESS)
}
