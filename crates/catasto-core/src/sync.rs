//! Re-fetching managed files from their sources.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::lockfile::{Lock, write_lock};
use crate::manifest::{Managed, Manifest, Source};
use catasto_git::{Fetched, GitCache, resolve_url};

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
    let manifest = Manifest::load(root)?;
    let lock = Lock::load(root)?;
    let mut fetched_git = fetch_git(root, mode, cache_dir, &manifest, &lock)?;

    let mut commits = HashMap::new();
    let mut fetched = Vec::with_capacity(manifest.managed.len());
    for (index, managed) in manifest.managed.iter().enumerate() {
        let content = match &managed.source {
            Source::Git { .. } => {
                let file = fetched_git[index]
                    .take()
                    .expect("every git source was fetched")?;
                commits.insert(managed.path.clone(), file.commit);
                file.content
            }
            local => read_local(root, &managed.path, local)?,
        };
        fetched.push((root.join(&managed.path), content));
    }

    write_all(fetched)?;
    write_lock(root, manifest, &commits)
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
) -> Result<Vec<Option<Result<Fetched>>>> {
    let mut by_url: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, managed) in manifest.managed.iter().enumerate() {
        let Source::Git { git, .. } = &managed.source else {
            continue;
        };
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
    let Source::Git { rev, path, .. } = &managed.source else {
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
        .fetch(url, rev, path, pinned)
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

fn write_all(files: Vec<(PathBuf, Vec<u8>)>) -> Result<()> {
    for (target, content) in files {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        std::fs::write(&target, content)
            .with_context(|| format!("cannot write {}", target.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::check::check;
    use catasto_git::testing::TestRepo;

    const MANIFEST: &str = "\
\"vocab/writ.tmt\": { source: \"../vocabularies/writ.tmt\" }
";

    fn sync(root: &Path) -> Result<Lock> {
        super::sync(root, SyncMode::Locked)
    }

    fn workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        let source = tmp.path().join("vocabularies/writ.tmt");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(vault.join("catasto.ctmt"), MANIFEST).unwrap();
        std::fs::write(&source, "canonical").unwrap();
        (tmp, vault, source)
    }

    #[test]
    fn fetches_a_file_that_does_not_exist_yet() {
        let (_tmp, vault, _) = workspace();

        sync(&vault).unwrap();

        assert_eq!(
            std::fs::read_to_string(vault.join("vocab/writ.tmt")).unwrap(),
            "canonical"
        );
        assert!(check(&vault).unwrap().is_ok());
    }

    #[test]
    fn overwrites_a_drifted_file_and_clears_every_violation() {
        let (_tmp, vault, source) = workspace();
        sync(&vault).unwrap();
        std::fs::write(vault.join("vocab/writ.tmt"), "edited").unwrap();
        std::fs::write(&source, "canonical v2").unwrap();
        assert!(!check(&vault).unwrap().is_ok());

        sync(&vault).unwrap();

        assert_eq!(
            std::fs::read_to_string(vault.join("vocab/writ.tmt")).unwrap(),
            "canonical v2"
        );
        assert!(check(&vault).unwrap().is_ok());
    }

    #[test]
    fn a_missing_source_leaves_the_workspace_untouched() {
        let (_tmp, vault, source) = workspace();
        std::fs::create_dir_all(vault.join("vocab")).unwrap();
        std::fs::write(vault.join("vocab/writ.tmt"), "local").unwrap();
        std::fs::remove_file(&source).unwrap();

        let err = sync(&vault).unwrap_err();

        assert!(
            err.to_string().contains("source of vocab/writ.tmt"),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(vault.join("vocab/writ.tmt")).unwrap(),
            "local"
        );
        assert!(!vault.join("catasto.lock").exists());
    }

    /// A vault whose one file comes from the `main` branch of a test repository.
    struct GitFixture {
        _tmp: tempfile::TempDir,
        vault: PathBuf,
        cache: PathBuf,
        repo: TestRepo,
    }

    impl GitFixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let vault = tmp.path().join("vault");
            let cache = tmp.path().join("cache");
            std::fs::create_dir_all(&vault).unwrap();
            let repo = TestRepo::new();
            repo.commit("shared/w.tmt", "one");
            std::fs::write(
                vault.join("catasto.ctmt"),
                format!(
                    "\"vocab/w.tmt\": {{ source: {{ git: {:?}, rev: main, path: \"shared/w.tmt\" }} }}\n",
                    repo.url()
                ),
            )
            .unwrap();
            Self {
                _tmp: tmp,
                vault,
                cache,
                repo,
            }
        }

        fn sync(&self, mode: SyncMode) -> Lock {
            sync_with(&self.vault, mode, Some(&self.cache)).unwrap()
        }

        fn content(&self) -> String {
            std::fs::read_to_string(self.vault.join("vocab/w.tmt")).unwrap()
        }
    }

    #[test]
    fn syncing_a_git_source_writes_the_file_and_locks_the_commit() {
        let fx = GitFixture::new();
        let head = fx.repo.git(&["rev-parse", "HEAD"]);

        let lock = fx.sync(SyncMode::Locked);

        assert_eq!(fx.content(), "one");
        assert_eq!(lock.locked[0].commit.as_deref(), Some(head.as_str()));
        assert!(check(&fx.vault).unwrap().is_ok());
    }

    #[test]
    fn a_locked_sync_stays_on_the_pinned_commit_after_the_branch_moves() {
        let fx = GitFixture::new();
        let pinned = fx.sync(SyncMode::Locked).locked[0].commit.clone();
        fx.repo.commit("shared/w.tmt", "two");

        let lock = fx.sync(SyncMode::Locked);

        assert_eq!(fx.content(), "one");
        assert_eq!(lock.locked[0].commit, pinned);
    }

    #[test]
    fn a_locked_sync_restores_a_modified_file_to_the_pinned_commit() {
        let fx = GitFixture::new();
        fx.sync(SyncMode::Locked);
        std::fs::write(fx.vault.join("vocab/w.tmt"), "edited").unwrap();
        fx.repo.commit("shared/w.tmt", "two");

        fx.sync(SyncMode::Locked);

        assert_eq!(fx.content(), "one");
        assert!(check(&fx.vault).unwrap().is_ok());
    }

    #[test]
    fn update_moves_to_where_the_rev_points_now() {
        let fx = GitFixture::new();
        fx.sync(SyncMode::Locked);
        let moved = fx.repo.commit("shared/w.tmt", "two");

        let lock = fx.sync(SyncMode::Update);

        assert_eq!(fx.content(), "two");
        assert_eq!(lock.locked[0].commit.as_deref(), Some(moved.as_str()));
        assert!(check(&fx.vault).unwrap().is_ok());
    }

    #[test]
    fn a_changed_rev_is_resolved_even_in_locked_mode() {
        let fx = GitFixture::new();
        fx.sync(SyncMode::Locked);
        fx.repo.tag("v1");
        let moved = fx.repo.commit("shared/w.tmt", "two");
        fx.repo.tag("v2");
        let manifest = std::fs::read_to_string(fx.vault.join("catasto.ctmt")).unwrap();
        std::fs::write(
            fx.vault.join("catasto.ctmt"),
            manifest.replace("rev: main", "rev: v2"),
        )
        .unwrap();

        let lock = fx.sync(SyncMode::Locked);

        assert_eq!(fx.content(), "two");
        assert_eq!(lock.locked[0].commit.as_deref(), Some(moved.as_str()));
    }

    #[test]
    fn a_relative_git_path_is_resolved_against_the_workspace_root() {
        let fx = GitFixture::new();
        // Both directories are created in the system temp directory, so the
        // repository is `../../<name>` from `<tmp>/vault`.
        let repo_name = Path::new(&fx.repo.url())
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let manifest = std::fs::read_to_string(fx.vault.join("catasto.ctmt")).unwrap();
        std::fs::write(
            fx.vault.join("catasto.ctmt"),
            manifest.replace(&fx.repo.url(), &format!("../../{repo_name}")),
        )
        .unwrap();

        let lock = fx.sync(SyncMode::Locked);

        assert_eq!(fx.content(), "one");
        assert!(lock.locked[0].commit.is_some());
        assert!(check(&fx.vault).unwrap().is_ok());
    }

    /// A vault with entries from two repositories, written in an order that
    /// alternates between them.
    fn two_repositories() -> (tempfile::TempDir, PathBuf, PathBuf, TestRepo, TestRepo) {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(&vault).unwrap();
        let first = TestRepo::new();
        first.commit("a1.tmt", "first a1");
        first.commit("a2.tmt", "first a2");
        let second = TestRepo::new();
        second.commit("b1.tmt", "second b1");
        std::fs::write(
            vault.join("catasto.ctmt"),
            format!(
                "\"v/a1.tmt\": {{ source: {{ git: {first:?}, rev: main, path: \"a1.tmt\" }} }}\n\
                 \"v/b1.tmt\": {{ source: {{ git: {second:?}, rev: main, path: \"b1.tmt\" }} }}\n\
                 \"v/a2.tmt\": {{ source: {{ git: {first:?}, rev: main, path: \"a2.tmt\" }} }}\n",
                first = first.url(),
                second = second.url(),
            ),
        )
        .unwrap();
        (tmp, vault, cache, first, second)
    }

    #[test]
    fn repositories_are_read_together_and_each_entry_gets_its_own_commit() {
        let (_tmp, vault, cache, first, second) = two_repositories();

        let lock = sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();

        for (file, content) in [
            ("v/a1.tmt", "first a1"),
            ("v/b1.tmt", "second b1"),
            ("v/a2.tmt", "first a2"),
        ] {
            assert_eq!(
                std::fs::read_to_string(vault.join(file)).unwrap(),
                content,
                "{file}"
            );
        }
        let commit = |file: &str| lock.get(Path::new(file)).unwrap().commit.clone().unwrap();
        assert_eq!(commit("v/a1.tmt"), first.git(&["rev-parse", "HEAD"]));
        assert_eq!(commit("v/a2.tmt"), first.git(&["rev-parse", "HEAD"]));
        assert_eq!(commit("v/b1.tmt"), second.git(&["rev-parse", "HEAD"]));
        assert!(check(&vault).unwrap().is_ok());
    }

    #[test]
    fn several_repositories_stay_pinned_until_update() {
        let (_tmp, vault, cache, first, second) = two_repositories();
        sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();
        first.commit("a1.tmt", "moved a1");
        second.commit("b1.tmt", "moved b1");

        sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();
        assert_eq!(
            std::fs::read_to_string(vault.join("v/b1.tmt")).unwrap(),
            "second b1"
        );

        sync_with(&vault, SyncMode::Update, Some(&cache)).unwrap();
        assert_eq!(
            std::fs::read_to_string(vault.join("v/a1.tmt")).unwrap(),
            "moved a1"
        );
        assert_eq!(
            std::fs::read_to_string(vault.join("v/b1.tmt")).unwrap(),
            "moved b1"
        );
    }

    #[test]
    fn the_error_reported_is_the_first_in_manifest_order() {
        let (_tmp, vault, cache, first, second) = two_repositories();
        // Both repositories fail, the second one's entry is written first.
        std::fs::write(
            vault.join("catasto.ctmt"),
            format!(
                "\"v/b.tmt\": {{ source: {{ git: {second:?}, rev: main, path: \"missing-b.tmt\" }} }}\n\
                 \"v/a.tmt\": {{ source: {{ git: {first:?}, rev: main, path: \"missing-a.tmt\" }} }}\n",
                first = first.url(),
                second = second.url(),
            ),
        )
        .unwrap();

        for _ in 0..5 {
            let err = sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap_err();
            assert!(err.to_string().contains("source of v/b.tmt"), "{err}");
        }
        assert!(!vault.join("v").exists());
    }

    #[test]
    fn a_git_failure_leaves_the_workspace_untouched() {
        let fx = GitFixture::new();
        let manifest = std::fs::read_to_string(fx.vault.join("catasto.ctmt")).unwrap();
        std::fs::write(
            fx.vault.join("catasto.ctmt"),
            manifest.replace("rev: main", "rev: nope"),
        )
        .unwrap();

        let err = super::sync_with(&fx.vault, SyncMode::Locked, Some(&fx.cache)).unwrap_err();

        assert!(
            format!("{err:#}").contains("source of vocab/w.tmt"),
            "{err:#}"
        );
        assert!(!fx.vault.join("vocab/w.tmt").exists());
        assert!(!fx.vault.join("catasto.lock").exists());
    }
}
