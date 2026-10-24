use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;

pub fn diff(root: &Path, only: &[PathBuf]) -> Result<ExitCode> {
    let output = catasto_core::diff(root, None, only)?;
    if output.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        print!("{output}");
        Ok(ExitCode::from(1))
    }
}
