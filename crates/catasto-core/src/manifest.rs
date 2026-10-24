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

use std::collections::HashSet;
use std::fmt;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::de::value::MapAccessDeserializer;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// File name of the manifest, relative to the workspace root.
pub const MANIFEST_FILE: &str = "catasto.ctmt";

/// The names the manifest had before it was `catasto.ctmt`. Nothing reads them; a workspace that still
/// has one is told to rename it.
const OLD_NAMES: [&str; 2] = ["catasto.motm", "catasto.mtm"];

/// Fails when the manifest is missing but one under an old name is there.
pub(crate) fn reject_old_names(root: &Path) -> Result<()> {
    for old in OLD_NAMES {
        if root.join(old).exists() {
            bail!(
                "found {old}, but the manifest is {MANIFEST_FILE} now: rename it \
                 (catasto reads nothing under the old name)"
            );
        }
    }
    Ok(())
}

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

/// Reads a `rev`, which must be a string. cotomet reads `1.2` and `20260101` as
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

/// Checks the part of a git source that an upstream also names.
fn check_git(git: &str, rev: &str) -> Result<(), String> {
    catasto_git::classify(git)?;
    if rev.is_empty() || rev.starts_with('-') {
        return Err(format!("invalid rev: {rev:?}"));
    }
    Ok(())
}

