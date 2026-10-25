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

use std::collections::{BTreeMap, HashMap};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::manifest::{Manifest, Source, Symlink, reject_misnamed};
use catasto_digest::{sha256_file, sha256_link};

/// File name of the lockfile, relative to the workspace root.
pub const LOCK_FILE: &str = "catasto.lock";

/// The format version this catasto reads and writes.
const LOCK_VERSION: u32 = 1;

/// What each managed file looked like when it was locked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lock {
    /// Locked files, sorted by path.
    pub locked: Vec<Locked>,
}

/// One locked file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locked {
    /// Location inside the workspace, relative to the root.
    pub path: PathBuf,
    /// The source declared when the file was locked.
    pub source: Source,
    /// Lowercase hex SHA-256 of the file content.
    pub sha256: String,
    /// Full commit id the file was read at. Present exactly for git sources.
    pub commit: Option<String>,
}

/// The JSON document.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockDocument {
    version: u32,
    files: BTreeMap<String, LockedBody>,
}

/// A locked file's fields, keyed by its path in [`LockDocument`].
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedBody {
    sha256: String,
    source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
}

impl Lock {
    /// A lock of `locked`, sorted by path.
    pub(crate) fn new(mut locked: Vec<Locked>) -> Self {
        locked.sort_by(|a, b| a.path.cmp(&b.path));
        Self { locked }
    }

    /// Loads `catasto.lock` from `root`. A missing file is an empty lock.
    pub fn load(root: &Path) -> Result<Self> {
        let file = root.join(LOCK_FILE);
        match std::fs::read_to_string(&file) {
            Ok(text) => Self::parse(&text).with_context(|| format!("invalid {}", file.display())),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                reject_misnamed(root, LOCK_FILE)?;
                Ok(Self::default())
            }
            Err(e) => Err(e).with_context(|| format!("cannot read {}", file.display())),
        }
    }

    fn parse(text: &str) -> Result<Self> {
        if !text.trim_start().starts_with('{') {
            bail!(
                "not a JSON lockfile; an older catasto wrote TOML. \
                 Delete it and run `catasto lock` (or `catasto sync` for git sources)"
            );
        }
        let document: LockDocument = serde_json::from_str(text)?;
        if document.version != LOCK_VERSION {
            bail!(
                "unsupported lock version {}; this catasto reads version {LOCK_VERSION}",
                document.version
            );
        }
        Ok(Self::new(
            document
                .files
                .into_iter()
                .map(|(path, body)| Locked {
                    path: path.into(),
                    source: body.source,
                    sha256: body.sha256,
                    commit: body.commit,
                })
                .collect(),
        ))
    }

    /// Renders the lockfile text.
    pub fn render(&self) -> Result<String> {
        let mut files = BTreeMap::new();
        for locked in &self.locked {
            let path = locked
                .path
                .to_str()
                .ok_or_else(|| anyhow!("path is not valid UTF-8: {}", locked.path.display()))?;
            files.insert(
                path.to_owned(),
                LockedBody {
                    sha256: locked.sha256.clone(),
                    source: locked.source.clone(),
                    commit: locked.commit.clone(),
                },
            );
        }
        let document = LockDocument {
            version: LOCK_VERSION,
            files,
        };
        let mut text = serde_json::to_string_pretty(&document)?;
        text.push('\n');
        Ok(text)
    }

    /// Looks up the entry for `path`.
    pub fn get(&self, path: &Path) -> Option<&Locked> {
        self.locked.iter().find(|l| l.path == path)
    }
}

/// Hashes every managed file and writes `catasto.lock`.
///
/// Every declared file must exist; the lock records what is on disk, so a file
/// that differs from its source shows up as `stale` on the next check.
///
/// A git source keeps the commit the existing lock recorded for it. `lock`
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
    write_lock(root, manifest, &commits)
}

/// Writes `catasto.lock` for `manifest`; `commits` maps git-sourced paths to
/// the commit they were read at.
pub(crate) fn write_lock(
    root: &Path,
    manifest: Manifest,
    commits: &HashMap<PathBuf, String>,
) -> Result<Lock> {
    write_lock_selected(root, manifest, commits, &Lock::default(), &|_| true)
}

/// Like [`write_lock`], for a run that touched only the files `selected` says
/// yes to. The others keep the entry `previous` has for them, as it was; one
/// that has no entry there (or whose source changed) is left out, so the next
/// `check` says it is `unlocked`.
pub(crate) fn write_lock_selected(
    root: &Path,
    manifest: Manifest,
    commits: &HashMap<PathBuf, String>,
    previous: &Lock,
    selected: &(dyn Fn(&Path) -> bool + Sync),
) -> Result<Lock> {
    write_lock_selected_with_hashes(root, manifest, commits, previous, selected, &HashMap::new())
}

/// Like [`write_lock_selected`], reusing precomputed hashes from `known_hashes`
/// instead of reading file contents from disk again.
pub(crate) fn write_lock_selected_with_hashes(
    root: &Path,
    manifest: Manifest,
    commits: &HashMap<PathBuf, String>,
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
            Ok(Some(Locked {
                sha256,
                path: managed.path,
                source: managed.source,
                commit,
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
