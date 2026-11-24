//! Re-fetching managed files and repositories from their sources.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::lockfile::{Lock, LockMode, write_lock_selected_with_hashes};
use crate::manifest::{Managed, ManagedKind, Manifest, Source};
use catasto_git::{Content, DirEntry, DirEntryContent, Fetched, GitCache, resolve_url};

/// How `sync` treats git sources that the lock already pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Read the commit recorded in `catasto.lock`. A source whose declaration
    /// changed, or that has no recorded commit, is resolved from its `rev`.
    Locked,
    /// Resolve every `rev` again and lock the commits it points at now.
    Update,
}

/// Options controlling a [`sync_with_options`] invocation.
#[derive(Debug, Clone)]
pub struct SyncOptions<'a> {
    pub mode: SyncMode,
    pub cache_dir: Option<&'a Path>,
    pub only: &'a [PathBuf],
    pub dry_run: bool,
}

/// Result of [`sync_with_options`].
#[derive(Debug, Clone)]
pub enum SyncOutcome {
    Applied(Lock),
    DryRun(DryRunReport),
}

/// Copies every managed file and clones every managed repository from its source,
/// then rewrites `catasto.lock`.
///
/// Every file source is read before anything is written, so a missing source leaves
/// the workspace untouched. Git sources are cached under the default cache
/// directory; see [`GitCache::default_dir`].
pub fn sync(root: &Path, mode: SyncMode) -> Result<Lock> {
    sync_with(root, mode, None)
}

/// Like [`sync`], with an explicit git cache directory.
pub fn sync_with(root: &Path, mode: SyncMode, cache_dir: Option<&Path>) -> Result<Lock> {
    sync_only(root, mode, cache_dir, &[])
}

/// What `sync` would write for one managed file or directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Workspace-relative path of the managed file or directory (as in manifest).
    pub path: PathBuf,
    /// Absolute path where it would be written on disk.
    pub destination: PathBuf,
    /// What `sync` would write: file bytes, link target, or directory tree.
    pub content: Content,
    /// The commit it came from (for git sources).
    pub commit: Option<String>,
    /// The canonical source from the manifest.
    pub source: Source,
    /// The declared upstream name, if the entry was written via `catasto.upstream(...)`.
    pub upstream: Option<String>,
    /// Whether the file is executable (`100755` in git). Always `false` for directories.
    pub executable: bool,
    /// The executable override explicitly declared in the manifest, if any.
    pub declared_executable: Option<bool>,
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
        let ManagedKind::File { source, .. } = &managed.kind else {
            continue;
        };
        let (content, commit, executable) = match source {
            Source::Git { .. } => {
                let file = fetched_git[index]
                    .take()
                    .expect("every git source was fetched")?;
                (file.content, Some(file.commit), file.executable)
            }
            local => (read_local(root, &managed.path, local)?, None, false),
        };
        let executable = match &content {
            Content::Dir(_) => false,
            Content::File(_) | Content::Link(_) => managed.is_executable(executable),
        };
        files.push(PlannedFile {
            path: managed.path.clone(),
            destination: root.join(&managed.path),
            content,
            commit,
            source: source.clone(),
            upstream: managed.upstream.as_ref().map(|u| u.name.clone()),
            executable,
            declared_executable: managed.executable(),
        });
    }

    Ok(SyncPlan {
        manifest,
        lock,
        files,
    })
}

/// Like [`sync_with`], for the entries in `only` (paths as the manifest writes
/// them); an empty `only` is every entry.
pub fn sync_only(
    root: &Path,
    mode: SyncMode,
    cache_dir: Option<&Path>,
    only: &[PathBuf],
) -> Result<Lock> {
    match sync_with_options(
        root,
        &SyncOptions {
            mode,
            cache_dir,
            only,
            dry_run: false,
        },
    )? {
        SyncOutcome::Applied(lock) => Ok(lock),
        SyncOutcome::DryRun(_) => unreachable!("dry_run was false"),
    }
}

