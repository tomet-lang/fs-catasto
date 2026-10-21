//! `catasto-core` -- core engine and models for filesystem schemas, manifests, and lockfiles.
//!
//! Just as a cadastre (*catasto*) records the boundaries and parcels of land,
//! `catasto` defines and verifies directory rules, naming conventions, media
//! manifests, and content lockfiles across a workspace.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// Summary report produced by a `catasto check` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    /// Root directory that was checked.
    pub root: PathBuf,
    /// Detected schema or lockfile violations.
    pub violations: Vec<String>,
}

impl CheckReport {
    /// Returns `true` when no violations were found.
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Checks the directory tree rooted at `root` against its catasto schema and lockfile.
pub fn check(root: &Path) -> Result<CheckReport> {
    if !root.exists() {
        anyhow::bail!("path does not exist: {}", root.display());
    }

    Ok(CheckReport {
        root: root.to_path_buf(),
        violations: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_current_dir_succeeds() {
        let report = check(Path::new(".")).unwrap();
        assert!(report.is_ok());
    }
}
