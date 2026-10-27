//! The generated `catasto.lock`.
//!
//! JSON, written by `catasto lock` and `catasto sync` and read by nothing that
//! needs a human in the loop. Files are keyed by path under `files`, so a
//! file named `version` cannot collide with the format's own fields:
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
pub use catasto_manifest::lock::{LOCK_FILE, Lock, LockMode, Locked};
use catasto_manifest::manifest::{Manifest, Source, Symlink};

/// Hashes every managed file and writes `catasto.lock`.
///
/// Every declared file must exist; the lock records what is on disk, so a file
/// that differs from its source shows up as `stale` on the next check.
///
/// A git source keeps the commit and mode the existing lock recorded for it. `lock`
/// never resolves a revision; `catasto sync` does.
pub fn lock(root: &Path) -> Result<Lock> {
    let manifest = Manifest::load(root)?;
    let existing = Lock::load(root)?;

    let commits = manifest
        .managed
        .iter()
        .filter_map(|managed| {
            let locked = existing.get(&managed.path)?;
            let commit = locked.commit.clone()?;
            (locked.source == managed.source).then(|| (managed.path.clone(), commit))
        })
        .collect();
    let modes = manifest
        .managed
        .iter()
        .filter_map(|managed| {
            let locked = existing.get(&managed.path)?;
            let mode = locked.mode?;
            (locked.source == managed.source).then(|| (managed.path.clone(), mode))
        })
        .collect();
    write_lock(root, manifest, &commits, &modes)
}

/// Writes `catasto.lock` for `manifest`; `commits` maps git-sourced paths to
/// the commit they were read at, and `modes` maps paths with special modes.
pub(crate) fn write_lock(
    root: &Path,
    manifest: Manifest,
    commits: &HashMap<PathBuf, String>,
    modes: &HashMap<PathBuf, LockMode>,
) -> Result<Lock> {
    write_lock_selected(root, manifest, commits, modes, &Lock::default(), &|_| true)
}

/// Like [`write_lock`], for a run that touched only the files `selected` says
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
    let locked_results: Result<Vec<Option<Locked>>> = manifest
        .managed
        .into_par_iter()
        .map(|managed| {
            if !selected(&managed.path) {
                let kept = previous
                    .get(&managed.path)
                    .filter(|kept| kept.source == managed.source)
                    .cloned();
                return Ok(kept);
            }
            let _token = jobserver_gate::acquire();
            let file = root.join(&managed.path);
            let meta = match std::fs::symlink_metadata(&file) {
                Ok(m) => m,
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    bail!("managed file does not exist: {}", managed.path.display());
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("cannot inspect {}", file.display()));
                }
            };

            let is_keep = matches!(
                &managed.source,
                Source::Git {
                    symlink: Symlink::Keep,
                    ..
                }
            );

            if is_keep {
                if !meta.is_symlink() {
                    bail!(
                        "`symlink: keep` declared for {}, but destination is not a symlink",
                        managed.path.display()
                    );
                }
            } else {
                if meta.is_symlink() {
                    bail!(
                        "managed file {} is a symlink; expected a regular file",
                        managed.path.display()
                    );
                }
                if !meta.is_file() {
                    bail!("managed file does not exist: {}", managed.path.display());
                }
            }

            let sha256 = match known_hashes.get(&managed.path) {
                Some(hash) => hash.clone(),
                None if is_keep => sha256_link(&file)?,
                None => sha256_file(&file)?,
            };

            let commit = match managed.source {
                Source::Path(_) => None,
                Source::Git { .. } => {
                    Some(commits.get(&managed.path).cloned().with_context(|| {
                        format!(
                            "no locked commit for {}: run `catasto sync`",
                            managed.path.display()
                        )
                    })?)
                }
            };
            let mode = modes.get(&managed.path).copied();
            Ok(Some(Locked {
                sha256,
                path: managed.path,
                source: managed.source,
                commit,
                mode,
            }))
        })
        .collect();
    jobserver_gate::reclaim_implicit_token();

    let locked: Vec<Locked> = locked_results?.into_iter().flatten().collect();

    let lock = Lock::new(locked);
    let file = root.join(LOCK_FILE);
    std::fs::write(&file, lock.render()?)
        .with_context(|| format!("cannot write {}", file.display()))?;
    Ok(lock)
}

#[cfg(test)]
mod tests;
