//! Drift detection between the manifest, the lockfile, and the files on disk.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use crate::lockfile::Lock;
use crate::manifest::{Managed, ManagedKind, Manifest, Repo, Source, Symlink};
use catasto_digest::{sha256_file, sha256_link};

/// How a managed entry has drifted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationKind {
    /// The file or repository differs from what was locked or declared.
    Modified,
    /// The file, directory, or `.gitmodules` does not exist.
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

/// One drifted entry.
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
    /// Detected drift, manifest order first, then orphans, then `.gitmodules`.
    pub violations: Vec<Violation>,
}

impl CheckReport {
    /// Returns `true` when no violations were found.
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Checks the managed files and repositories under `root` against `catasto.ctmt` and `catasto.lock`.
///
/// Entries that are not declared and not locked are unmanaged and never reported.
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
        if !manifest
            .managed
            .iter()
            .any(|m| m.is_file() && m.path == locked.path)
        {
            violations.push(Violation {
                path: locked.path.clone(),
                kind: ViolationKind::Orphaned,
            });
        }
    }

    for locked_repo in &lock.repos {
        if !manifest
            .managed
            .iter()
            .any(|m| m.is_repo() && m.path == locked_repo.path)
        {
            violations.push(Violation {
                path: locked_repo.path.clone(),
                kind: ViolationKind::Orphaned,
            });
        }
    }

    if let Some(drift) = crate::submodule::check_gitmodules(root, &manifest, &lock)? {
        let kind = match drift {
            crate::submodule::GitmodulesDrift::Missing => ViolationKind::Missing,
            crate::submodule::GitmodulesDrift::Modified => ViolationKind::Modified,
            crate::submodule::GitmodulesDrift::Orphaned => ViolationKind::Orphaned,
        };
        violations.push(Violation {
            path: PathBuf::from(crate::submodule::GITMODULES_FILE),
            kind,
        });
    }

    Ok(CheckReport {
        root: root.to_path_buf(),
        violations,
    })
}

fn check_entry(root: &Path, lock: &Lock, managed: &Managed) -> Result<Vec<ViolationKind>> {
    let _token = jobserver_gate::acquire();
    match &managed.kind {
        ManagedKind::File { source, executable } => {
            check_file(root, lock, &managed.path, source, *executable)
        }
        ManagedKind::Repo(repo) => check_repo(root, lock, &managed.path, repo),
    }
}

fn check_file(
    root: &Path,
    lock: &Lock,
    path: &Path,
    source: &Source,
    executable: Option<bool>,
) -> Result<Vec<ViolationKind>> {
    let Some(locked) = lock.get(path).filter(|locked| {
        &locked.source == source
            && matches!(source, Source::Git { .. }) == locked.commit.is_some()
            // A declared executable mode that the lock does not hold is a declaration changed since the lock.
            && executable.is_none_or(|declared| {
                declared == (locked.mode == Some(crate::lockfile::LockMode::Executable))
            })
    }) else {
        return Ok(vec![ViolationKind::Unlocked]);
    };

    let file = root.join(path);
    let meta = match std::fs::symlink_metadata(&file) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("cannot inspect {}", file.display())),
    };

    let is_keep = matches!(
        source,
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
            #[cfg(unix)]
            let mode_matches = {
                use std::os::unix::fs::PermissionsExt;
                let is_exec = meta.permissions().mode() & 0o111 != 0;
                let locked_exec = locked.mode == Some(crate::lockfile::LockMode::Executable);
                is_exec == locked_exec
            };
            #[cfg(not(unix))]
            let mode_matches = true;

            if meta.is_symlink()
                || !meta.is_file()
                || !mode_matches
                || sha256_file(&file)? != locked.sha256
            {
                kinds.push(ViolationKind::Modified);
            }
        }
    }

    let readable_source = source.local_path(root).filter(|s| s.is_file());
    if let Some(source_file) = readable_source {
        if sha256_file(&source_file)? != locked.sha256 {
            kinds.push(ViolationKind::Stale);
        }
    }

    Ok(kinds)
}

fn check_repo(root: &Path, lock: &Lock, path: &Path, repo: &Repo) -> Result<Vec<ViolationKind>> {
    let Some(_locked) = lock.get_repo(path).filter(|locked| {
        locked.git == repo.git
            && locked.branch == repo.branch
            && locked.submodule == repo.submodule
    }) else {
        return Ok(vec![ViolationKind::Unlocked]);
    };

    let dir = root.join(path);
    if !dir.exists() || !dir.join(".git").exists() {
        return Ok(vec![ViolationKind::Missing]);
    }

    let resolved_url = catasto_git::resolve_url(root, &repo.git)?;
    match catasto_git::remote_url(&dir) {
        Ok(actual_url) if actual_url == repo.git || actual_url == resolved_url => {}
        Ok(_) | Err(_) => return Ok(vec![ViolationKind::Modified]),
    }

    Ok(Vec::new())
}

#[cfg(test)]
mod tests;
