//! `catasto-git` -- reading files out of git repositories through the system `git`.
//!
//! Each repository URL gets one bare clone in the cache directory. Nothing is
//! written into the workspace.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use catasto_digest::sha256_hex;

mod cmd;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod tree;
mod url;

pub use url::{UrlKind, classify, resolve_url};

/// Environment variable that overrides the cache directory.
pub const CACHE_DIR_ENV: &str = "CATASTO_CACHE_DIR";

/// How symlinks in git repositories are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SymlinkMode {
    /// Resolve symlinks inside the repository and return the target file (default).
    #[default]
    Follow,
    /// Keep the symlink itself; the entry must be a symlink.
    Keep,
}

impl From<catasto_manifest::manifest::Symlink> for SymlinkMode {
    fn from(s: catasto_manifest::manifest::Symlink) -> Self {
        match s {
            catasto_manifest::manifest::Symlink::Follow => Self::Follow,
            catasto_manifest::manifest::Symlink::Keep => Self::Keep,
        }
    }
}

/// Content read from a git repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// Regular file content.
    File(Vec<u8>),
    /// Symlink target path string.
    Link(String),
}

impl Content {
    /// Returns the bytes of the file, or `None` if it is a symlink.
    pub fn file_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::File(bytes) => Some(bytes),
            Self::Link(_) => None,
        }
    }

    /// Returns the symlink target, or `None` if it is a file.
    pub fn link_target(&self) -> Option<&str> {
        match self {
            Self::Link(target) => Some(target),
            Self::File(_) => None,
        }
    }
}

/// A file or link read from a repository, with the commit it was read at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// Full commit id the revision resolved to (or was pinned to).
    pub commit: String,
    /// Content of the file or link at that commit.
    pub content: Content,
}

/// Bare clones of the repositories catasto reads from.
#[derive(Debug, Clone)]
pub struct GitCache {
    dir: PathBuf,
}

impl GitCache {
    /// A cache rooted at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The default cache directory: `$CATASTO_CACHE_DIR`, else
    /// `$XDG_CACHE_HOME/catasto`, else `~/.cache/catasto`.
    pub fn default_dir() -> Result<PathBuf> {
        if let Some(dir) = non_empty_env(CACHE_DIR_ENV) {
            return Ok(dir.into());
        }
        if let Some(dir) = non_empty_env("XDG_CACHE_HOME") {
            return Ok(PathBuf::from(dir).join("catasto"));
        }
        if let Some(home) = non_empty_env("HOME") {
            return Ok(PathBuf::from(home).join(".cache").join("catasto"));
        }
        bail!("cannot locate a cache directory: set {CACHE_DIR_ENV}")
    }

    /// Reads `path` from the repository at `url`.
    ///
    /// With `pinned`, that commit is used and the network is touched only if
    /// the commit is not cached yet. Without it, the repository is fetched and
    /// `rev` (a tag, a branch, or a commit already reachable from one) is
    /// resolved to a commit.
    ///
    /// With [`SymlinkMode::Follow`], any symlinks are resolved inside the
    /// repository up to 8 hops with loop detection, returning the final regular
    /// file content as [`Content::File`].
    /// With [`SymlinkMode::Keep`], the entry must be a symlink, and its target
    /// text is returned as [`Content::Link`] without following.
    ///
    /// In both modes, intermediate directory components must not be symlinks,
    /// and symlink targets are validated not to be empty, contain NUL bytes,
    /// be absolute, or escape the repository root.
    pub fn fetch(
        &self,
        url: &str,
        rev: &str,
        path: &Path,
        pinned: Option<&str>,
        symlink: SymlinkMode,
    ) -> Result<Fetched> {
        let repo = self.repo_dir(url);
        cmd::init_bare(&repo)?;

        let commit = match pinned {
            Some(commit) => {
                cmd::check_commit_id(commit)?;
                if !cmd::has_commit(&repo, commit) {
                    cmd::fetch_remote(&repo, url)?;
                    if !cmd::has_commit(&repo, commit) {
                        bail!("locked commit {commit} is not in {url}");
                    }
                }
                commit.to_owned()
            }
            None => {
                cmd::fetch_remote(&repo, url)?;
                cmd::resolve(&repo, url, rev)?
            }
        };

        let content = tree::read_path(&repo, &commit, path, url, symlink)?;
        Ok(Fetched { commit, content })
    }

    fn repo_dir(&self, url: &str) -> PathBuf {
        let key = sha256_hex(url.as_bytes());
        self.dir.join("git").join(&key[..16])
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests;
