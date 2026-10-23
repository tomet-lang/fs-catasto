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
/// Written as an inline table so further kinds can be added without breaking
/// the format:
///
/// ```toml
/// source = { path = "../vocabularies/writ.vocabulary.tmt" }
/// source = { git = "https://example.com/vocabularies.git", rev = "v1", path = "writ.vocabulary.tmt" }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawSource", into = "RawSource")]
pub enum Source {
    /// A file on the local filesystem, relative to the workspace root.
    Path(PathBuf),
    /// A file in a git repository, read at the commit `rev` resolves to.
    Git {
        /// An `https://` URL, or a local path relative to the workspace root.
        /// ssh is not supported.
        git: String,
        /// A tag, a branch, or a commit id.
        rev: String,
        /// Location of the file inside the repository.
        path: PathBuf,
    },
}

impl Source {
    /// The source file on the local filesystem, for sources that are local.
    pub fn local_path(&self, root: &Path) -> Option<PathBuf> {
        match self {
            Source::Path(path) => Some(root.join(path)),
            Source::Git { .. } => None,
        }
    }
}

/// The flat table form of [`Source`]; unknown keys are rejected.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
}

impl TryFrom<RawSource> for Source {
    type Error = String;

    fn try_from(raw: RawSource) -> Result<Self, String> {
        match raw {
            RawSource {
                git: None,
                rev: None,
                path: Some(path),
            } => Ok(Source::Path(path)),
            RawSource {
                git: Some(git),
                rev: Some(rev),
                path: Some(path),
            } => {
                crate::git::classify(&git)?;
                if rev.is_empty() || rev.starts_with('-') {
                    return Err(format!("invalid rev: {rev:?}"));
                }
                if !is_inside_workspace(&path) {
                    return Err(format!(
                        "git path must be relative and stay inside the repository: {}",
                        path.display()
                    ));
                }
                Ok(Source::Git { git, rev, path })
            }
            RawSource { git: Some(_), .. } => {
                Err("a git source needs `git`, `rev` and `path`".to_owned())
            }
            RawSource { rev: Some(_), .. } => Err("`rev` is only valid with `git`".to_owned()),
            RawSource { .. } => Err("a source needs `path`, or `git`, `rev` and `path`".to_owned()),
        }
    }
}

impl From<Source> for RawSource {
    fn from(source: Source) -> Self {
        match source {
            Source::Path(path) => RawSource {
                git: None,
                rev: None,
                path: Some(path),
            },
            Source::Git { git, rev, path } => RawSource {
                git: Some(git),
                rev: Some(rev),
                path: Some(path),
            },
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
    fn parses_a_git_source() {
        let manifest = Manifest::parse(
            "[[managed]]\n\
             path = \"a\"\n\
             source = { git = \"https://example.com/v.git\", rev = \"v1\", path = \"dir/w.tmt\" }\n",
        )
        .unwrap();

        assert_eq!(
            manifest.managed[0].source,
            Source::Git {
                git: "https://example.com/v.git".into(),
                rev: "v1".into(),
                path: "dir/w.tmt".into(),
            }
        );
    }

    #[test]
    fn rejects_ssh_style_git_urls() {
        for url in [
            "git@github.com:tomet-lang/twrit.git",
            "github-kakusika:tomet-lang/twrit.git",
            "ssh://git@github.com/tomet-lang/twrit.git",
            "http://example.com/v.git",
        ] {
            let text = format!(
                "[[managed]]\npath = \"a\"\nsource = {{ git = \"{url}\", rev = \"v\", path = \"p\" }}\n"
            );
            assert!(Manifest::parse(&text).is_err(), "accepted {url}");
        }
    }

    #[test]
    fn accepts_https_and_relative_local_git_urls() {
        for url in ["https://github.com/tomet-lang/twrit.git", "../twrit"] {
            let text = format!(
                "[[managed]]\npath = \"a\"\nsource = {{ git = \"{url}\", rev = \"v\", path = \"p\" }}\n"
            );
            assert!(Manifest::parse(&text).is_ok(), "rejected {url}");
        }
    }

    #[test]
    fn rejects_malformed_git_sources() {
        for source in [
            "{ git = \"u\", path = \"p\" }",
            "{ git = \"u\", rev = \"v\" }",
            "{ rev = \"v\", path = \"p\" }",
            "{ git = \"-u\", rev = \"v\", path = \"p\" }",
            "{ git = \"u\", rev = \"-v\", path = \"p\" }",
            "{ git = \"\", rev = \"v\", path = \"p\" }",
            "{ git = \"u\", rev = \"v\", path = \"../p\" }",
            "{ git = \"u\", rev = \"v\", path = \"/p\" }",
            "{}",
        ] {
            let text = format!("[[managed]]\npath = \"a\"\nsource = {source}\n");
            assert!(Manifest::parse(&text).is_err(), "accepted {source}");
        }
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
