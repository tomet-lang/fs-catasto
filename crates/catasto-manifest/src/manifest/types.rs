use std::collections::HashSet;
use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::de::value::MapAccessDeserializer;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::MANIFEST_FILE;
use super::is_inside_workspace;
use super::reject_misnamed;
use super::upstream::Upstreams;

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
pub(super) struct Entry {
    pub(super) source: Source,
}

impl<'de> Deserialize<'de> for Manifest {
    /// Reads the entries in the order they were written. The cotomet parser
    /// already rejects a path written twice, so a manifest read from text has
    /// no duplicates; validation still checks one built in code.
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
                    let entry: Entry = map.next_value()?;
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

/// How symlinks in a git source are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Symlink {
    /// Resolve symlinks inside the repository and place the target file (default).
    #[default]
    Follow,
    /// Keep the symlink itself; the upstream entry must be a symlink.
    Keep,
}

impl Serialize for Symlink {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Follow => serializer.serialize_str("follow"),
            Self::Keep => serializer.serialize_str("keep"),
        }
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
/// source: { git: "https://example.com/vocabularies.git", rev: v1, path: "writ.vocabulary.tmt", symlink: keep }
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
        /// How symlinks in the git source are handled (`Follow` by default, or `Keep`).
        symlink: Symlink,
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
    /// How symlinks are handled (`follow` by default, or `keep`).
    #[serde(default, deserialize_with = "deserialize_symlink")]
    symlink: Symlink,
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

/// Reads a `rev`, which must be a string. cotomet reads `1.2` and `20260101` as
/// numbers, and a tag that is one has to be quoted.
pub(super) fn quoted_rev<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
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

pub(super) const NUMBER_REV: &str =
    "`rev` reads as a number; write it in quotes, e.g. `rev: \"1.2\"`";

/// Checks the part of a git source that an upstream also names.
pub(super) fn check_git(git: &str, rev: &str) -> Result<(), String> {
    crate::manifest::url::classify(git)?;
    if rev.is_empty() || rev.starts_with('-') {
        return Err(format!("invalid rev: {rev:?}"));
    }
    Ok(())
}

impl GitSource {
    fn into_source(self) -> Result<Source, String> {
        let GitSource {
            git,
            rev,
            path,
            symlink,
        } = self;
        check_git(&git, &rev)?;
        if !is_inside_workspace(&path) {
            return Err(format!(
                "git path must be relative and stay inside the repository: {}",
                path.display()
            ));
        }
        Ok(Source::Git {
            git,
            rev,
            path,
            symlink,
        })
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
            #[serde(skip_serializing_if = "is_follow")]
            symlink: Symlink,
        }

        fn is_follow(symlink: &Symlink) -> bool {
            matches!(symlink, Symlink::Follow)
        }

        match self {
            Source::Path(path) => path.serialize(serializer),
            Source::Git {
                git,
                rev,
                path,
                symlink,
            } => GitSourceRef {
                git,
                rev,
                path,
                symlink: *symlink,
            }
            .serialize(serializer),
        }
    }
}

impl Manifest {
    /// Loads `catasto.ctmt` from `root`. A missing file is an empty manifest.
    pub fn load(root: &Path) -> Result<Self> {
        let file = root.join(MANIFEST_FILE);
        match std::fs::read_to_string(&file) {
            Ok(text) => Self::parse(&text).with_context(|| format!("invalid {}", file.display())),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                reject_misnamed(root, MANIFEST_FILE)?;
                Ok(Self::default())
            }
            Err(e) => Err(e).with_context(|| format!("cannot read {}", file.display())),
        }
    }

    /// Parses and validates manifest text.
    ///
    /// The `::upstreams` declaration at the top is read first and its names
    /// are written into the entries that use them; the entries are then read
    /// as if they had been written in full. A file with nothing but comments
    /// is the empty manifest.
    pub fn parse(text: &str) -> Result<Self> {
        let file = cotomet::syntax::parse_file(text).map_err(|e| anyhow!("{e}"))?;
        let upstreams = Upstreams::from_header(&file.header)?;
        let body = upstreams.resolve(file.body)?;
        let manifest: Self = cotomet::from_value(body).map_err(|e| anyhow!("{e}"))?;
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
