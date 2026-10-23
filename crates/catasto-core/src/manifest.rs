//! The hand-written `catasto.mtm` declaration.
//!
//! A manifest is a motem map from a path inside the workspace to what
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
//! Quote a `rev` that reads as a number (`rev: "1.2"`), or motem reads it as
//! one.

use std::collections::HashSet;
use std::fmt;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::de::value::MapAccessDeserializer;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// File name of the manifest, relative to the workspace root.
pub const MANIFEST_FILE: &str = "catasto.mtm";

/// The set of files catasto manages in a workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    /// Declared files, in the order they were written.
    pub managed: Vec<Managed>,
}

/// One declared file: where it lives in the workspace and where it comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Managed {
    /// Location inside the workspace, relative to the root.
    pub path: PathBuf,
    /// Where the canonical content comes from.
    pub source: Source,
}

/// What a manifest entry holds, keyed by its path.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, expecting = "a map with a `source`")]
struct Entry {
    source: Source,
}

impl<'de> Deserialize<'de> for Manifest {
    /// Reads the entries in the order they were written. Duplicate paths are
    /// kept here and rejected by validation, which can name them.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;

        impl<'de> Visitor<'de> for Entries {
            type Value = Manifest;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a map from workspace paths to entries")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Manifest, A::Error> {
                let mut managed = Vec::new();
                while let Some(path) = map.next_key::<String>()? {
                    let entry: Entry = map
                        .next_value()
                        .map_err(|e| de::Error::custom(format!("{path}: {e}")))?;
                    managed.push(Managed {
                        path: path.into(),
                        source: entry.source,
                    });
                }
                Ok(Manifest { managed })
            }
        }

        deserializer.deserialize_map(Entries)
    }
}

/// Origin of a managed file.
///
/// Written as a string for a local file, or a map for a git repository, so
/// further kinds can be added without breaking the format:
///
/// ```text
/// source: "../vocabularies/writ.vocabulary.tmt"
/// source: { git: "https://example.com/vocabularies.git", rev: v1, path: "writ.vocabulary.tmt" }
/// ```
///
/// The lockfile writes the same shapes in JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// The map form of [`Source`]; unknown keys are rejected.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitSource {
    git: String,
    #[serde(deserialize_with = "quoted_rev")]
    rev: String,
    path: PathBuf,
}

/// Reads a `rev`, which must be a string. motem reads `1.2` and `20260101` as
/// numbers, and a tag that is one has to be quoted.
fn quoted_rev<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    struct Rev;

    impl Visitor<'_> for Rev {
        type Value = String;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a string")
        }

        fn visit_str<E: de::Error>(self, rev: &str) -> Result<String, E> {
            Ok(rev.to_owned())
        }

        fn visit_i64<E: de::Error>(self, _: i64) -> Result<String, E> {
            Err(E::custom(NUMBER_REV))
        }

        fn visit_u64<E: de::Error>(self, _: u64) -> Result<String, E> {
            Err(E::custom(NUMBER_REV))
        }

        fn visit_f64<E: de::Error>(self, _: f64) -> Result<String, E> {
            Err(E::custom(NUMBER_REV))
        }
    }

    deserializer.deserialize_any(Rev)
}

const NUMBER_REV: &str = "`rev` reads as a number; write it in quotes, e.g. `rev: \"1.2\"`";

impl GitSource {
    fn into_source(self) -> Result<Source, String> {
        let GitSource { git, rev, path } = self;
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
}

impl<'de> Deserialize<'de> for Source {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SourceVisitor;

        impl<'de> Visitor<'de> for SourceVisitor {
            type Value = Source;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a local path, or a map with `git`, `rev` and `path`")
            }

            fn visit_str<E: de::Error>(self, path: &str) -> Result<Source, E> {
                if path.is_empty() {
                    return Err(E::custom("a source path must not be empty"));
                }
                Ok(Source::Path(path.into()))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Source, A::Error> {
                GitSource::deserialize(MapAccessDeserializer::new(map))?
                    .into_source()
                    .map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_any(SourceVisitor)
    }
}

impl Serialize for Source {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct GitSourceRef<'a> {
            git: &'a str,
            rev: &'a str,
            path: &'a Path,
        }

        match self {
            Source::Path(path) => path.serialize(serializer),
            Source::Git { git, rev, path } => GitSourceRef { git, rev, path }.serialize(serializer),
        }
    }
}

impl Manifest {
    /// Loads `catasto.mtm` from `root`. A missing file is an empty manifest.
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
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let manifest: Self = motem::from_str(text).map_err(|e| anyhow!("{e}"))?;
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

    fn path_source(path: &str) -> Source {
        Source::Path(path.into())
    }

    #[test]
    fn parses_managed_entries_in_the_order_written() {
        let manifest = Manifest::parse(
            r#"
            "b/writ.vocabulary.tmt": { source: "../vocabularies/writ.vocabulary.tmt" }
            "a/rust.vocabulary.tmt": { source: "../vocabularies/rust.vocabulary.tmt" }
            "#,
        )
        .unwrap();

        assert_eq!(
            manifest.managed,
            vec![
                Managed {
                    path: "b/writ.vocabulary.tmt".into(),
                    source: path_source("../vocabularies/writ.vocabulary.tmt"),
                },
                Managed {
                    path: "a/rust.vocabulary.tmt".into(),
                    source: path_source("../vocabularies/rust.vocabulary.tmt"),
                },
            ]
        );
    }

