//! Re-fetching managed files from their sources.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::git::{GitCache, resolve_url};
use crate::lockfile::{Lock, write_lock};
use crate::manifest::{Manifest, Source};

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
    let mut cache: Option<GitCache> = None;

    let mut commits = HashMap::new();
    let mut fetched = Vec::with_capacity(manifest.managed.len());
    for managed in &manifest.managed {
        let content = match &managed.source {
            Source::Git { git, rev, path } => {
                let pinned = match mode {
                    SyncMode::Update => None,
                    SyncMode::Locked => lock
                        .get(&managed.path)
                        .filter(|locked| locked.source == managed.source)
                        .and_then(|locked| locked.commit.as_deref()),
                };
                if cache.is_none() {
                    cache = Some(match cache_dir {
                        Some(dir) => GitCache::new(dir),
                        None => GitCache::new(GitCache::default_dir()?),
                    });
                }
                let cache = cache.as_ref().expect("cache was just initialised");
                let url = resolve_url(root, git)?;
                let file = cache
                    .fetch(&url, rev, path, pinned)
                    .with_context(|| format!("source of {}", managed.path.display()))?;
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
    use crate::git::testing::TestRepo;

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
        std::fs::write(vault.join("catasto.mtm"), MANIFEST).unwrap();
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
                vault.join("catasto.mtm"),
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
        let manifest = std::fs::read_to_string(fx.vault.join("catasto.mtm")).unwrap();
        std::fs::write(
            fx.vault.join("catasto.mtm"),
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
        let manifest = std::fs::read_to_string(fx.vault.join("catasto.mtm")).unwrap();
        std::fs::write(
            fx.vault.join("catasto.mtm"),
            manifest.replace(&fx.repo.url(), &format!("../../{repo_name}")),
        )
        .unwrap();

        let lock = fx.sync(SyncMode::Locked);

        assert_eq!(fx.content(), "one");
        assert!(lock.locked[0].commit.is_some());
        assert!(check(&fx.vault).unwrap().is_ok());
    }

    #[test]
    fn a_git_failure_leaves_the_workspace_untouched() {
        let fx = GitFixture::new();
        let manifest = std::fs::read_to_string(fx.vault.join("catasto.mtm")).unwrap();
        std::fs::write(
            fx.vault.join("catasto.mtm"),
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
