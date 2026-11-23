use std::collections::HashSet;
use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::de::value::MapAccessDeserializer;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tomet_ast::Value;
use tomet_scalar::Scalar;
use tomet_semantics::Constructed;

use super::upstream::Header;
use super::zone::Zone;
use super::{MANIFEST_FILE, is_inside_workspace, reject_misnamed, vocabulary};

/// The set of files and directory zones catasto manages in a workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    /// Declared files, in the order they were written.
    pub managed: Vec<Managed>,
    /// Declared directory zone rules (`::zone(...)`), in the order they were written.
    pub zones: Vec<Zone>,
}

/// A Git repository cloned as a submodule worktree in the workspace (`catasto.git(..., submodule: true)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    /// An `https://` URL, or a local path relative to the workspace root.
    pub git: String,
    /// Branch, tag, or commit to check out and track.
    pub rev: String,
    /// Optional subdirectory or file inside the repository for sparse-checkout.
    pub path: Option<PathBuf>,
}

/// What a managed entry represents: either a file/directory or a Git submodule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedKind {
    File {
        /// Where the canonical content comes from.
        source: Source,
        /// The executable mode the entry declares, if it does: `Some(true)` places the file executable and
        /// `Some(false)` not, whatever the source says. `None` leaves it to the source (a git file's `100755`
        /// or not; a local file is never executable).
        executable: Option<bool>,
    },
    Submodule(Submodule),
}

/// One declared entry: where it lives in the workspace and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Managed {
    /// Location inside the workspace, relative to the root.
    pub path: PathBuf,
    /// File/directory source (with optional executable mode) or Git submodule.
    pub kind: ManagedKind,
}

impl Managed {
    /// Returns `true` if this entry manages a file or directory.
    pub fn is_file(&self) -> bool {
        matches!(self.kind, ManagedKind::File { .. })
    }

    /// Returns `true` if this entry manages a Git submodule.
    pub fn is_submodule(&self) -> bool {
        matches!(self.kind, ManagedKind::Submodule(_))
    }

    /// Returns the source if this entry is a file or directory.
    pub fn source(&self) -> Option<&Source> {
        match &self.kind {
            ManagedKind::File { source, .. } => Some(source),
            ManagedKind::Submodule(_) => None,
        }
    }

    /// Returns the submodule spec if this entry is a submodule.
    pub fn submodule(&self) -> Option<&Submodule> {
        match &self.kind {
            ManagedKind::Submodule(sub) => Some(sub),
            ManagedKind::File { .. } => None,
        }
    }

    /// Returns the declared executable mode if this entry is a file or directory.
    pub fn executable(&self) -> Option<bool> {
        match &self.kind {
            ManagedKind::File { executable, .. } => *executable,
            ManagedKind::Submodule(_) => None,
        }
    }

    /// Whether the file is placed executable, given whether its `source` is.
    pub fn is_executable(&self, source_is_executable: bool) -> bool {
        match &self.kind {
            ManagedKind::File { executable, .. } => executable.unwrap_or(source_is_executable),
            ManagedKind::Submodule(_) => false,
        }
    }
}

