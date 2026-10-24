//! Re-fetching managed files from their sources.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::lockfile::{Lock, write_lock_selected_with_hashes};
use crate::manifest::{Managed, Manifest, Source};
use catasto_git::{Content, Fetched, GitCache, resolve_url};

/// How `sync` treats git sources that the lock already pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Read the commit recorded in `catasto.lock`. A source whose declaration
    /// changed, or that has no recorded commit, is resolved from its `rev`.
    Locked,
    /// Resolve every `rev` again and lock the commits it points at now.
    Update,
}

/// Copies every managed file from its source, then rewrites `catasto.lock`.
///
/// Every source is read before anything is written, so a missing source leaves
/// the workspace untouched. Git sources are cached under the default cache
/// directory; see [`GitCache::default_dir`].
pub fn sync(root: &Path, mode: SyncMode) -> Result<Lock> {
    sync_with(root, mode, None)
}

/// Like [`sync`], with an explicit git cache directory.
pub fn sync_with(root: &Path, mode: SyncMode, cache_dir: Option<&Path>) -> Result<Lock> {
    sync_only(root, mode, cache_dir, &[])
}

/// What `sync` would write for one managed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Workspace-relative path of the managed file (as in manifest).
    pub path: PathBuf,
    /// Absolute path where it would be written on disk.
    pub destination: PathBuf,
    /// What `sync` would write: file bytes or link target.
    pub content: Content,
    /// The commit it came from (for git sources).
    pub commit: Option<String>,
    /// The canonical source from the manifest.
    pub source: Source,
}

/// A plan of changes `sync` would write.
#[derive(Debug, Clone)]
pub struct SyncPlan {
    pub manifest: Manifest,
    pub lock: Lock,
    pub files: Vec<PlannedFile>,
}

/// Computes the content `sync` would write for the selected files, fetching
/// what is needed from git repositories and reading local sources.
pub fn plan_sync(
    root: &Path,
    mode: SyncMode,
    cache_dir: Option<&Path>,
    only: &[PathBuf],
) -> Result<SyncPlan> {
    let manifest = Manifest::load(root)?;
    let lock = Lock::load(root)?;
    for path in only {
        if !manifest.managed.iter().any(|managed| &managed.path == path) {
            bail!(
                "{} is not declared in {}",
                path.display(),
                crate::MANIFEST_FILE
            );
        }
    }
    let selected = |path: &Path| only.is_empty() || only.iter().any(|p| p == path);
    let mut fetched_git = fetch_git(root, mode, cache_dir, &manifest, &lock, &selected)?;

    let mut files = Vec::with_capacity(manifest.managed.len());
    for (index, managed) in manifest.managed.iter().enumerate() {
        if !selected(&managed.path) {
            continue;
        }
        let (content, commit) = match &managed.source {
            Source::Git { .. } => {
                let file = fetched_git[index]
                    .take()
                    .expect("every git source was fetched")?;
                (file.content, Some(file.commit))
            }
            local => (Content::File(read_local(root, &managed.path, local)?), None),
        };
        files.push(PlannedFile {
            path: managed.path.clone(),
            destination: root.join(&managed.path),
            content,
            commit,
            source: managed.source.clone(),
        });
    }

    Ok(SyncPlan {
        manifest,
        lock,
        files,
    })
}

/// Like [`sync_with`], for the files in `only` (paths as the manifest writes
/// them); an empty `only` is every file. The other files, and what the lock
/// says about them, stay as they are: `catasto sync --update --only a` moves
/// `a` and nothing else.
pub fn sync_only(
    root: &Path,
    mode: SyncMode,
    cache_dir: Option<&Path>,
    only: &[PathBuf],
) -> Result<Lock> {
    let plan = plan_sync(root, mode, cache_dir, only)?;
    let mut commits = HashMap::new();
    let mut hashes = HashMap::new();
    let mut fetched = Vec::with_capacity(plan.files.len());

    for file in &plan.files {
        if let Some(commit) = &file.commit {
            commits.insert(file.path.clone(), commit.clone());
        }
        let hash = match &file.content {
            Content::File(bytes) => catasto_digest::sha256_hex(bytes),
            Content::Link(link_target) => catasto_digest::sha256_hex(link_target.as_bytes()),
        };
        hashes.insert(file.path.clone(), hash);
        fetched.push((file.destination.clone(), file.content.clone()));
    }

    write_all(fetched)?;
    let selected = |path: &Path| only.is_empty() || only.iter().any(|p| p == path);
    write_lock_selected_with_hashes(root, plan.manifest, &commits, &plan.lock, &selected, &hashes)
}