impl GitSource {
    fn into_source(self) -> Result<Source, String> {
        let GitSource { git, rev, path } = self;
        check_git(&git, &rev)?;
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
    /// Loads `catasto.ctmt` from `root`. A missing file is an empty manifest.
    pub fn load(root: &Path) -> Result<Self> {
        let file = root.join(MANIFEST_FILE);
        match std::fs::read_to_string(&file) {
            Ok(text) => Self::parse(&text).with_context(|| format!("invalid {}", file.display())),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                reject_old_names(root)?;
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

/// What `::upstreams` names: a git repository at a `rev`, and the directory
/// of it that the entries' `path`s are relative to.
#[derive(Deserialize)]
#[serde(
    deny_unknown_fields,
    expecting = "a map with `git`, `rev` and optionally `root`"
)]
struct Upstream {
    git: String,
    #[serde(deserialize_with = "quoted_rev")]
    rev: String,
    /// The directory inside the repository that `path` starts from. Without
    /// it, `path` starts at the root of the repository.
    root: Option<PathBuf>,
}

/// The places an entry's `source` can name with `upstream:`.
///
/// Declared once, as a map from a name to where it is, at the top of the
/// manifest. Only the manifest knows them: [`Upstreams::resolve`] writes each
/// into the entries that use it, and the lockfile never sees the name.
#[derive(Default)]
struct Upstreams {
    /// In the order they were declared, which is the order an error lists them.
    named: Vec<(String, Upstream)>,
}

impl Upstreams {
    /// Reads the declarations before the first entry. `::upstreams` is the
    /// only one catasto knows, and it may appear once.
    fn from_header(header: &[cotomet::syntax::Declaration]) -> Result<Self> {
        let mut upstreams = None;
        for declaration in header {
            if declaration.name != "upstreams" {
                bail!(
                    "unknown declaration `::{}`; the only one catasto reads is `::upstreams`",
                    declaration.name
                );
            }
            if upstreams.is_some() {
                bail!("`::upstreams` is declared twice; put every upstream in the one");
            }
            if !declaration.args.is_empty() {
                bail!(
                    "`::upstreams` takes a map, not arguments: `::upstreams{{ name: {{ git: \"…\", rev: main }} }}`"
                );
            }
            let Some(cotomet::Value::Map(entries)) = &declaration.data else {
                bail!(
                    "`::upstreams` needs a map of names: `::upstreams{{ name: {{ git: \"…\", rev: main }} }}`"
                );
            };
            let mut named = Vec::with_capacity(entries.len());
            for (name, value) in entries {
                let upstream: Upstream = cotomet::from_value(value.clone())
                    .map_err(|e| anyhow!("upstream `{name}`: {e}"))?;
                check_git(&upstream.git, &upstream.rev)
                    .map_err(|e| anyhow!("upstream `{name}`: {e}"))?;
                if let Some(root) = &upstream.root
                    && !is_inside_workspace(root)
                {
                    bail!(
                        "upstream `{name}`: `root` must be relative and stay inside the repository: {}",
                        root.display()
                    );
                }
                named.push((name.clone(), upstream));
            }
            upstreams = Some(Self { named });
        }
        Ok(upstreams.unwrap_or_default())
    }

    /// Writes the named upstreams into the entries of `body`: a `source` of
    /// `{ upstream: n, path: p }` becomes `{ git: …, rev: …, path: p }`.
    /// Anything else in the body is left as it is, for the entry types to read
    /// and reject.
    fn resolve(&self, body: cotomet::Value) -> Result<cotomet::Value> {
        let cotomet::Value::Map(entries) = body else {
            return Ok(body);
        };
        let mut resolved = Vec::with_capacity(entries.len());
        for (path, entry) in entries {
            let entry = match entry {
                cotomet::Value::Map(fields) => {
                    let mut out = Vec::with_capacity(fields.len());
                    for (key, value) in fields {
                        let value = match (key.as_str(), value) {
                            ("source", cotomet::Value::Map(source)) => {
                                cotomet::Value::Map(self.resolve_source(&path, source)?)
                            }
                            (_, value) => value,
                        };
                        out.push((key, value));
                    }
                    cotomet::Value::Map(out)
                }
                other => other,
            };
            resolved.push((path, entry));
        }
        Ok(cotomet::Value::Map(resolved))
    }

    fn resolve_source(
        &self,
        path: &str,
        source: Vec<(String, cotomet::Value)>,
    ) -> Result<Vec<(String, cotomet::Value)>> {
        let Some(at) = source.iter().position(|(key, _)| key == "upstream") else {
            return Ok(source);
        };
        if let Some((key, _)) = source.iter().find(|(key, _)| key == "git" || key == "rev") {
            bail!(
                "{path}: `upstream` already says where the file is; remove `{key}` (or `upstream`, and write `git` and `rev`)"
            );
        }
        let cotomet::Value::String(name) = &source[at].1 else {
            bail!(
                "{path}: `upstream` is the name of a declared upstream, e.g. `upstream: vocabularies`"
            );
        };
        let Some((_, upstream)) = self.named.iter().find(|(known, _)| known == name) else {
            let known = if self.named.is_empty() {
                "none is declared; add `::upstreams{ … }` at the top".to_owned()
            } else {
                let names: Vec<_> = self.named.iter().map(|(known, _)| known.as_str()).collect();
                format!("declared: {}", names.join(", "))
            };
            bail!("{path}: unknown upstream `{name}` ({known})");
        };
        let mut out = Vec::with_capacity(source.len() + 1);
        out.push((
            "git".to_owned(),
            cotomet::Value::String(upstream.git.clone()),
        ));
        out.push((
            "rev".to_owned(),
            cotomet::Value::String(upstream.rev.clone()),
        ));
        for (key, value) in source {
            match (key.as_str(), value) {
                ("upstream", _) => {}
                // `path` is read from `root`, and the lock records it from the
                // top of the repository, as when it is written out in full.
                ("path", cotomet::Value::String(path)) => {
                    let full = match &upstream.root {
                        Some(root) => root.join(path).to_string_lossy().into_owned(),
                        None => path,
                    };
                    out.push((key, cotomet::Value::String(full)));
                }
                (_, value) => out.push((key, value)),
            }
        }
        Ok(out)
    }
}

/// One place named in `::upstreams`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamInfo {
    pub name: String,
    pub git: String,
    pub rev: String,
    /// The directory of the repository its entries' paths start from.
    pub root: Option<PathBuf>,
}

/// The places `::upstreams` names in `text`, in the order they were written.
pub fn upstreams_of(text: &str) -> Result<Vec<UpstreamInfo>> {
    let file = cotomet::syntax::parse_file(text).map_err(|e| anyhow!("{e}"))?;
    let upstreams = Upstreams::from_header(&file.header)?;
    Ok(upstreams
        .named
        .into_iter()
        .map(|(name, upstream)| UpstreamInfo {
            name,
            git: upstream.git,
            rev: upstream.rev,
            root: upstream.root,
        })
        .collect())
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
    fn a_manifest_with_only_comments_is_empty() {
        assert_eq!(
            Manifest::parse("// nothing managed yet\n").unwrap(),
            Manifest::default()
        );
    }

    const VOCABULARIES: &str =
        "::upstreams{\n  vocabularies: { git: \"https://example.com/v.git\", rev: main }\n}\n";

    #[test]
    fn an_upstream_reads_as_the_entry_written_in_full() {
        let named = Manifest::parse(&format!(
            "{VOCABULARIES}\n\"a.tmt\": {{ source: {{ upstream: vocabularies, path: \"x/a.tmt\" }} }}\n"
        ))
        .unwrap();
        let written = Manifest::parse(
            "\"a.tmt\": { source: { git: \"https://example.com/v.git\", rev: main, path: \"x/a.tmt\" } }",
        )
        .unwrap();
        assert_eq!(named, written);
    }

    #[test]
    fn a_root_is_joined_in_front_of_the_path() {
        let rooted = Manifest::parse(
            "::upstreams{ v: { git: \"https://example.com/v.git\", rev: main, root: vocabularies } }\n\
             \"a.tmt\": { source: { upstream: v, path: \"rust/a.tmt\" } }\n",
        )
        .unwrap();
        let written = Manifest::parse(
            "\"a.tmt\": { source: { git: \"https://example.com/v.git\", rev: main, path: \"vocabularies/rust/a.tmt\" } }",
        )
        .unwrap();
        assert_eq!(rooted, written);
    }

    #[test]
    fn a_root_is_checked_where_it_is_declared() {
        for root in ["/etc", "../outside", "a/../b", "\"\""] {
            let err = Manifest::parse(&format!(
                "::upstreams{{ v: {{ git: \"https://example.com/v.git\", rev: main, root: {root} }} }}"
            ))
            .unwrap_err();
            assert!(
                err.to_string()
                    .contains("`root` must be relative and stay inside"),
                "{root}: {err}"
            );
        }
    }

    #[test]
    fn a_path_cannot_climb_out_of_the_root() {
        let err = Manifest::parse(
            "::upstreams{ v: { git: \"https://example.com/v.git\", rev: main, root: vocabularies } }\n\
             \"a.tmt\": { source: { upstream: v, path: \"../secret.tmt\" } }\n",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("stay inside the repository"),
            "{err}"
        );
    }

    #[test]
    fn two_entries_share_an_upstream_and_keep_their_order() {
        let manifest = Manifest::parse(&format!(
            "{VOCABULARIES}\n\
             \"b.tmt\": {{ source: {{ upstream: vocabularies, path: \"b.tmt\" }} }}\n\
             \"a.tmt\": {{ source: {{ upstream: vocabularies, path: \"a.tmt\" }} }}\n"
        ))
        .unwrap();
        let paths: Vec<_> = manifest.managed.iter().map(|m| m.path.clone()).collect();
        assert_eq!(paths, [PathBuf::from("b.tmt"), PathBuf::from("a.tmt")]);
        for managed in &manifest.managed {
            assert!(matches!(
                &managed.source,
                Source::Git { git, rev, .. } if git == "https://example.com/v.git" && rev == "main"
            ));
        }
    }

    #[test]
    fn a_local_source_and_a_written_out_git_source_need_no_upstream() {
        let manifest = Manifest::parse(&format!(
            "{VOCABULARIES}\n\
             \"a.tmt\": {{ source: \"../a.tmt\" }}\n\
             \"b.tmt\": {{ source: {{ git: \"https://example.com/o.git\", rev: v1, path: \"b.tmt\" }} }}\n"
        ))
        .unwrap();
        assert_eq!(manifest.managed[0].source, path_source("../a.tmt"));
        assert!(matches!(manifest.managed[1].source, Source::Git { .. }));
    }

    #[test]
    fn an_unknown_upstream_lists_the_declared_ones() {
        let err = Manifest::parse(&format!(
            "{VOCABULARIES}\n\"a.tmt\": {{ source: {{ upstream: vocabularis, path: \"a.tmt\" }} }}\n"
        ))
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "a.tmt: unknown upstream `vocabularis` (declared: vocabularies)"
        );
    }

