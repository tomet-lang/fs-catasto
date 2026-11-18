//! The generated `catasto.lock`.
//!
//! JSON, written by `catasto lock` and `catasto sync` and read by nothing that
//! needs a human in the loop. Files are keyed by path under `files` and repositories
//! under `repos`, so a file named `version` cannot collide with the format's own fields:
//!
//! ```text
//! {
//!   "version": 1,
//!   "files": {
//!     "a.tmt": { "sha256": "...", "source": "../vocabularies/a.tmt" },
//!     "w.tmt": {
//!       "sha256": "...",
//!       "source": { "git": "https://example.com/v.git", "rev": "v1", "path": "w.tmt" },
//!       "commit": "..."
//!     }
//!   }
//! }
//! ```

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use catasto_digest::{sha256_file, sha256_link};
pub use catasto_manifest::lock::{LOCK_FILE, Lock, LockMode, Locked, LockedRepo};
use catasto_manifest::manifest::{ManagedKind, Manifest, Repo, Source, Symlink};

/// Hashes every managed file and records repository commits in `catasto.lock`.
///
/// Every declared file must exist; the lock records what is on disk, so a file
/// that differs from its source shows up as `stale` on the next check.
///
/// A git source keeps the commit the existing lock recorded for it. So does its mode, unless the entry declares
/// one (`executable`), which is recorded as it is. `lock` never resolves a revision; `catasto sync` does.
pub fn lock(root: &Path) -> Result<Lock> {
    let manifest = Manifest::load(root)?;
    let existing = Lock::load(root)?;

    let mut commits: HashMap<PathBuf, String> = HashMap::new();
    let mut modes: HashMap<PathBuf, LockMode> = HashMap::new();

    for managed in &manifest.managed {
        match &managed.kind {
            ManagedKind::File { source, executable } => {
                if let Some(locked) = existing.get(&managed.path)
                    && &locked.source == source
                    && let Some(commit) = &locked.commit
                {
                    commits.insert(managed.path.clone(), commit.clone());
                }
                let mode = match executable {
                    Some(true) => Some(LockMode::Executable),
                    Some(false) => None,
                    None => existing
                        .get(&managed.path)
                        .filter(|locked| &locked.source == source)
                        .and_then(|locked| locked.mode),
                };
                if let Some(mode) = mode {
                    modes.insert(managed.path.clone(), mode);
                }
            }
            ManagedKind::Repo(repo) => {
                let commit = existing
                    .get_repo(&managed.path)
                    .filter(|locked| {
                        locked.git == repo.git
                            && locked.branch == repo.branch
                            && locked.submodule == repo.submodule
                    })
                    .and_then(|r| r.commit.clone())
                    .or_else(|| {
                        let dir = root.join(&managed.path);
                        dir.join(".git")
                            .exists()
                            .then(|| catasto_git::head_commit(&dir).ok())
                            .flatten()
                    });
                if let Some(commit) = commit {
                    commits.insert(managed.path.clone(), commit);
                }
            }
        }
    }

    write_lock_selected(root, manifest, &commits, &modes, &existing, &|_| true)
}

/// Writes `catasto.lock` for `manifest`; `commits` maps git-sourced paths to
/// the commit they were read at, and `modes` maps paths with special modes.
#[allow(dead_code)]
pub(crate) fn write_lock(
    root: &Path,
    manifest: Manifest,
    commits: &HashMap<PathBuf, String>,
    modes: &HashMap<PathBuf, LockMode>,
) -> Result<Lock> {
    write_lock_selected(root, manifest, commits, modes, &Lock::default(), &|_| true)
}

/// Like [`write_lock`], for a run that touched only the entries `selected` says
/// yes to. The others keep the entry `previous` has for them, as it was; one
/// that has no entry there (or whose source changed) is left out, so the next
/// `check` says it is `unlocked`.
pub(crate) fn write_lock_selected(
    root: &Path,
    manifest: Manifest,
    commits: &HashMap<PathBuf, String>,
    modes: &HashMap<PathBuf, LockMode>,
    previous: &Lock,
    selected: &(dyn Fn(&Path) -> bool + Sync),
) -> Result<Lock> {
    write_lock_selected_with_hashes(
        root,
        manifest,
        commits,
        modes,
        previous,
        selected,
        &HashMap::new(),
    )
}

