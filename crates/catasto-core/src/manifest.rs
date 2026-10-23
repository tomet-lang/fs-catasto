//! The hand-written `catasto.toml` declaration.
//!
//! ```toml
//! [[managed]]
//! path = "tmtroot/schema/vocabularies/writ.vocabulary.tmt"
//! source = { path = "../vocabularies/writ.vocabulary.tmt" }
//! ```

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// File name of the manifest, relative to the workspace root.
pub const MANIFEST_FILE: &str = "catasto.toml";

/// The set of files catasto manages in a workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Declared files, in the order they were written.
    #[serde(default)]
    pub managed: Vec<Managed>,
}

/// One declared file: where it lives in the workspace and where it comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Managed {
    /// Location inside the workspace, relative to the root.
    pub path: PathBuf,
    /// Where the canonical content comes from.
    pub source: Source,
}

/// Origin of a managed file.
///
/// Written as a table so further kinds can be added without breaking the format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// A file on the local filesystem, relative to the workspace root.
    Path(PathBuf),
}

impl Source {
    /// Resolves the source to a filesystem path under `root`.
    pub fn resolve(&self, root: &Path) -> PathBuf {
        match self {
            Source::Path(path) => root.join(path),
        }
    }
}

impl Manifest {
    /// Loads `catasto.toml` from `root`. A missing file is an empty manifest.
    pub fn load(root: &Path) -> Result<Self> {
        let file = root.join(MANIFEST_FILE);
        match std::fs::read_to_string(&file) {
            Ok(text) => Self::parse(&text).with_context(|| format!("invalid {}", file.display())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", file.display())),
        }
    }

    /// Parses and validates manifest text.
    pub fn parse(text: &str) -> Result<Self> {
        let manifest: Self = toml::from_str(text)?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<()> {
        let mut seen = HashSet::new();
        for managed in &self.managed {
            if !is_inside_workspace(&managed.path) {
                bail!(
                    "managed path must be relative and stay inside the workspace: {}",
                    managed.path.display()
                );
            }
            if !seen.insert(&managed.path) {
                bail!("path is declared twice: {}", managed.path.display());
            }
        }
        Ok(())
    }
}

fn is_inside_workspace(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_managed_entries() {
        let manifest = Manifest::parse(
            r#"
            [[managed]]
            path = "a/writ.vocabulary.tmt"
            source = { path = "../vocabularies/writ.vocabulary.tmt" }
            "#,
        )
        .unwrap();

        assert_eq!(
            manifest.managed,
            vec![Managed {
                path: "a/writ.vocabulary.tmt".into(),
                source: Source::Path("../vocabularies/writ.vocabulary.tmt".into()),
            }]
        );
    }

    #[test]
    fn empty_text_is_an_empty_manifest() {
        assert_eq!(Manifest::parse("").unwrap(), Manifest::default());
    }

    #[test]
    fn rejects_unknown_fields() {
        let err = Manifest::parse(
            "[[managed]]\npath = \"a\"\nsource = { path = \"b\" }\nowner = \"x\"\n",
        );
        assert!(err.is_err());
    }

    #[test]
    fn rejects_unknown_source_kind() {
        let err = Manifest::parse("[[managed]]\npath = \"a\"\nsource = { ftp = \"b\" }\n");
        assert!(err.is_err());
    }

    #[test]
    fn rejects_paths_that_escape_the_workspace() {
        for bad in ["../a", "/etc/a", "a/../../b", ""] {
            let text = format!("[[managed]]\npath = \"{bad}\"\nsource = {{ path = \"s\" }}\n");
            assert!(Manifest::parse(&text).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn rejects_duplicate_paths() {
        let text = "[[managed]]\npath = \"a\"\nsource = { path = \"s\" }\n\
                    [[managed]]\npath = \"a\"\nsource = { path = \"t\" }\n";
        assert!(Manifest::parse(text).is_err());
    }

    #[test]
    fn missing_file_is_an_empty_manifest() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Manifest::load(dir.path()).unwrap(), Manifest::default());
    }
}