    #[test]
    fn an_upstream_with_none_declared_says_where_to_declare() {
        let err =
            Manifest::parse("\"a.tmt\": { source: { upstream: v, path: \"a.tmt\" } }").unwrap_err();
        assert!(err.to_string().contains("add `::upstreams{ … }`"), "{err}");
    }

    #[test]
    fn an_upstream_does_not_mix_with_git_or_rev() {
        for extra in ["git: \"https://example.com/o.git\"", "rev: v2"] {
            let err = Manifest::parse(&format!(
                "{VOCABULARIES}\n\"a.tmt\": {{ source: {{ upstream: vocabularies, {extra}, path: \"a.tmt\" }} }}\n"
            ))
            .unwrap_err();
            assert!(err.to_string().contains("`upstream` already says"), "{err}");
        }
    }

    #[test]
    fn an_upstream_needs_a_path_like_a_git_source_does() {
        let err = Manifest::parse(&format!(
            "{VOCABULARIES}\n\"a.tmt\": {{ source: {{ upstream: vocabularies }} }}\n"
        ))
        .unwrap_err();
        assert!(err.to_string().contains("missing field `path`"), "{err}");
    }

    #[test]
    fn an_upstream_is_checked_where_it_is_declared() {
        let err =
            Manifest::parse("::upstreams{ v: { git: \"git@example.com:o/v.git\", rev: main } }")
                .unwrap_err();
        assert!(err.to_string().starts_with("upstream `v`: "), "{err}");

        let err =
            Manifest::parse("::upstreams{ v: { git: \"https://example.com/v.git\", rev: 1.2 } }")
                .unwrap_err();
        assert!(err.to_string().contains("write it in quotes"), "{err}");

        let err = Manifest::parse("::upstreams{ v: { git: \"https://example.com/v.git\" } }")
            .unwrap_err();
        assert!(err.to_string().contains("missing field `rev`"), "{err}");
    }

    #[test]
    fn the_upstreams_declaration_is_one_map_and_the_only_declaration() {
        let err = Manifest::parse(&format!("{VOCABULARIES}{VOCABULARIES}")).unwrap_err();
        assert!(err.to_string().contains("declared twice"), "{err}");

        let err = Manifest::parse("::upstreams(v)").unwrap_err();
        assert!(err.to_string().contains("takes a map"), "{err}");

        let err = Manifest::parse("::upstreams").unwrap_err();
        assert!(err.to_string().contains("needs a map"), "{err}");

        let err = Manifest::parse("::kind(catalog)").unwrap_err();
        assert!(
            err.to_string().contains("unknown declaration `::kind`"),
            "{err}"
        );
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
        assert_eq!(err.to_string(), "\"a.tmt\": missing field `source`");
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
        assert!(err.to_string().contains("duplicate key `a`"), "{err}");
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
