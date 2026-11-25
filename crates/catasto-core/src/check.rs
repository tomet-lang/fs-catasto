//! Drift detection between the manifest, the lockfile, and the files on disk.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use crate::lockfile::Lock;
use crate::manifest::{Managed, ManagedKind, Manifest, Source, Submodule, Symlink};
use catasto_digest::{sha256_dir, sha256_file, sha256_link};

/// How a managed entry or directory zone has drifted or failed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// A directory matching a `::zone(...)` target is missing a `require` pattern.
    ZoneMissingRequired { zone: String, pattern: String },
    /// An entry in a `::zone(...)` directory did not match any `allow` pattern.
    ZoneDisallowed { zone: String },
    /// An entry in a `::zone(...)` directory matched a `deny` pattern.
    ZoneDenied { zone: String, pattern: String },
}

impl ViolationKind {
    /// The one word for it, as `catasto list` and `catasto check --json` show it.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Missing => "missing",
            Self::Stale => "stale",
            Self::Unlocked => "unlocked",
            Self::Orphaned => "orphaned",
            Self::ZoneMissingRequired { .. } => "zone-require",
            Self::ZoneDisallowed { .. } => "zone-allow",
            Self::ZoneDenied { .. } => "zone-deny",
        }
    }

    /// Returns `true` if this violation originates from a `::zone(...)` rule.
    pub fn is_zone(&self) -> bool {
        matches!(
            self,
            Self::ZoneMissingRequired { .. }
                | Self::ZoneDisallowed { .. }
                | Self::ZoneDenied { .. }
        )
    }
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Modified => f.write_str(
                "modified: differs from the locked sha256 (`catasto sync` restores it; `catasto lock` keeps it, for a local source)",
            ),
            Self::Missing => {
                f.write_str("missing: managed file does not exist (run `catasto sync`)")
            }
            Self::Stale => {
                f.write_str("stale: source has changed since it was locked (run `catasto sync`)")
            }
            Self::Unlocked => f.write_str("unlocked: not in catasto.lock (run `catasto sync`)"),
            Self::Orphaned => f.write_str(
                "orphaned: in catasto.lock but not declared in catasto.ctmt (run `catasto sync`)",
            ),
            Self::ZoneMissingRequired { zone, pattern } => write!(
                f,
                "zone `{zone}`: missing required entry matching `{pattern}`"
            ),
            Self::ZoneDisallowed { zone } => {
                write!(f, "zone `{zone}`: entry is not allowed by `allow` patterns")
            }
            Self::ZoneDenied { zone, pattern } => {
                write!(f, "zone `{zone}`: entry matches denied pattern `{pattern}`")
            }
        }
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

    let mut violations = check_managed(root, &manifest, &lock)?;
    violations.extend(zone::check_zones(root, &manifest.zones)?);

    Ok(CheckReport {
        root: root.to_path_buf(),
        violations,
    })
}

pub(crate) fn check_managed(
    root: &Path,
    manifest: &Manifest,
    lock: &Lock,
) -> Result<Vec<Violation>> {
    let entry_violations: Result<Vec<Vec<Violation>>> = manifest
        .managed
        .par_iter()
        .map(|managed| {
            let kinds = check_entry(root, lock, managed)?;
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

    for locked_sub in &lock.submodules {
        if !manifest
            .managed
            .iter()
            .any(|m| m.is_submodule() && m.path == locked_sub.path)
        {
            violations.push(Violation {
                path: locked_sub.path.clone(),
                kind: ViolationKind::Orphaned,
            });
        }
    }

    if let Some(drift) = crate::submodule::check_gitmodules(root, manifest, lock)? {
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

    Ok(violations)
}

fn check_entry(root: &Path, lock: &Lock, managed: &Managed) -> Result<Vec<ViolationKind>> {
    let _token = jobserver_gate::acquire();
    match &managed.kind {
        ManagedKind::File {
            source,
            executable,
            symlink,
        } => check_file(root, lock, &managed.path, source, *executable, *symlink),
        ManagedKind::Submodule(sub) => check_submodule(root, lock, &managed.path, sub),
    }
}

fn check_file(
    root: &Path,
    lock: &Lock,
    path: &Path,
    source: &Source,
    executable: Option<bool>,
    symlink: Symlink,
) -> Result<Vec<ViolationKind>> {
    let file = root.join(path);
    let meta = match std::fs::symlink_metadata(&file) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("cannot inspect {}", file.display())),
    };

    let is_dir_on_disk = meta.as_ref().is_some_and(|m| m.is_dir());
    let Some(locked) = lock.get(path).filter(|locked| {
        &locked.source == source
            && locked.symlink == symlink
            && matches!(source, Source::Git { .. }) == locked.commit.is_some()
            // A declared executable mode that the lock does not hold is a declaration changed since the lock
            // (for directories, locked.mode is always None because executable bits are hashed in sha256_dir).
            && (is_dir_on_disk
                || executable.is_none_or(|declared| {
                    declared == (locked.mode == Some(crate::lockfile::LockMode::Executable))
                }))
    }) else {
        return Ok(vec![ViolationKind::Unlocked]);
    };

    let is_keep = symlink == Symlink::Keep;

    let mut kinds = Vec::new();

    match meta {
        None => {
            kinds.push(ViolationKind::Missing);
        }
        Some(meta) if meta.is_dir() => {
            if sha256_dir(&file)? != locked.sha256 {
                kinds.push(ViolationKind::Modified);
            }
        }
        Some(meta) if is_keep => {
            if !meta.is_symlink() || sha256_link(&file)? != locked.sha256 {
                kinds.push(ViolationKind::Modified);
            }
        }
        Some(meta) => {
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

    if let Some(source_path) = source.local_path(root) {
        if source_path.is_dir() {
            let source_hash = if executable.is_some() {
                crate::sync::read_local(root, path, source)?.sha256(executable)
            } else {
                sha256_dir(&source_path)?
            };
            if source_hash != locked.sha256 {
                kinds.push(ViolationKind::Stale);
            }
        } else if source_path.is_file() && sha256_file(&source_path)? != locked.sha256 {
            kinds.push(ViolationKind::Stale);
        }
    }

    Ok(kinds)
}

fn check_submodule(
    root: &Path,
    lock: &Lock,
    path: &Path,
    sub: &Submodule,
) -> Result<Vec<ViolationKind>> {
    let Some(_locked) = lock.get_submodule(path).filter(|locked| {
        locked.git == sub.git && locked.rev == sub.rev && locked.subpath == sub.path
    }) else {
        return Ok(vec![ViolationKind::Unlocked]);
    };

    let dir = root.join(path);
    if !dir.exists() || !dir.join(".git").exists() {
        return Ok(vec![ViolationKind::Missing]);
    }

    let resolved_url = catasto_git::resolve_url(root, &sub.git)?;
    match catasto_git::remote_url(&dir) {
        Ok(actual_url) if actual_url == sub.git || actual_url == resolved_url => {}
        Ok(_) | Err(_) => return Ok(vec![ViolationKind::Modified]),
    }

    if !catasto_git::check_sparse_checkout(&dir, sub.path.as_deref()).unwrap_or(false) {
        return Ok(vec![ViolationKind::Modified]);
    }

    Ok(Vec::new())
}

#[cfg(test)]
mod tests;
mod zone;
