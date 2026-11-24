//! The generated `catasto.lock` data model and serialization.

use std::collections::BTreeMap;
use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::manifest::{Source, Symlink, check_git, is_relative_inside, reject_misnamed};

/// File name of the lockfile, relative to the workspace root.
pub const LOCK_FILE: &str = "catasto.lock";

/// The format version this catasto reads and writes.
pub const LOCK_VERSION: u32 = 1;

/// What each managed file, directory, and submodule looked like when it was locked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lock {
    /// Locked files and directories, sorted by path.
    pub locked: Vec<Locked>,
    /// Locked submodules, sorted by path.
    pub submodules: Vec<LockedSubmodule>,
}

/// Special mode for a locked file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LockMode {
    /// File is executable (`100755` in git).
    Executable,
}

/// One locked file or directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locked {
    /// Location inside the workspace, relative to the root.
    pub path: PathBuf,
    /// The source declared when the entry was locked.
    pub source: Source,
    /// Lowercase hex SHA-256 of the file content or directory tree.
    pub sha256: String,
    /// Full commit id the entry was read at. Present exactly for git sources.
    pub commit: Option<String>,
    /// Special mode of the file, absent for normal files and directories.
    pub mode: Option<LockMode>,
    /// How symlinks in a git source are handled (`Follow` by default, or `Keep`).
    pub symlink: Symlink,
}

/// One locked submodule entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedSubmodule {
    /// Location inside the workspace, relative to the root.
    pub path: PathBuf,
    /// Repository URL (`https://` or local path).
    pub git: String,
    /// Branch, tag, or commit checked out and tracked.
    pub rev: String,
    /// Optional subdirectory or file inside the repository for sparse-checkout.
    pub subpath: Option<PathBuf>,
    /// Full commit id checked out at HEAD.
    pub commit: Option<String>,
}

/// The JSON document.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockDocument {
    version: u32,
    #[serde(default)]
    files: BTreeMap<String, LockedBody>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    submodules: BTreeMap<String, LockedSubmoduleBody>,
}

fn is_follow(symlink: &Symlink) -> bool {
    matches!(symlink, Symlink::Follow)
}

fn deserialize_symlink<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Symlink, D::Error> {
    struct SymlinkVisitor;

    impl Visitor<'_> for SymlinkVisitor {
        type Value = Symlink;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("`keep`")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Symlink, E> {
            if v == "keep" {
                Ok(Symlink::Keep)
            } else {
                Err(E::custom(
                    "invalid symlink option: `keep` is the only supported value",
                ))
            }
        }

        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Symlink, E> {
            Err(E::custom(
                "invalid symlink option: `keep` is the only supported value",
            ))
        }

        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Symlink, E> {
            Err(E::custom(
                "invalid symlink option: `keep` is the only supported value",
            ))
        }

        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Symlink, E> {
            Err(E::custom(
                "invalid symlink option: `keep` is the only supported value",
            ))
        }

        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Symlink, E> {
            Err(E::custom(
                "invalid symlink option: `keep` is the only supported value",
            ))
        }
    }

    deserializer.deserialize_any(SymlinkVisitor)
}

/// A locked file's fields, keyed by its path in [`LockDocument`].
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedBody {
    sha256: String,
    source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<LockMode>,
    #[serde(
        default,
        skip_serializing_if = "is_follow",
        deserialize_with = "deserialize_symlink"
    )]
    symlink: Symlink,
}

/// A locked submodule's fields, keyed by its path in [`LockDocument`].
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedSubmoduleBody {
    git: String,
    rev: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
}

impl Lock {
    /// A lock of `locked` files and `submodules`, each sorted by path.
    pub fn new(mut locked: Vec<Locked>, mut submodules: Vec<LockedSubmodule>) -> Self {
        locked.sort_by(|a, b| a.path.cmp(&b.path));
        submodules.sort_by(|a, b| a.path.cmp(&b.path));
        Self { locked, submodules }
    }

