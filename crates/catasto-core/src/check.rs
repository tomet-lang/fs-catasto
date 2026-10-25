//! Drift detection between the manifest, the lockfile, and the files on disk.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use crate::lockfile::Lock;
use crate::manifest::{Managed, Manifest, Source, Symlink};
use catasto_digest::{sha256_file, sha256_link};

/// How a managed file has drifted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationKind {
    /// The file differs from its locked SHA-256.
    Modified,
    /// The file does not exist.
    Missing,
    /// The source differs from the locked SHA-256.
    Stale,
    /// Declared, but the lock has no entry for it or records another source.
    Unlocked,
    /// Locked, but no longer declared.
    Orphaned,
}

impl ViolationKind {
    /// The one word for it, as `catasto list` shows it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Missing => "missing",
            Self::Stale => "stale",
            Self::Unlocked => "unlocked",
            Self::Orphaned => "orphaned",
        }
    }
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Modified => {
                "modified: differs from the locked sha256 (`catasto sync` restores it; `catasto lock` keeps it, for a local source)"
            }
            Self::Missing => "missing: managed file does not exist (run `catasto sync`)",
            Self::Stale => "stale: source has changed since it was locked (run `catasto sync`)",
            Self::Unlocked => "unlocked: not in catasto.lock (run `catasto sync`)",
            Self::Orphaned => {
                "orphaned: in catasto.lock but not declared in catasto.ctmt (run `catasto sync`)"
            }
        })
    }
}

/// One drifted file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Location inside the workspace, relative to the root.
    pub path: PathBuf,
    /// How it drifted.
    pub kind: ViolationKind,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.kind)
    }
}

/// Summary report produced by a `catasto check` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    /// Root directory that was checked.
    pub root: PathBuf,
    /// Detected drift, manifest order first, then orphans.
    pub violations: Vec<Violation>,
}

impl CheckReport {
    /// Returns `true` when no violations were found.
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Checks the managed files under `root` against `catasto.ctmt` and `catasto.lock`.
///
/// Files that are not declared and not locked are unmanaged and never reported.
/// A local source is consulted only when it is readable, so a vendored
/// workspace checks cleanly without its sources around. A git source is never
/// consulted: `check` does not touch the network.
pub fn check(root: &Path) -> Result<CheckReport> {
    if !root.exists() {
        bail!("path does not exist: {}", root.display());
    }

    let manifest = Manifest::load(root)?;
    let lock = Lock::load(root)?;

    let entry_violations: Result<Vec<Vec<Violation>>> = manifest
        .managed
        .par_iter()
        .map(|managed| {
            let kinds = check_entry(root, &lock, managed)?;
            let v = kinds
                .into_iter()
                .map(|kind| Violation {
                    path: managed.path.clone(),
                    kind,
                })
                .collect();
            Ok(v)
        })
        .collect();
    jobserver_gate::reclaim_implicit_token();

    let mut violations: Vec<Violation> = entry_violations?.into_iter().flatten().collect();

    for locked in &lock.locked {
        if !manifest.managed.iter().any(|m| m.path == locked.path) {
            violations.push(Violation {
                path: locked.path.clone(),
                kind: ViolationKind::Orphaned,
            });
        }
    }

    Ok(CheckReport {
        root: root.to_path_buf(),
        violations,
    })
}

fn check_entry(root: &Path, lock: &Lock, managed: &Managed) -> Result<Vec<ViolationKind>> {
    let _token = jobserver_gate::acquire();
    let Some(locked) = lock.get(&managed.path).filter(|locked| {
        locked.source == managed.source
            && matches!(managed.source, Source::Git { .. }) == locked.commit.is_some()
    }) else {
        return Ok(vec![ViolationKind::Unlocked]);
    };

    let file = root.join(&managed.path);
    let meta = match std::fs::symlink_metadata(&file) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("cannot inspect {}", file.display())),
    };

    let is_keep = matches!(
        &managed.source,
        Source::Git {
            symlink: Symlink::Keep,
            ..
        }
    );

    let mut kinds = Vec::new();

    match (is_keep, meta) {
        (_, None) => {
            kinds.push(ViolationKind::Missing);
        }
        (true, Some(meta)) => {
            if !meta.is_symlink() || sha256_link(&file)? != locked.sha256 {
                kinds.push(ViolationKind::Modified);
            }
        }
        (false, Some(meta)) => {
            if meta.is_symlink() || !meta.is_file() || sha256_file(&file)? != locked.sha256 {
                kinds.push(ViolationKind::Modified);
            }
        }
    }

    let readable_source = managed.source.local_path(root).filter(|s| s.is_file());
    if let Some(source) = readable_source {
        if sha256_file(&source)? != locked.sha256 {
            kinds.push(ViolationKind::Stale);
        }
    }

    Ok(kinds)
}

#[cfg(test)]
mod tests;
