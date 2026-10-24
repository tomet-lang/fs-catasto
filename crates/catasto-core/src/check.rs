//! Drift detection between the manifest, the lockfile, and the files on disk.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use catasto_digest::sha256_file;
use crate::lockfile::Lock;
use crate::manifest::{Manifest, Source};

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

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Modified => "modified: differs from the locked sha256",
            Self::Missing => "missing: managed file does not exist",
            Self::Stale => "stale: source has changed since it was locked",
            Self::Unlocked => "unlocked: not in catasto.lock (run `catasto lock`)",
            Self::Orphaned => "orphaned: in catasto.lock but not declared in catasto.motm",
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

/// Checks the managed files under `root` against `catasto.motm` and `catasto.lock`.
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
    let mut violations = Vec::new();
    let mut report = |path: &Path, kind| {
        violations.push(Violation {
            path: path.to_path_buf(),
            kind,
        });
    };

    for managed in &manifest.managed {
        let Some(locked) = lock.get(&managed.path).filter(|locked| {
            locked.source == managed.source
                && matches!(managed.source, Source::Git { .. }) == locked.commit.is_some()
        }) else {
            report(&managed.path, ViolationKind::Unlocked);
            continue;
        };

        let file = root.join(&managed.path);
        if !file.is_file() {
            report(&managed.path, ViolationKind::Missing);
        } else if sha256_file(&file)? != locked.sha256 {
            report(&managed.path, ViolationKind::Modified);
        }

        let readable_source = managed.source.local_path(root).filter(|s| s.is_file());
        if let Some(source) = readable_source {
            if sha256_file(&source)? != locked.sha256 {
                report(&managed.path, ViolationKind::Stale);
            }
        }
    }

    for locked in &lock.locked {
        if !manifest.managed.iter().any(|m| m.path == locked.path) {
            report(&locked.path, ViolationKind::Orphaned);
        }
    }

    Ok(CheckReport {
        root: root.to_path_buf(),
        violations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lockfile::lock;

    const MANIFEST: &str = "\
\"vocab/writ.tmt\": { source: \"../vocabularies/writ.tmt\" }
";

    /// A vault at `<tmp>/vault` with a sibling `<tmp>/vocabularies`, already locked.
    struct Fixture {
        _tmp: tempfile::TempDir,
        vault: PathBuf,
        source: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let vault = tmp.path().join("vault");
            let source = tmp.path().join("vocabularies/writ.tmt");
            std::fs::create_dir_all(vault.join("vocab")).unwrap();
            std::fs::create_dir_all(source.parent().unwrap()).unwrap();
            std::fs::write(vault.join("catasto.motm"), MANIFEST).unwrap();
            std::fs::write(vault.join("vocab/writ.tmt"), "v1").unwrap();
            std::fs::write(&source, "v1").unwrap();
            lock(&vault).unwrap();
            Self {
                _tmp: tmp,
                vault,
                source,
            }
        }

        fn kinds(&self) -> Vec<ViolationKind> {
            check(&self.vault)
                .unwrap()
                .violations
                .into_iter()
                .map(|v| v.kind)
                .collect()
        }
    }

    #[test]
    fn clean_workspace_has_no_violations() {
        assert!(Fixture::new().kinds().is_empty());
    }

    #[test]
    fn detects_a_modified_file() {
        let fx = Fixture::new();
        std::fs::write(fx.vault.join("vocab/writ.tmt"), "edited").unwrap();

        assert_eq!(fx.kinds(), [ViolationKind::Modified]);
    }

    #[test]
    fn detects_a_missing_file() {
        let fx = Fixture::new();
        std::fs::remove_file(fx.vault.join("vocab/writ.tmt")).unwrap();

        assert_eq!(fx.kinds(), [ViolationKind::Missing]);
    }

    #[test]
    fn detects_a_stale_file_when_the_source_moved_on() {
        let fx = Fixture::new();
        std::fs::write(&fx.source, "v2").unwrap();

        assert_eq!(fx.kinds(), [ViolationKind::Stale]);
    }

    #[test]
    fn reports_modified_and_stale_together() {
        let fx = Fixture::new();
        std::fs::write(fx.vault.join("vocab/writ.tmt"), "edited").unwrap();
        std::fs::write(&fx.source, "v2").unwrap();

        assert_eq!(fx.kinds(), [ViolationKind::Modified, ViolationKind::Stale]);
    }

    #[test]
    fn an_unreachable_source_is_not_stale() {
        let fx = Fixture::new();
        std::fs::remove_file(&fx.source).unwrap();

        assert!(fx.kinds().is_empty());
    }

    #[test]
    fn detects_a_declaration_missing_from_the_lock() {
        let fx = Fixture::new();
        std::fs::remove_file(fx.vault.join("catasto.lock")).unwrap();

        assert_eq!(fx.kinds(), [ViolationKind::Unlocked]);
    }

    #[test]
    fn a_changed_source_declaration_needs_relocking() {
        let fx = Fixture::new();
        std::fs::write(
            fx.vault.join("catasto.motm"),
            MANIFEST.replace("../vocabularies/writ.tmt", "../elsewhere/writ.tmt"),
        )
        .unwrap();

        assert_eq!(fx.kinds(), [ViolationKind::Unlocked]);
    }

    #[test]
    fn detects_a_locked_file_that_is_no_longer_declared() {
        let fx = Fixture::new();
        std::fs::remove_file(fx.vault.join("catasto.motm")).unwrap();

        assert_eq!(fx.kinds(), [ViolationKind::Orphaned]);
    }

    #[test]
    fn unmanaged_files_are_ignored() {
        let fx = Fixture::new();
        std::fs::write(fx.vault.join("vocab/other.tmt"), "mine").unwrap();

        assert!(fx.kinds().is_empty());
    }

    #[test]
    fn a_workspace_without_catasto_files_is_clean() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check(dir.path()).unwrap().is_ok());
    }

    #[test]
    fn a_missing_root_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check(&dir.path().join("nope")).is_err());
    }

    const GIT_SOURCE: &str = "{ git: \"/does/not/exist.git\", rev: main, path: \"w.tmt\" }";

    /// A vault with one git-sourced file whose lock entry records `commit`.
    fn git_vault(commit: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("w.tmt"), "abc").unwrap();
        std::fs::write(
            dir.path().join("catasto.motm"),
            format!("\"w.tmt\": {{ source: {GIT_SOURCE} }}\n"),
        )
        .unwrap();
        let commit = commit
            .map(|c| format!(",\"commit\":\"{c}\""))
            .unwrap_or_default();
        std::fs::write(
            dir.path().join("catasto.lock"),
            format!(
                "{{\"version\":1,\"files\":{{\"w.tmt\":{{\"sha256\":\
                 \"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\",\
                 \"source\":{{\"git\":\"/does/not/exist.git\",\"rev\":\"main\",\"path\":\"w.tmt\"}}{commit}}}}}}}"
            ),
        )
        .unwrap();
        dir
    }

    #[test]
    fn a_git_source_checks_offline() {
        let dir = git_vault(Some("0123456789abcdef0123456789abcdef01234567"));

        assert!(check(dir.path()).unwrap().is_ok());
    }

    #[test]
    fn a_git_entry_without_a_commit_needs_syncing() {
        let dir = git_vault(None);

        let kinds: Vec<_> = check(dir.path())
            .unwrap()
            .violations
            .into_iter()
            .map(|v| v.kind)
            .collect();

        assert_eq!(kinds, [ViolationKind::Unlocked]);
    }

    #[test]
    fn a_modified_git_file_is_still_detected_offline() {
        let dir = git_vault(Some("0123456789abcdef0123456789abcdef01234567"));
        std::fs::write(dir.path().join("w.tmt"), "edited").unwrap();

        let kinds: Vec<_> = check(dir.path())
            .unwrap()
            .violations
            .into_iter()
            .map(|v| v.kind)
            .collect();

        assert_eq!(kinds, [ViolationKind::Modified]);
    }
}