/// Runs `sync` (when `dry_run` is false) or previews its changes without modifying
/// files, cloning repositories, or touching `.gitmodules` / `catasto.lock`.
pub fn sync_with_options(root: &Path, options: &SyncOptions<'_>) -> Result<SyncOutcome> {
    if options.dry_run {
        let report = run_dry_run(root, options.mode, options.cache_dir, options.only)?;
        return Ok(SyncOutcome::DryRun(report));
    }

    let plan = plan_sync(root, options.mode, options.cache_dir, options.only)?;
    let mut commits = HashMap::new();
    let mut modes = HashMap::new();
    let mut hashes = HashMap::new();
    let mut fetched = Vec::with_capacity(plan.files.len());

    for file in &plan.files {
        if let Some(commit) = &file.commit {
            commits.insert(file.path.clone(), commit.clone());
        }
        if file.executable && matches!(file.content, Content::File(_)) {
            modes.insert(file.path.clone(), LockMode::Executable);
        }
        let hash = file.content.sha256(file.declared_executable);
        hashes.insert(file.path.clone(), hash);
        fetched.push(SyncItem {
            destination: file.destination.clone(),
            content: file.content.clone(),
            executable: file.executable,
            declared_executable: file.declared_executable,
        });
    }

    write_all(fetched)?;
    let selected = |path: &Path| options.only.is_empty() || options.only.iter().any(|p| p == path);

    for managed in &plan.manifest.managed {
        if !selected(&managed.path) {
            continue;
        }
        let ManagedKind::Submodule(sub) = &managed.kind else {
            continue;
        };
        let dest = root.join(&managed.path);
        let resolved_url = resolve_url(root, &sub.git)?;
        let commit = if !dest.exists() {
            let pinned = (options.mode == SyncMode::Locked)
                .then(|| {
                    plan.lock
                        .get_submodule(&managed.path)
                        .filter(|locked| {
                            locked.git == sub.git
                                && locked.rev == sub.rev
                                && locked.subpath == sub.path
                        })
                        .and_then(|locked| locked.commit.as_deref())
                })
                .flatten();
            catasto_git::clone_submodule(
                &resolved_url,
                &dest,
                &sub.rev,
                pinned,
                sub.path.as_deref(),
            )?
        } else {
            if !dest.join(".git").exists() {
                bail!(
                    "destination {} exists and is not a git repository",
                    managed.path.display()
                );
            }
            let actual_url = catasto_git::remote_url(&dest)
                .with_context(|| format!("cannot inspect remote of {}", managed.path.display()))?;
            if actual_url != sub.git && actual_url != resolved_url {
                bail!(
                    "repository at {} has remote {actual_url}, expected {}",
                    managed.path.display(),
                    sub.git
                );
            }
            match options.mode {
                SyncMode::Update => {
                    catasto_git::update_submodule(&dest, &resolved_url, &sub.rev, sub.path.as_deref())?
                }
                SyncMode::Locked => {
                    catasto_git::apply_sparse_checkout(&dest, sub.path.as_deref())?;
                    catasto_git::head_commit(&dest)?
                }
            }
        };
        commits.insert(managed.path.clone(), commit);
    }

    crate::submodule::sync_gitmodules(root, &plan.manifest, &plan.lock)?;

    let lock = write_lock_selected_with_hashes(
        root,
        plan.manifest,
        &commits,
        &modes,
        &plan.lock,
        &selected,
        &hashes,
    )?;
    Ok(SyncOutcome::Applied(lock))
}

/// What `sync --dry-run` would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DryRunReport {
    /// Rendered output to print to stdout.
    pub output: String,
    /// Number of entries that would be changed.
    pub changed_files: usize,
    /// Total number of entries selected / checked.
    pub total_files: usize,
    /// Moved git sources (`(label, rev, from, to)`).
    pub moved_sources: Vec<MovedSource>,
}

/// A git source that moves to another commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedSource {
    pub label: String,
    pub rev: String,
    pub from: String,
    pub to: String,
}

/// Previews the changes that `sync` with `mode` would apply, without modifying
/// any workspace files, cloning repositories, or touching `.gitmodules`, the lock, or the manifest.
pub fn sync_dry_run(
    root: &Path,
    mode: SyncMode,
    cache_dir: Option<&Path>,
    only: &[PathBuf],
) -> Result<DryRunReport> {
    match sync_with_options(
        root,
        &SyncOptions {
            mode,
            cache_dir,
            only,
            dry_run: true,
        },
    )? {
        SyncOutcome::DryRun(report) => Ok(report),
        SyncOutcome::Applied(_) => unreachable!("dry_run was true"),
    }
}

