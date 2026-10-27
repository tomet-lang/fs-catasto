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

/// What each managed file looked like when it was locked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lock {
    /// Locked files, sorted by path.
    pub locked: Vec<Locked>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<LockMode>,
}

impl Lock {
    /// A lock of `locked`, sorted by path.
    pub fn new(mut locked: Vec<Locked>) -> Self {
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
        Ok(Self::new(
            document
                .files
                .into_iter()
                .map(|(path, body)| Locked {
                    path: path.into(),
                    source: body.source,
                    sha256: body.sha256,
                    commit: body.commit,
                    mode: body.mode,
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
                    mode: locked.mode,
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
