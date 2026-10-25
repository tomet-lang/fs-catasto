//! The hand-written `catasto.ctmt` declaration.
//!
//! A manifest is a cotomet map from a path inside the workspace to what
//! catasto knows about that path. Today an entry says where the file comes
//! from:
//!
//! ```text
//! "tmtroot/schema/vocabularies/writ.vocabulary.tmt": { source: "../vocabularies/writ.vocabulary.tmt" }
//!
//! "tmtroot/schema/vocabularies/rust.vocabulary.tmt": {
//!   source: { git: "https://example.com/vocabularies.git", rev: v1, path: "rust.vocabulary.tmt" }
//! }
//! ```
//!
//! The key comes first because every declaration catasto will take is about a
//! path, and the lockfile is keyed by path too. An entry is always a map; a
//! `source` is a string for a local file, or a map for a git repository.
//! Quote a `rev` that reads as a number (`rev: "1.2"`), or cotomet reads it as
//! one.
//!
//! A place that several entries come from is named once, in an `::upstreams`
//! declaration at the top of the file, and each entry refers to it:
//!
//! ```text
//! ::upstreams{
//!   vocabularies: { git: "https://example.com/vocabularies.git", rev: v1 }
//! }
//!
//! "tmtroot/schema/vocabularies/rust.vocabulary.tmt": {
//!   source: { upstream: vocabularies, path: "rust.vocabulary.tmt" }
//! }
//! ```
//!
//! An upstream may name a `root`, the directory of the repository that its
//! entries' `path`s start from: with `root: vocabularies` in the declaration,
//! `path: "rust/rust.vocabulary.tmt"` is the file `vocabularies/rust/rust.vocabulary.tmt`.
//!
//! An upstream is not a macro: it is resolved while the file is read, into
//! the `git`, `rev` and `path` map written out in full (the `root` joined in
//! front of the `path`), so everything after the parse, and the lockfile, see
//! the same [`Source`] either way.

use std::path::{Component, Path};

use anyhow::{Result, bail};

#[cfg(test)]
mod tests;
mod types;
mod upstream;
pub mod url;

pub use types::{Managed, Manifest, Source, Symlink};
pub use upstream::{UpstreamInfo, upstreams_of};
pub use url::{UrlKind, classify};

/// File name of the manifest, relative to the workspace root.
pub const MANIFEST_FILE: &str = "catasto.ctmt";

/// Fails when `expected` is missing from `root` but a file whose name differs from it only in case is there.
///
/// The names catasto reads are exact. On a case-sensitive filesystem `Catasto.ctmt` would otherwise be read as
/// "no manifest" without a word, while on a case-insensitive one it would work, so one tree would behave
/// differently from one machine to the next.
pub fn reject_misnamed(root: &Path, expected: &str) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name != expected && name.eq_ignore_ascii_case(expected) {
            bail!("found {name}, but catasto reads only {expected}: rename it");
        }
    }
    Ok(())
}

pub fn is_inside_workspace(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}