/// Like [`write_lock_selected`], reusing precomputed hashes from `known_hashes`
/// instead of reading file contents from disk again.
pub(crate) fn write_lock_selected_with_hashes(
    root: &Path,
    manifest: Manifest,
    commits: &HashMap<PathBuf, String>,
    modes: &HashMap<PathBuf, LockMode>,
    previous: &Lock,
    selected: &(dyn Fn(&Path) -> bool + Sync),
    known_hashes: &HashMap<PathBuf, String>,
) -> Result<Lock> {
    let mut file_entries: Vec<(PathBuf, Source)> = Vec::new();
    let mut repo_entries: Vec<(PathBuf, Repo)> = Vec::new();
    for managed in manifest.managed {
        match managed.kind {
            ManagedKind::File { source, .. } => file_entries.push((managed.path, source)),
            ManagedKind::Repo(repo) => repo_entries.push((managed.path, repo)),
        }
    }

    let locked_repos: Vec<LockedRepo> = repo_entries
        .into_iter()
        .filter_map(|(path, repo)| {
            if !selected(&path) {
                return previous
                    .get_repo(&path)
                    .filter(|kept| {
                        kept.git == repo.git
                            && kept.branch == repo.branch
                            && kept.submodule == repo.submodule
                    })
                    .cloned();
            }
            let commit = commits.get(&path).cloned().or_else(|| {
                previous
                    .get_repo(&path)
                    .filter(|kept| {
                        kept.git == repo.git
                            && kept.branch == repo.branch
                            && kept.submodule == repo.submodule
                    })
                    .and_then(|r| r.commit.clone())
            });
            Some(LockedRepo {
                path,
                git: repo.git,
                branch: repo.branch,
                submodule: repo.submodule,
                commit,
            })
        })
        .collect();

    let locked_results: Result<Vec<Option<Locked>>> = file_entries
        .into_par_iter()
        .map(|(path, source)| {
            if !selected(&path) {
                let kept = previous
                    .get(&path)
                    .filter(|kept| kept.source == source)
                    .cloned();
                return Ok(kept);
            }
            let _token = jobserver_gate::acquire();
            let file = root.join(&path);
            let meta = match std::fs::symlink_metadata(&file) {
                Ok(m) => m,
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    bail!("managed file does not exist: {}", path.display());
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("cannot inspect {}", file.display()));
                }
            };

            let is_keep = matches!(
                source,
                Source::Git {
                    symlink: Symlink::Keep,
                    ..
                }
            );

            if is_keep {
                if !meta.is_symlink() {
                    bail!(
                        "`symlink: keep` declared for {}, but destination is not a symlink",
                        path.display()
                    );
                }
            } else {
                if meta.is_symlink() {
                    bail!(
                        "managed file {} is a symlink; expected a regular file",
                        path.display()
                    );
                }
                if !meta.is_file() {
                    bail!("managed file does not exist: {}", path.display());
                }
            }

            let sha256 = match known_hashes.get(&path) {
                Some(hash) => hash.clone(),
                None if is_keep => sha256_link(&file)?,
                None => sha256_file(&file)?,
            };

            let commit = match source {
                Source::Path(_) => None,
                Source::Git { .. } => Some(commits.get(&path).cloned().with_context(|| {
                    format!(
                        "no locked commit for {}: run `catasto sync`",
                        path.display()
                    )
                })?),
            };
            let mode = modes.get(&path).copied();
            Ok(Some(Locked {
                sha256,
                path,
                source,
                commit,
                mode,
            }))
        })
        .collect();
    jobserver_gate::reclaim_implicit_token();

    let locked: Vec<Locked> = locked_results?.into_iter().flatten().collect();

    let lock = Lock::new(locked, locked_repos);
    let file = root.join(LOCK_FILE);
    std::fs::write(&file, lock.render()?)
        .with_context(|| format!("cannot write {}", file.display()))?;
    Ok(lock)
}

#[cfg(test)]
mod tests;