fn run_dry_run(
    root: &Path,
    mode: SyncMode,
    cache_dir: Option<&Path>,
    only: &[PathBuf],
) -> Result<DryRunReport> {
    let plan = plan_sync(root, mode, cache_dir, only)?;

    let mut moved_sources = Vec::new();
    let mut seen_groups = std::collections::HashSet::new();

    for file in &plan.files {
        let Source::Git { git, rev, .. } = &file.source else {
            continue;
        };
        let old_commit = plan.lock.get(&file.path).and_then(|l| l.commit.as_deref());
        let new_commit = file.commit.as_deref();

        if let (Some(old), Some(new)) = (old_commit, new_commit)
            && old != new
        {
            let label = file.upstream.clone().unwrap_or_else(|| git.clone());

            if seen_groups.insert((label.clone(), rev.clone())) {
                moved_sources.push(MovedSource {
                    label,
                    rev: rev.clone(),
                    from: old[..old.len().min(7)].to_owned(),
                    to: new[..new.len().min(7)].to_owned(),
                });
            }
        }
    }

    let mut output = String::new();
    for moved in &moved_sources {
        output.push_str(&format!(
            "{} {}: {} -> {}\n",
            moved.label, moved.rev, moved.from, moved.to
        ));
    }

    let mut total_files = plan.files.len();
    let mut changed_files = 0;
    for file in &plan.files {
        if let Some(hunk) = crate::diff::diff_file(file)? {
            output.push_str(&hunk);
            changed_files += 1;
        }
    }

    let selected = |path: &Path| only.is_empty() || only.iter().any(|p| p == path);
    for managed in &plan.manifest.managed {
        if !selected(&managed.path) {
            continue;
        }
        let ManagedKind::Submodule(sub) = &managed.kind else {
            continue;
        };
        total_files += 1;
        let dest = root.join(&managed.path);
        if !dest.exists() || !dest.join(".git").exists() {
            output.push_str(&format!(
                "would clone submodule {} ({})\n",
                managed.path.display(),
                sub.git
            ));
            changed_files += 1;
        } else if mode == SyncMode::Update {
            output.push_str(&format!(
                "would update submodule {} ({})\n",
                managed.path.display(),
                sub.git
            ));
            changed_files += 1;
        }
    }

    if let Some(drift) = crate::submodule::check_gitmodules(root, &plan.manifest, &plan.lock)? {
        let action = match drift {
            crate::submodule::GitmodulesDrift::Missing => "would create .gitmodules\n",
            crate::submodule::GitmodulesDrift::Modified => "would update .gitmodules\n",
            crate::submodule::GitmodulesDrift::Orphaned => "would remove .gitmodules\n",
        };
        output.push_str(action);
        changed_files += 1;
    }

    if changed_files == 0 {
        output.push_str("nothing would change\n");
    } else {
        output.push_str(&format!(
            "would change {changed_files} of {total_files} file(s)\n"
        ));
    }

    Ok(DryRunReport {
        output,
        changed_files,
        total_files,
        moved_sources,
    })
}

/// Reads every git file source, one result per entry of the manifest (`None` for
/// entries that are not git file sources).
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
        let ManagedKind::File {
            source: Source::Git { git, .. },
            ..
        } = &managed.kind
        else {
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
    let ManagedKind::File {
        source: source @ Source::Git { rev, path, .. },
        symlink,
        ..
    } = &managed.kind
    else {
        unreachable!("only git file sources are grouped by URL");
    };
    let pinned = match mode {
        SyncMode::Update => None,
        SyncMode::Locked => lock
            .get(&managed.path)
            .filter(|locked| &locked.source == source && locked.symlink == *symlink)
            .and_then(|locked| locked.commit.as_deref()),
    };
    cache
        .fetch(url, rev, path.as_deref(), pinned, (*symlink).into())
        .with_context(|| format!("source of {}", managed.path.display()))
}

pub(crate) fn read_local(root: &Path, managed: &Path, source: &Source) -> Result<Content> {
    let Some(file) = source.local_path(root) else {
        unreachable!("only local sources are read from the filesystem");
    };
    let meta = std::fs::symlink_metadata(&file).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "source of {} does not exist: {}",
                managed.display(),
                file.display()
            )
        } else {
            anyhow::Error::new(e).context(format!("cannot inspect {}", file.display()))
        }
    })?;
    if meta.is_dir() {
        let mut entries = Vec::new();
        collect_local_dir_entries(&file, Path::new(""), &mut entries)?;
        entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        return Ok(Content::Dir(entries));
    }
    if !file.is_file() {
        bail!(
            "source of {} does not exist: {}",
            managed.display(),
            file.display()
        );
    }
    let bytes = std::fs::read(&file).with_context(|| format!("cannot read {}", file.display()))?;
    Ok(Content::File(bytes))
}

