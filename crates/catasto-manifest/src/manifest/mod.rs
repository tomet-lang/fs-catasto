//! The hand-written `catasto.ctmt` declaration.
//!
//! A manifest declares `::use(catasto, version: "1.0.0")`, optional shared
//! Git repositories in `::upstream(...)`, and maps workspace-relative paths to
//! constructor calls from the `catasto` vocabulary:
//!
//! ```text
//! ::use(catasto, version: "1.0.0")
//! ::upstream(
//!   vocabularies: catasto.git("https://example.com/vocabularies.git", rev: main, root: vocabularies)
//! )
//!
//! "tmtroot/schema/vocabularies/writ.vocabulary.tmt": catasto.local("../vocabularies/writ.vocabulary.tmt")
//!
//! "tmtroot/schema/vocabularies/rust.vocabulary.tmt": catasto.upstream(
//!   vocabularies,
//!   path: "rust/rust.vocabulary.tmt"
//! )
//!
//! "scripts/deploy.sh": (
//!   catasto.git("https://example.com/tools.git", rev: v1, path: "deploy.sh"),
//!   executable: true
//! )
//! ```
//!
//! An upstream is resolved while the file is read, into the full `git`, `rev`
//! and `path` of [`Source::Git`] (joining `root` in front of `path`), so
//! everything after the parse, and the lockfile, see the same [`Source`]
//! either way.

use std::path::{Component, Path};
use std::sync::LazyLock;

use anyhow::{Result, bail};
use tomet_semantics::Vocabulary;

#[cfg(test)]
mod tests;
mod types;
mod upstream;
pub mod url;
mod zone;

pub use types::{Managed, ManagedKind, Manifest, Repo, Source, Symlink};
pub use upstream::{UpstreamInfo, upstreams_of};
pub use url::{UrlKind, classify};
pub use zone::{EntryRules, Zone, ZonePattern};

/// File name of the manifest, relative to the workspace root.
pub const MANIFEST_FILE: &str = "catasto.ctmt";

/// Canonical `catasto@1.0.0` vocabulary source.
pub const VOCABULARY_TMT: &str =
    include_str!("../../../../tmtroot/schema/vocabularies/catasto@1.0.0.vocabulary.tmt");

static VOCABULARY: LazyLock<Vocabulary> = LazyLock::new(|| {
    let doc = tomet_ast_parser::parse_file(VOCABULARY_TMT)
        .expect("embedded catasto@1.0.0.vocabulary.tmt must parse");
    Vocabulary::from_file(&doc).expect("embedded catasto@1.0.0.vocabulary.tmt must be a vocabulary")
});

/// Returns the parsed canonical `catasto@1.0.0` vocabulary.
pub fn vocabulary() -> &'static Vocabulary {
    &VOCABULARY
}

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