    /// A lock containing only locked files.
    pub fn from_files(locked: Vec<Locked>) -> Self {
        Self::new(locked, Vec::new())
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

    /// Parses the JSON text of a lockfile.
    pub fn parse(text: &str) -> Result<Self> {
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
        let mut files = Vec::with_capacity(document.files.len());
        for (path, body) in document.files {
            if body.symlink == Symlink::Keep && matches!(body.source, Source::Path(_)) {
                bail!("file `{path}`: local source cannot specify `symlink`");
            }
            files.push(Locked {
                path: path.into(),
                source: body.source,
                sha256: body.sha256,
                commit: body.commit,
                mode: body.mode,
                symlink: body.symlink,
            });
        }
        let mut submodules = Vec::with_capacity(document.submodules.len());
        for (path, body) in document.submodules {
            check_git(&body.git, &body.rev).map_err(|e| anyhow!("submodule `{path}`: {e}"))?;
            if let Some(ref p) = body.path
                && !is_relative_inside(p)
            {
                bail!(
                    "submodule `{path}`: git path must be relative and stay inside the repository: {}",
                    p.display()
                );
            }
            submodules.push(LockedSubmodule {
                path: path.into(),
                git: body.git,
                rev: body.rev,
                subpath: body.path,
                commit: body.commit,
            });
        }
        Ok(Self::new(files, submodules))
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
                    mode: locked.mode,
                    symlink: locked.symlink,
                },
            );
        }
        let mut submodules = BTreeMap::new();
        for sub in &self.submodules {
            let path = sub
                .path
                .to_str()
                .ok_or_else(|| anyhow!("path is not valid UTF-8: {}", sub.path.display()))?;
            submodules.insert(
                path.to_owned(),
                LockedSubmoduleBody {
                    git: sub.git.clone(),
                    rev: sub.rev.clone(),
                    path: sub.subpath.clone(),
                    commit: sub.commit.clone(),
                },
            );
        }
        let document = LockDocument {
            version: LOCK_VERSION,
            files,
            submodules,
        };
        let mut text = serde_json::to_string_pretty(&document)?;
        text.push('\n');
        Ok(text)
    }

    /// Looks up the locked file entry for `path`.
    pub fn get(&self, path: &Path) -> Option<&Locked> {
        self.locked.iter().find(|l| l.path == path)
    }

    /// Looks up the locked submodule entry for `path`.
    pub fn get_submodule(&self, path: &Path) -> Option<&LockedSubmodule> {
        self.submodules.iter().find(|s| s.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Symlink;

    #[test]
    fn round_trips_files_and_submodules() {
        let lock = Lock::new(
            vec![
                Locked {
                    path: "dir".into(),
                    source: Source::Git {
                        git: "https://example.com/a.git".into(),
                        rev: "main".into(),
                        path: None,
                    },
                    sha256: "abc".into(),
                    commit: Some("deadbeef".into()),
                    mode: None,
                    symlink: Symlink::Follow,
                },
                Locked {
                    path: "link.tmt".into(),
                    source: Source::Git {
                        git: "https://example.com/a.git".into(),
                        rev: "v1".into(),
                        path: Some("link.tmt".into()),
                    },
                    sha256: "ghi".into(),
                    commit: Some("deadbeef".into()),
                    mode: None,
                    symlink: Symlink::Keep,
                },
                Locked {
                    path: "run.sh".into(),
                    source: Source::Git {
                        git: "https://example.com/a.git".into(),
                        rev: "v1".into(),
                        path: Some("bin/run.sh".into()),
                    },
                    sha256: "def".into(),
                    commit: Some("deadbeef".into()),
                    mode: Some(LockMode::Executable),
                    symlink: Symlink::Follow,
                },
            ],
            vec![LockedSubmodule {
                path: "tools/sisho".into(),
                git: "https://example.com/sisho.git".into(),
                rev: "main".into(),
                subpath: Some("crates/sisho".into()),
                commit: Some("cafebabe".into()),
            }],
        );

        let rendered = lock.render().unwrap();
        assert!(!rendered.contains("\"repos\""), "{rendered}");
        assert!(rendered.contains("\"submodules\""), "{rendered}");
        assert!(rendered.contains("\"symlink\": \"keep\""), "{rendered}");
        let parsed = Lock::parse(&rendered).unwrap();
        assert_eq!(parsed, lock);
        assert_eq!(
            parsed.get_submodule(Path::new("tools/sisho")),
            Some(&lock.submodules[0])
        );
    }

    #[test]
    fn rejects_legacy_repos_field_and_invalid_submodule_fields() {
        let legacy = r#"{
  "version": 1,
  "files": {},
  "repos": {
    "tools/sisho": { "git": "https://example.com/sisho.git" }
  }
}"#;
        assert!(Lock::parse(legacy).is_err());

        let bad_url = r#"{
  "version": 1,
  "files": {},
  "submodules": {
    "tools/sisho": { "git": "git@github.com:org/repo.git", "rev": "main" }
  }
}"#;
        assert!(Lock::parse(bad_url).is_err());

        let bad_subpath = r#"{
  "version": 1,
  "files": {},
  "submodules": {
    "tools/sisho": { "git": "https://example.com/sisho.git", "rev": "main", "path": "../outside" }
  }
}"#;
        assert!(Lock::parse(bad_subpath).is_err());

        let symlink_in_source = r#"{
  "version": 1,
  "files": {
    "a": {
      "sha256": "abc",
      "source": { "git": "https://example.com/a.git", "rev": "main", "path": "a", "symlink": "keep" },
      "commit": "0123456789abcdef0123456789abcdef01234567"
    }
  }
}"#;
        assert!(Lock::parse(symlink_in_source).is_err());

        let symlink_on_local = r#"{
  "version": 1,
  "files": {
    "a": {
      "sha256": "abc",
      "source": "src/a",
      "symlink": "keep"
    }
  }
}"#;
        assert!(Lock::parse(symlink_on_local).is_err());
    }
}