impl Manifest {
    /// Iterates over all declared submodule entries.
    pub fn submodules(&self) -> impl Iterator<Item = (&Path, &Submodule)> {
        self.managed.iter().filter_map(|m| match &m.kind {
            ManagedKind::Submodule(sub) => Some((m.path.as_path(), sub)),
            ManagedKind::File { .. } => None,
        })
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
/// Written in `catasto.ctmt` as a constructor call (`catasto.local(...)`,
/// `catasto.git(...)`, `catasto.upstream(...)`), and serialized in JSON for
/// `catasto.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A file on the local filesystem, relative to the workspace root.
    Path(PathBuf),
    /// A file or directory in a git repository, read at the commit `rev` resolves to.
    Git {
        /// An `https://` URL, or a local path relative to the workspace root.
        /// ssh is not supported.
        git: String,
        /// A tag, a branch, or a commit id.
        rev: String,
        /// Location of the file or directory inside the repository (`None` for repository root).
        path: Option<PathBuf>,
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

/// The map form of [`Source`] in `catasto.lock`; unknown keys are rejected.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitSource {
    git: String,
    #[serde(deserialize_with = "quoted_rev")]
    rev: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
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

/// Reads a `rev`, which must be a string. Numeric revisions must be quoted.
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

pub(super) const NUMBER_REV: &str =
    "`rev` reads as a number; write it in quotes, e.g. `rev: \"1.2\"`";

/// Checks the part of a git source that an upstream or submodule also names.
pub(crate) fn check_git(git: &str, rev: &str) -> Result<(), String> {
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
        if let Some(ref p) = path
            && !is_inside_workspace(p)
        {
            return Err(format!(
                "git path must be relative and stay inside the repository: {}",
                p.display()
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
                f.write_str("a local path, or a map with `git` and `rev`")
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
            #[serde(skip_serializing_if = "Option::is_none")]
            path: Option<&'a Path>,
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
                path: path.as_deref(),
                symlink: *symlink,
            }
            .serialize(serializer),
        }
    }
}

pub(super) fn require_text(constructed: &Constructed, arg: &str, context: &str) -> Result<String> {
    match constructed.get(arg) {
        Some(Value::Scalar(Scalar::Text(s))) => Ok(s.clone()),
        Some(_) => bail!("{context}: `{arg}` must be a string"),
        None => bail!("{context}: missing required argument `{arg}`"),
    }
}

pub(super) fn require_rev(constructed: &Constructed, context: &str) -> Result<String> {
    match constructed.get("rev") {
        Some(Value::Scalar(Scalar::Text(s))) => Ok(s.clone()),
        Some(Value::Scalar(Scalar::Int(_) | Scalar::Float(_) | Scalar::Number(_))) => {
            bail!("{context}: {NUMBER_REV}")
        }
        Some(_) => bail!("{context}: `rev` must be a string"),
        None => bail!("{context}: missing required argument `rev`"),
    }
}

fn parse_symlink(constructed: &Constructed, context: &str) -> Result<Symlink> {
    match constructed.get("symlink") {
        None => Ok(Symlink::Follow),
        Some(Value::Scalar(Scalar::Text(s))) => match s.as_str() {
            "follow" => Ok(Symlink::Follow),
            "keep" => Ok(Symlink::Keep),
            other => {
                bail!("{context}: invalid symlink option `{other}`; expected `follow` or `keep`")
            }
        },
        Some(_) => bail!("{context}: invalid symlink option; expected `follow` or `keep`"),
    }
}

fn split_entry_value<'a>(path: &str, value: &'a Value) -> Result<(&'a Value, Option<bool>)> {
    match value {
        Value::Call(_, _) => Ok((value, None)),
        Value::Tuple(tuple) => {
            let mut unnamed = tuple.unnamed();
            let Some(call) = unnamed.next() else {
                bail!(
                    "{path}: entry tuple must start with a constructor call, e.g. `(catasto.local(\"...\"), executable: true)`"
                );
            };
            if unnamed.next().is_some() {
                bail!("{path}: entry tuple takes only one constructor call");
            }
            if !matches!(call, Value::Call(_, _)) {
                bail!(
                    "{path}: first item of entry tuple must be a constructor call (e.g. `catasto.local(\"...\")`)"
                );
            }
            let mut executable = None;
            for (key, val) in tuple.named() {
                if key != "executable" {
                    bail!("{path}: unknown entry field `{key}`; only `executable` is allowed");
                }
                let Value::Scalar(Scalar::Bool(b)) = val else {
                    bail!("{path}: `executable` must be a boolean (`true` or `false`)");
                };
                executable = Some(*b);
            }
            Ok((call, executable))
        }
        _ => bail!(
            "{path}: entry must be a constructor call (e.g. `catasto.local(\"...\")`) or `(call, executable: bool)`"
        ),
    }
}

fn parse_entry_kind(
    path: &str,
    call_value: &Value,
    executable: Option<bool>,
    header: &Header,
) -> Result<ManagedKind> {
    if !header.has_kind {
        bail!(
            "`catasto` constructors require `::kind(catasto, version: \"1.0.0\")` at the top of the manifest"
        );
    }
    let constructed = vocabulary()
        .read_call(call_value)
        .map_err(|e| anyhow!("{path}: {e}"))?;

    let source = match constructed.name.as_str() {
        "local" => {
            let local_path = require_text(&constructed, "path", path)?;
            if local_path.is_empty() {
                bail!("{path}: a source path must not be empty");
            }
            Source::Path(local_path.into())
        }
        "git" => {
            if let Value::Call(_, raw_args) = call_value
                && raw_args.contains_key("root")
            {
                bail!(
                    "{path}: `catasto.git` in a manifest entry cannot specify `root`; declare an upstream in `::upstream(...)` or include it in `path`"
                );
            }
            let git = require_text(&constructed, "url", path)?;
            let rev = require_rev(&constructed, path)?;
            check_git(&git, &rev).map_err(|e| anyhow!("{path}: {e}"))?;
            let entry_path = match constructed.get("path") {
                None | Some(Value::Scalar(Scalar::None)) => None,
                Some(Value::Scalar(Scalar::Text(p))) => {
                    let pb = PathBuf::from(p);
                    if !is_inside_workspace(&pb) {
                        bail!(
                            "{path}: git path must be relative and stay inside the repository: {}",
                            pb.display()
                        );
                    }
                    Some(pb)
                }
                Some(_) => bail!("{path}: `path` must be a string"),
            };
            let is_submodule = match constructed.get("submodule") {
                None => false,
                Some(Value::Scalar(Scalar::Bool(b))) => *b,
                Some(_) => bail!("{path}: `submodule` must be a boolean (`true` or `false`)"),
            };
            if is_submodule {
                if executable.is_some() {
                    bail!("{path}: catasto.git(..., submodule: true) cannot declare executable");
                }
                if let Value::Call(_, raw_args) = call_value
                    && raw_args.contains_key("symlink")
                {
                    bail!("{path}: catasto.git(..., submodule: true) cannot specify `symlink`");
                }
                return Ok(ManagedKind::Submodule(Submodule {
                    git,
                    rev,
                    path: entry_path,
                }));
            }
            let symlink = parse_symlink(&constructed, path)?;
            if symlink == Symlink::Keep && entry_path.is_none() {
                bail!("{path}: `symlink: keep` requires `path`");
            }
            Source::Git {
                git,
                rev,
                path: entry_path,
                symlink,
            }
        }
        "upstream" => {
            let name = require_text(&constructed, "upstream", path)?;
            let upstream = header
                .upstreams
                .get(&name)
                .map_err(|e| anyhow!("{path}: {e}"))?;
            let entry_path_str = require_text(&constructed, "path", path)?;
            let entry_path = PathBuf::from(&entry_path_str);
            if !is_inside_workspace(&entry_path) {
                bail!(
                    "{path}: upstream path must be relative and stay inside the repository: {}",
                    entry_path.display()
                );
            }
            let full_path = match &upstream.root {
                Some(root) => root.join(&entry_path),
                None => entry_path,
            };
            let symlink = parse_symlink(&constructed, path)?;
            Source::Git {
                git: upstream.git.clone(),
                rev: upstream.rev.clone(),
                path: Some(full_path),
                symlink,
            }
        }
        other => bail!("{path}: unknown constructor `catasto.{other}`"),
    };

    Ok(ManagedKind::File { source, executable })
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
    /// A file with nothing but comments (or only `::kind(catasto, ...)`) is the
    /// empty manifest.
    pub fn parse(text: &str) -> Result<Self> {
        let file = cotomet::parse_file(text).map_err(|e| anyhow!("{e}"))?;
        let header = Header::from_header(&file.header)?;
        let Value::Tuple(entries) = &file.body else {
            bail!("manifest body must be a map from workspace paths to entries");
        };

        let mut managed = Vec::with_capacity(entries.len());
        for entry in entries.entries() {
            let Some(path) = entry.name() else {
                bail!("manifest entry must have a workspace path key");
            };
            let (call_value, executable) = split_entry_value(path, entry.value())?;
            let kind = parse_entry_kind(path, call_value, executable, &header)?;
            managed.push(Managed {
                path: path.into(),
                kind,
            });
        }

        let manifest = Self {
            managed,
            zones: header.zones,
        };
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
        for (submodule_path, _) in self.submodules() {
            for managed in &self.managed {
                if managed.path != submodule_path
                    && is_strictly_inside(&managed.path, submodule_path)
                {
                    bail!(
                        "entry '{}' cannot be inside declared submodule '{}'",
                        managed.path.display(),
                        submodule_path.display()
                    );
                }
            }
        }
        Ok(())
    }
}

fn normal_components(p: &Path) -> Vec<std::path::Component<'_>> {
    p.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

fn is_strictly_inside(child: &Path, dir: &Path) -> bool {
    let child_parts = normal_components(child);
    let dir_parts = normal_components(dir);
    child_parts.len() > dir_parts.len() && child_parts.starts_with(&dir_parts)
}