    #[test]
    fn comments_and_unquoted_values_are_fine() {
        let manifest = Manifest::parse(
            "// shared vocabularies\n\
             \"a.tmt\": { source: ../v/a.tmt } // the writ one\n",
        )
        .unwrap();

        assert_eq!(manifest.managed[0].source, path_source("../v/a.tmt"));
    }

    #[test]
    fn a_key_that_needs_no_quotes_may_be_bare() {
        let manifest = Manifest::parse("a.tmt: { source: s }").unwrap();
        assert_eq!(manifest.managed[0].path, PathBuf::from("a.tmt"));
    }

    #[test]
    fn empty_text_is_an_empty_manifest() {
        assert_eq!(Manifest::parse("").unwrap(), Manifest::default());
        assert_eq!(Manifest::parse("\n  \n").unwrap(), Manifest::default());
    }

    #[test]
    fn an_entry_is_always_a_map() {
        let err = Manifest::parse("\"a\": \"s\"").unwrap_err();
        assert!(err.to_string().contains("a: invalid type"), "{err}");
        assert!(err.to_string().contains("a map with a `source`"), "{err}");
    }

    #[test]
    fn an_entry_needs_a_source() {
        let err = Manifest::parse("\"a.tmt\": {}").unwrap_err();
        assert_eq!(err.to_string(), "a.tmt: missing field `source`");
    }

    #[test]
    fn rejects_unknown_fields() {
        let err = Manifest::parse("\"a\": { source: \"b\", owner: x }");
        assert!(err.is_err());
    }

    #[test]
    fn rejects_unknown_source_kind() {
        let err = Manifest::parse("\"a\": { source: { ftp: \"b\" } }");
        assert!(err.is_err());
    }

    #[test]
    fn rejects_an_empty_source_path() {
        assert!(Manifest::parse("\"a\": { source: \"\" }").is_err());
    }

    #[test]
    fn parses_a_git_source() {
        let manifest = Manifest::parse(
            "\"a\": { source: { git: \"https://example.com/v.git\", rev: v1, path: \"dir/w.tmt\" } }",
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
    fn a_rev_that_reads_as_a_number_must_be_quoted() {
        let text = |rev: &str| {
            format!("\"a\": {{ source: {{ git: \"../v\", rev: {rev}, path: \"p\" }} }}")
        };

        let err = Manifest::parse(&text("1.2")).unwrap_err();
        assert!(err.to_string().contains("write it in quotes"), "{err}");
        assert!(Manifest::parse(&text("\"1.2\"")).is_ok());
    }

    #[test]
    fn rejects_ssh_style_git_urls() {
        for url in [
            "git@github.com:tomet-lang/twrit.git",
            "github-kakusika:tomet-lang/twrit.git",
            "ssh://git@github.com/tomet-lang/twrit.git",
            "http://example.com/v.git",
        ] {
            let text = format!("\"a\": {{ source: {{ git: \"{url}\", rev: v, path: \"p\" }} }}");
            assert!(Manifest::parse(&text).is_err(), "accepted {url}");
        }
    }

    #[test]
    fn accepts_https_and_relative_local_git_urls() {
        for url in ["https://github.com/tomet-lang/twrit.git", "../twrit"] {
            let text = format!("\"a\": {{ source: {{ git: \"{url}\", rev: v, path: \"p\" }} }}");
            assert!(Manifest::parse(&text).is_ok(), "rejected {url}");
        }
    }

    #[test]
    fn rejects_malformed_git_sources() {
        for source in [
            "{ git: \"u\", path: \"p\" }",
            "{ git: \"u\", rev: \"v\" }",
            "{ rev: \"v\", path: \"p\" }",
            "{ git: \"-u\", rev: \"v\", path: \"p\" }",
            "{ git: \"u\", rev: \"-v\", path: \"p\" }",
            "{ git: \"\", rev: \"v\", path: \"p\" }",
            "{ git: \"../u\", rev: \"v\", path: \"../p\" }",
            "{ git: \"../u\", rev: \"v\", path: \"/p\" }",
            "{ path: \"p\" }",
            "{}",
        ] {
            let text = format!("\"a\": {{ source: {source} }}");
            assert!(Manifest::parse(&text).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn rejects_paths_that_escape_the_workspace() {
        for bad in ["../a", "/etc/a", "a/../../b", ""] {
            let text = format!("\"{bad}\": {{ source: \"s\" }}");
            assert!(Manifest::parse(&text).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn rejects_duplicate_paths() {
        let text = "\"a\": { source: \"s\" }\n\"a\": { source: \"t\" }\n";
        let err = Manifest::parse(text).unwrap_err();
        assert!(err.to_string().contains("declared twice"), "{err}");
    }

    #[test]
    fn missing_file_is_an_empty_manifest() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Manifest::load(dir.path()).unwrap(), Manifest::default());
    }

    #[test]
    fn a_source_round_trips_through_json() {
        for source in [
            path_source("../v/a.tmt"),
            Source::Git {
                git: "https://example.com/v.git".into(),
                rev: "v1".into(),
                path: "dir/w.tmt".into(),
            },
        ] {
            let json = serde_json::to_string(&source).unwrap();
            assert_eq!(serde_json::from_str::<Source>(&json).unwrap(), source);
        }
    }
}