/// Reads every git source, one result per entry of the manifest (`None` for
/// the entries that are not git sources).
///
/// A git URL has one bare clone in the cache, which two readers must not
/// update at once, so the entries are grouped by URL: different URLs are read
/// at the same time, each on a thread that holds a jobserver token, and the
/// entries of one URL one after another. The results go back by the position
/// of the entry, so what `sync` writes, and which error it reports first,
/// does not depend on which thread finished first.
fn fetch_git(
    root: &Path,
    mode: SyncMode,
    cache_dir: Option<&Path>,
    manifest: &Manifest,
    lock: &Lock,
    selected: &dyn Fn(&Path) -> bool,
) -> Result<Vec<Option<Result<Fetched>>>> {
    let mut by_url: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, managed) in manifest.managed.iter().enumerate() {
        let Source::Git { git, .. } = &managed.source else {
            continue;
        };
        if !selected(&managed.path) {
            continue;
        }
        let url = resolve_url(root, git)?;
        match by_url.iter_mut().find(|(known, _)| *known == url) {
            Some((_, indices)) => indices.push(index),
            None => by_url.push((url, vec![index])),
        }
    }

    let mut results: Vec<Option<Result<Fetched>>> = manifest.managed.iter().map(|_| None).collect();
    if by_url.is_empty() {
        return Ok(results);
    }
    let cache = match cache_dir {
        Some(dir) => GitCache::new(dir),
        None => GitCache::new(GitCache::default_dir()?),
    };

    let groups: Vec<Vec<(usize, Result<Fetched>)>> = std::thread::scope(|scope| {
        let cache = &cache;
        let handles: Vec<_> = by_url
            .iter()
            .map(|(url, indices)| {
                scope.spawn(move || {
                    let _token = jobserver_gate::acquire();
                    indices
                        .iter()
                        .map(|&index| {
                            (
                                index,
                                fetch_entry(cache, url, mode, lock, &manifest.managed[index]),
                            )
                        })
                        .collect()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a git fetch thread panicked"))
            .collect()
    });
    jobserver_gate::reclaim_implicit_token();

    for (index, result) in groups.into_iter().flatten() {
        results[index] = Some(result);
    }
    Ok(results)
}

/// Reads one git source from `url`, at the commit the lock pins when `mode` lets it.
fn fetch_entry(
    cache: &GitCache,
    url: &str,
    mode: SyncMode,
    lock: &Lock,
    managed: &Managed,
) -> Result<Fetched> {
    let Source::Git {
        rev, path, symlink, ..
    } = &managed.source
    else {
        unreachable!("only git sources are grouped by URL");
    };
    let pinned = match mode {
        SyncMode::Update => None,
        SyncMode::Locked => lock
            .get(&managed.path)
            .filter(|locked| locked.source == managed.source)
            .and_then(|locked| locked.commit.as_deref()),
    };
    cache
        .fetch(url, rev, path, pinned, (*symlink).into())
        .with_context(|| format!("source of {}", managed.path.display()))
}

fn read_local(root: &Path, managed: &Path, source: &Source) -> Result<Vec<u8>> {
    let Some(file) = source.local_path(root) else {
        unreachable!("only local sources are read from the filesystem");
    };
    if !file.is_file() {
        bail!(
            "source of {} does not exist: {}",
            managed.display(),
            file.display()
        );
    }
    std::fs::read(&file).with_context(|| format!("cannot read {}", file.display()))
}

fn write_all(files: Vec<(PathBuf, Content)>) -> Result<()> {
    for (target, content) in files {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        match content {
            Content::File(bytes) => {
                write_file_replacing_symlink(&target, &bytes)?;
            }
            Content::Link(link_target) => {
                write_symlink_atomic(&target, &link_target)?;
            }
        }
    }
    Ok(())
}

/// Writes `bytes` to `target` as a regular file.
///
/// A symlink at `target` is removed first, since writing would otherwise go
/// through it into whatever it points at. A regular file is rewritten in
/// place, which keeps its mode.
fn write_file_replacing_symlink(target: &Path, bytes: &[u8]) -> Result<()> {
    if std::fs::symlink_metadata(target).is_ok_and(|meta| meta.is_symlink()) {
        std::fs::remove_file(target)
            .with_context(|| format!("cannot replace the symlink {}", target.display()))?;
    }
    std::fs::write(target, bytes).with_context(|| format!("cannot write {}", target.display()))
}

#[cfg(unix)]
fn write_symlink_atomic(target: &Path, link_target: &str) -> Result<()> {
    use std::time::SystemTime;
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let temp_path = parent.join(format!(".catasto.symlink.{nonce}.tmp"));
    let _ = std::fs::remove_file(&temp_path);
    std::os::unix::fs::symlink(link_target, &temp_path)
        .with_context(|| format!("cannot create temporary symlink for {}", target.display()))?;
    if let Err(e) = std::fs::rename(&temp_path, target) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e).with_context(|| format!("cannot place symlink {}", target.display()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn write_symlink_atomic(target: &Path, _link_target: &str) -> Result<()> {
    bail!(
        "symlinks are not supported on this platform: cannot place {}",
        target.display()
    );
}

#[cfg(test)]
mod tests;
