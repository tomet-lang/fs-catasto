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
use serde::{Deserialize, Serialize};

use catasto_digest::sha256_file;
use crate::manifest::{Manifest, Source};

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
    fn new(mut locked: Vec<Locked>) -> Self {
        locked.sort_by(|a, b| a.path.cmp(&b.path));
        Self { locked }
    }

    /// Loads `catasto.lock` from `root`. A missing file is an empty lock.
    pub fn load(root: &Path) -> Result<Self> {
        let file = root.join(LOCK_FILE);
        match std::fs::read_to_string(&file) {
            Ok(text) => Self::parse(&text).with_context(|| format!("invalid {}", file.display())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
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
    let mut locked = Vec::with_capacity(manifest.managed.len());
    for managed in manifest.managed {
        let file = root.join(&managed.path);
        if !file.is_file() {
            bail!("managed file does not exist: {}", managed.path.display());
        }
        let commit = match managed.source {
            Source::Path(_) => None,
            Source::Git { .. } => Some(commits.get(&managed.path).cloned().with_context(|| {
                format!(
                    "no locked commit for {}: run `catasto sync`",
                    managed.path.display()
                )
            })?),
        };
        locked.push(Locked {
            sha256: sha256_file(&file)?,
            path: managed.path,
            source: managed.source,
            commit,
        });
    }

    let lock = Lock::new(locked);
    let file = root.join(LOCK_FILE);
    std::fs::write(&file, lock.render()?)
        .with_context(|| format!("cannot write {}", file.display()))?;
    Ok(lock)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(manifest: &str, files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("catasto.motm"), manifest).unwrap();
        for (path, content) in files {
            let file = dir.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, content).unwrap();
        }
        dir
    }

    const MANIFEST: &str = "\
\"b.tmt\": { source: \"src/b.tmt\" }
\"a.tmt\": { source: \"src/a.tmt\" }
";

    #[test]
    fn lock_records_hashes_sorted_by_path() {
        let dir = workspace(MANIFEST, &[("a.tmt", "abc"), ("b.tmt", "")]);

        let lock = lock(dir.path()).unwrap();

        let paths: Vec<_> = lock
            .locked
            .iter()
            .map(|l| l.path.to_str().unwrap())
            .collect();
        assert_eq!(paths, ["a.tmt", "b.tmt"]);
        assert_eq!(
            lock.locked[0].sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn written_lock_round_trips() {
        let dir = workspace(MANIFEST, &[("a.tmt", "abc"), ("b.tmt", "x")]);

        let written = lock(dir.path()).unwrap();

        assert_eq!(Lock::load(dir.path()).unwrap(), written);
    }

    #[test]
    fn the_lock_text_is_json_keyed_by_path() {
        let dir = workspace(
            "\"a.tmt\": { source: \"src/a.tmt\" }\n",
            &[("a.tmt", "abc")],
        );

        lock(dir.path()).unwrap();

        let text = std::fs::read_to_string(dir.path().join(LOCK_FILE)).unwrap();
        assert_eq!(
            text,
            "{\n  \"version\": 1,\n  \"files\": {\n    \"a.tmt\": {\n      \"sha256\": \
             \"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\",\n      \
             \"source\": \"src/a.tmt\"\n    }\n  }\n}\n"
        );
    }

    #[test]
    fn a_file_named_version_does_not_collide_with_the_format() {
        let dir = workspace(
            "version: { source: \"src/version\" }\n",
            &[("version", "1")],
        );

        let written = lock(dir.path()).unwrap();

        assert_eq!(Lock::load(dir.path()).unwrap(), written);
        assert_eq!(written.locked[0].path, PathBuf::from("version"));
    }

    #[test]
    fn lock_is_deterministic() {
        let dir = workspace(MANIFEST, &[("a.tmt", "abc"), ("b.tmt", "x")]);

        lock(dir.path()).unwrap();
        let first = std::fs::read(dir.path().join(LOCK_FILE)).unwrap();
        lock(dir.path()).unwrap();
        let second = std::fs::read(dir.path().join(LOCK_FILE)).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn lock_fails_when_a_managed_file_is_missing() {
        let dir = workspace(MANIFEST, &[("a.tmt", "abc")]);

        let err = lock(dir.path()).unwrap_err();

        assert!(err.to_string().contains("b.tmt"), "{err}");
        assert!(!dir.path().join(LOCK_FILE).exists());
    }

    const GIT_MANIFEST: &str = "\
\"w.tmt\": { source: { git: \"https://example.com/v.git\", rev: v1, path: \"w.tmt\" } }
";

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    fn locked_git_entry(commit: &str, rev: &str) -> String {
        format!(
            "{{\"version\":1,\"files\":{{\"w.tmt\":{{\"sha256\":\"x\",\"source\":\
             {{\"git\":\"https://example.com/v.git\",\"rev\":\"{rev}\",\"path\":\"w.tmt\"}},\
             \"commit\":\"{commit}\"}}}}}}"
        )
    }

    #[test]
    fn lock_cannot_pin_a_git_source_by_itself() {
        let dir = workspace(GIT_MANIFEST, &[("w.tmt", "abc")]);

        let err = lock(dir.path()).unwrap_err();

        assert!(err.to_string().contains("run `catasto sync`"), "{err}");
    }

    #[test]
    fn lock_keeps_the_commit_of_an_unchanged_git_source() {
        let dir = workspace(GIT_MANIFEST, &[("w.tmt", "abc")]);
        std::fs::write(dir.path().join(LOCK_FILE), locked_git_entry(COMMIT, "v1")).unwrap();

        let lock = lock(dir.path()).unwrap();

        assert_eq!(lock.locked[0].commit.as_deref(), Some(COMMIT));
        assert_eq!(Lock::load(dir.path()).unwrap(), lock);
    }

    #[test]
    fn lock_drops_the_commit_when_the_rev_changed() {
        let dir = workspace(GIT_MANIFEST, &[("w.tmt", "abc")]);
        std::fs::write(dir.path().join(LOCK_FILE), locked_git_entry(COMMIT, "v0")).unwrap();

        assert!(lock(dir.path()).is_err());
    }

    #[test]
    fn path_sources_have_no_commit_in_the_lock() {
        let dir = workspace(MANIFEST, &[("a.tmt", "abc"), ("b.tmt", "x")]);

        lock(dir.path()).unwrap();

        let text = std::fs::read_to_string(dir.path().join(LOCK_FILE)).unwrap();
        assert!(!text.contains("commit"), "{text}");
    }

    #[test]
    fn missing_lock_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Lock::load(dir.path()).unwrap(), Lock::default());
    }

    #[test]
    fn an_older_toml_lock_says_what_to_do() {
        let err =
            Lock::parse("# Generated by `catasto lock`. Do not edit.\n\n[[locked]]\n").unwrap_err();
        assert!(
            err.to_string().contains("older catasto wrote TOML"),
            "{err}"
        );
    }

    #[test]
    fn a_lock_of_another_version_is_rejected() {
        let err = Lock::parse("{\"version\":2,\"files\":{}}").unwrap_err();
        assert!(
            err.to_string().contains("unsupported lock version 2"),
            "{err}"
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(Lock::parse("{\"version\":1,\"files\":{},\"extra\":1}").is_err());
        assert!(
            Lock::parse(
                "{\"version\":1,\"files\":{\"a\":{\"sha256\":\"x\",\"source\":\"s\",\"extra\":1}}}"
            )
            .is_err()
        );
    }
}
