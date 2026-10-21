use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;

pub fn check(path: &Path) -> Result<ExitCode> {
    let report = catasto_core::check(path)?;
    if report.is_ok() {
        Ok(ExitCode::SUCCESS)
    } else {
        for violation in &report.violations {
            eprintln!("catasto: {violation}");
        }
        Ok(ExitCode::FAILURE)
    }
}
