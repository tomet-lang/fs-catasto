//! The generated `catasto.lock` data model and serialization.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::manifest::{Source, reject_misnamed};

/// File name of the lockfile, relative to the workspace root.
pub const LOCK_FILE: &str = "catasto.lock";

/// The format version this catasto reads and writes.
pub const LOCK_VERSION: u32 = 1;

/// What each managed file and repository looked like when it was locked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lock {
    /// Locked files, sorted by path.
    pub locked: Vec<Locked>,
    /// Locked repositories, sorted by path.
    pub repos: Vec<LockedRepo>,
}

/// Special mode for a locked file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LockMode {
    /// File is executable (`100755` in git).
    Executable,
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
    /// Special mode of the file, absent for normal files.
    pub mode: Option<LockMode>,
}

/// One locked repository entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedRepo {
    /// Location inside the workspace, relative to the root.
    pub path: PathBuf,
    /// Repository URL (`https://` or local path).
    pub git: String,
    /// Branch checked out and tracked, if declared.
    pub branch: Option<String>,
    /// Whether registered in `.gitmodules`.
    pub submodule: bool,
    /// Full commit id checked out at HEAD.
    pub commit: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The JSON document.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockDocument {
    version: u32,
    #[serde(default)]
    files: BTreeMap<String, LockedBody>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    repos: BTreeMap<String, LockedRepoBody>,
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
}

/// A locked repository's fields, keyed by its path in [`LockDocument`].
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedRepoBody {
    git: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    submodule: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
}

impl Lock {
    /// A lock of `locked` files and `repos`, each sorted by path.
    pub fn new(mut locked: Vec<Locked>, mut repos: Vec<LockedRepo>) -> Self {
        locked.sort_by(|a, b| a.path.cmp(&b.path));
        repos.sort_by(|a, b| a.path.cmp(&b.path));
        Self { locked, repos }
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
        let files = document
            .files
            .into_iter()
            .map(|(path, body)| Locked {
                path: path.into(),
                source: body.source,
                sha256: body.sha256,
                commit: body.commit,
                mode: body.mode,
            })
            .collect();
        let repos = document
            .repos
            .into_iter()
            .map(|(path, body)| LockedRepo {
                path: path.into(),
                git: body.git,
                branch: body.branch,
                submodule: body.submodule,
                commit: body.commit,
            })
            .collect();
        Ok(Self::new(files, repos))
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
                },
            );
        }
        let mut repos = BTreeMap::new();
        for repo in &self.repos {
            let path = repo
                .path
                .to_str()
                .ok_or_else(|| anyhow!("path is not valid UTF-8: {}", repo.path.display()))?;
            repos.insert(
                path.to_owned(),
                LockedRepoBody {
                    git: repo.git.clone(),
                    branch: repo.branch.clone(),
                    submodule: repo.submodule,
                    commit: repo.commit.clone(),
                },
            );
        }
        let document = LockDocument {
            version: LOCK_VERSION,
            files,
            repos,
        };
        let mut text = serde_json::to_string_pretty(&document)?;
        text.push('\n');
        Ok(text)
    }

    /// Looks up the locked file entry for `path`.
    pub fn get(&self, path: &Path) -> Option<&Locked> {
        self.locked.iter().find(|l| l.path == path)
    }

    /// Looks up the locked repository entry for `path`.
    pub fn get_repo(&self, path: &Path) -> Option<&LockedRepo> {
        self.repos.iter().find(|r| r.path == path)
    }
}