fn collect_local_dir_entries(
    dir: &Path,
    prefix: &Path,
    entries: &mut Vec<DirEntry>,
) -> Result<()> {
    let read_dir =
        std::fs::read_dir(dir).with_context(|| format!("cannot read directory {}", dir.display()))?;
    for item in read_dir {
        let item = item.with_context(|| format!("cannot read entry in {}", dir.display()))?;
        let entry_path = item.path();
        let rel_path = if prefix.as_os_str().is_empty() {
            PathBuf::from(item.file_name())
        } else {
            prefix.join(item.file_name())
        };
        let meta = std::fs::symlink_metadata(&entry_path)
            .with_context(|| format!("cannot stat {}", entry_path.display()))?;
        if meta.is_symlink() {
            let target = std::fs::read_link(&entry_path)
                .with_context(|| format!("cannot read link {}", entry_path.display()))?;
            let target_str = target
                .to_str()
                .with_context(|| {
                    format!("symlink target is not valid UTF-8: {}", target.display())
                })?
                .to_owned();
            entries.push(DirEntry {
                rel_path,
                content: DirEntryContent::Link(target_str),
            });
        } else if meta.is_dir() {
            collect_local_dir_entries(&entry_path, &rel_path, entries)?;
        } else if meta.is_file() {
            let bytes = std::fs::read(&entry_path)
                .with_context(|| format!("cannot read {}", entry_path.display()))?;
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                meta.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let executable = false;
            entries.push(DirEntry {
                rel_path,
                content: DirEntryContent::File { bytes, executable },
            });
        } else {
            bail!("unsupported file type at {}", entry_path.display());
        }
    }
    Ok(())
}

struct SyncItem {
    destination: PathBuf,
    content: Content,
    executable: bool,
    declared_executable: Option<bool>,
}

fn write_all(files: Vec<SyncItem>) -> Result<()> {
    for file in files {
        write_one(&file)?;
    }
    Ok(())
}

fn write_one(item: &SyncItem) -> Result<()> {
    if let Some(parent) = item.destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    match &item.content {
        Content::File(bytes) => {
            if std::fs::symlink_metadata(&item.destination).is_ok_and(|m| m.is_dir()) {
                std::fs::remove_dir_all(&item.destination).with_context(|| {
                    format!("cannot replace directory {}", item.destination.display())
                })?;
            }
            write_file_replacing_symlink(&item.destination, bytes)?;
            apply_executable_mode(&item.destination, item.executable)?;
        }
        Content::Link(link_target) => {
            if std::fs::symlink_metadata(&item.destination).is_ok_and(|m| m.is_dir()) {
                std::fs::remove_dir_all(&item.destination).with_context(|| {
                    format!("cannot replace directory {}", item.destination.display())
                })?;
            }
            write_symlink_atomic(&item.destination, link_target)?;
        }
        Content::Dir(entries) => {
            if let Ok(m) = std::fs::symlink_metadata(&item.destination) {
                if m.is_dir() {
                    std::fs::remove_dir_all(&item.destination).with_context(|| {
                        format!("cannot clean directory {}", item.destination.display())
                    })?;
                } else {
                    std::fs::remove_file(&item.destination)
                        .with_context(|| format!("cannot replace {}", item.destination.display()))?;
                }
            }
            std::fs::create_dir_all(&item.destination).with_context(|| {
                format!("cannot create directory {}", item.destination.display())
            })?;
            for entry in entries {
                let child = match &entry.content {
                    DirEntryContent::File { bytes, executable } => SyncItem {
                        destination: item.destination.join(&entry.rel_path),
                        content: Content::File(bytes.clone()),
                        executable: item.declared_executable.unwrap_or(*executable),
                        declared_executable: None,
                    },
                    DirEntryContent::Link(target) => SyncItem {
                        destination: item.destination.join(&entry.rel_path),
                        content: Content::Link(target.clone()),
                        executable: false,
                        declared_executable: None,
                    },
                };
                write_one(&child)?;
            }
        }
    }
    Ok(())
}

/// Makes the executable bits of `target` agree with `is_executable`, which is what `check` asks for: a file the
/// lock records as executable has them, any other has none. Only the three executable bits move; the rest of the
/// mode stays as it is. This does not depend on what an earlier lock said, so a lock written before the mode was
/// recorded (no `mode` anywhere) is brought in line too.
#[cfg(unix)]
fn apply_executable_mode(target: &Path, is_executable: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(target)
        .with_context(|| format!("cannot inspect {}", target.display()))?;
    let mode = meta.permissions().mode();
    let new_mode = if is_executable {
        mode | 0o111
    } else {
        mode & !0o111
    };
    if new_mode != mode {
        std::fs::set_permissions(target, std::fs::Permissions::from_mode(new_mode)).with_context(
            || {
                let verb = if is_executable { "set" } else { "clear" };
                format!(
                    "cannot {verb} executable permissions on {}",
                    target.display()
                )
            },
        )?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_executable_mode(_target: &Path, _is_executable: bool) -> Result<()> {
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
