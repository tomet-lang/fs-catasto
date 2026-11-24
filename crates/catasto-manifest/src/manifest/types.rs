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
use super::{MANIFEST_FILE, is_inside_workspace, is_relative_inside, reject_misnamed, vocabulary};

/// The set of files and directory zones catasto manages in a workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    /// Declared files, in the order they were written.
    pub managed: Vec<Managed>,
    /// Declared directory zone rules (`::zone(...)`), in the order they were written.
    pub zones: Vec<Zone>,
}

/// The `catasto.upstream(name, path: ...)` reference an entry was declared with, before expansion into [`Source::Git`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamRef {
    /// Name of the upstream declared in `::upstream(...)`.
    pub name: String,
    /// Optional path relative to the upstream's `path` (or repository root).
    pub path: Option<PathBuf>,
}

/// A Git repository cloned as a submodule worktree in the workspace (`(..., submodule: true)`).
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
        /// How symlinks in a git source are handled (`Follow` by default, or `Keep`).
        symlink: Symlink,
    },
    Submodule(Submodule),
}

/// One declared entry: where it lives in the workspace and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Managed {
    /// Location inside the workspace, relative to the root.
    pub path: PathBuf,
    /// Present when the entry was declared via `catasto.upstream(name, path: ...)`.
    pub upstream: Option<UpstreamRef>,
    /// File/directory source (with optional placement options) or Git submodule.
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

    /// Returns the upstream reference if this entry was declared via `catasto.upstream(...)`.
    pub fn upstream(&self) -> Option<&UpstreamRef> {
        self.upstream.as_ref()
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

    /// Returns the symlink handling mode if this entry is a file or directory.
    pub fn symlink(&self) -> Option<Symlink> {
        match &self.kind {
            ManagedKind::File { symlink, .. } => Some(*symlink),
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
        let GitSource { git, rev, path } = self;
        check_git(&git, &rev)?;
        if let Some(ref p) = path
            && !is_relative_inside(p)
        {
            return Err(format!(
                "git path must be relative and stay inside the repository: {}",
                p.display()
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
        }

        match self {
            Source::Path(path) => path.serialize(serializer),
            Source::Git { git, rev, path } => GitSourceRef {
                git,
                rev,
                path: path.as_deref(),
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

/// Parses a `catasto.git(url, rev:, path:)` constructor call identically in `::upstream` and in manifest entries.
pub(super) fn parse_git_call(
    constructed: &Constructed,
    context: &str,
) -> Result<(String, String, Option<PathBuf>)> {
    let git = require_text(constructed, "url", context)?;
    let rev = require_rev(constructed, context)?;
    check_git(&git, &rev).map_err(|e| anyhow!("{context}: {e}"))?;
    let path = match constructed.get("path") {
        None | Some(Value::Scalar(Scalar::None)) => None,
        Some(Value::Scalar(Scalar::Text(p))) => {
            let pb = PathBuf::from(p);
            if !is_relative_inside(&pb) {
                bail!(
                    "{context}: git path must be relative and stay inside the repository: {}",
                    pb.display()
                );
            }
            Some(pb)
        }
        Some(_) => bail!("{context}: `path` must be a string"),
    };
    Ok((git, rev, path))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct EntryOptions {
    executable: Option<bool>,
    symlink: Option<Symlink>,
    submodule: Option<bool>,
}

fn parse_symlink_value(val: &Value, context: &str) -> Result<Symlink> {
    match val {
        Value::Scalar(Scalar::Text(s)) => match s.as_str() {
            "follow" => Ok(Symlink::Follow),
            "keep" => Ok(Symlink::Keep),
            other => {
                bail!("{context}: invalid symlink option `{other}`; expected `follow` or `keep`")
            }
        },
        _ => bail!("{context}: invalid symlink option; expected `follow` or `keep`"),
    }
}

fn split_entry_value<'a>(path: &str, value: &'a Value) -> Result<(&'a Value, EntryOptions)> {
    match value {
        Value::Call(_, _) => Ok((value, EntryOptions::default())),
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
            let mut options = EntryOptions::default();
            for (key, val) in tuple.named() {
                match key {
                    "executable" => {
                        let Value::Scalar(Scalar::Bool(b)) = val else {
                            bail!("{path}: `executable` must be a boolean (`true` or `false`)");
                        };
                        options.executable = Some(*b);
                    }
                    "symlink" => {
                        options.symlink = Some(parse_symlink_value(val, path)?);
                    }
                    "submodule" => {
                        let Value::Scalar(Scalar::Bool(b)) = val else {
                            bail!("{path}: `submodule` must be a boolean (`true` or `false`)");
                        };
                        options.submodule = Some(*b);
                    }
                    _ => {
                        bail!(
                            "{path}: unknown entry field `{key}`; expected `executable`, `symlink`, or `submodule`"
                        );
                    }
                }
            }
            Ok((call, options))
        }
        _ => bail!(
            "{path}: entry must be a constructor call (e.g. `catasto.local(\"...\")`) or `(call, executable: ..., symlink: ..., submodule: ...)`"
        ),
    }
}

fn resolve_git_entry_kind(
    path: &str,
    git: String,
    rev: String,
    repo_path: Option<PathBuf>,
    options: EntryOptions,
) -> Result<ManagedKind> {
    if options.submodule == Some(true) {
        if options.executable.is_some() {
            bail!("{path}: submodule entry (`submodule: true`) cannot declare `executable`");
        }
        if options.symlink.is_some() {
            bail!("{path}: submodule entry (`submodule: true`) cannot specify `symlink`");
        }
        return Ok(ManagedKind::Submodule(Submodule {
            git,
            rev,
            path: repo_path,
        }));
    }
    Ok(ManagedKind::File {
        source: Source::Git {
            git,
            rev,
            path: repo_path,
        },
        executable: options.executable,
        symlink: options.symlink.unwrap_or(Symlink::Follow),
    })
}

fn parse_entry(
    path: &str,
    call_value: &Value,
    options: EntryOptions,
    header: &Header,
) -> Result<(ManagedKind, Option<UpstreamRef>)> {
    if !header.has_kind {
        bail!(
            "`catasto` constructors require `::kind(catasto, version: \"1.0.0\")` at the top of the manifest"
        );
    }
    let constructed = vocabulary()
        .read_call(call_value)
        .map_err(|e| anyhow!("{path}: {e}"))?;

    match constructed.name.as_str() {
        "local" => {
            if options.symlink.is_some() {
                bail!("{path}: `catasto.local` cannot specify `symlink`");
            }
            if options.submodule.is_some() {
                bail!("{path}: `catasto.local` cannot specify `submodule`");
            }
            let local_path = require_text(&constructed, "path", path)?;
            if local_path.is_empty() {
                bail!("{path}: a source path must not be empty");
            }
            Ok((
                ManagedKind::File {
                    source: Source::Path(local_path.into()),
                    executable: options.executable,
                    symlink: Symlink::Follow,
                },
                None,
            ))
        }
        "git" => {
            let (git, rev, entry_path) = parse_git_call(&constructed, path)?;
            let kind = resolve_git_entry_kind(path, git, rev, entry_path, options)?;
            Ok((kind, None))
        }
        "upstream" => {
            let name = require_text(&constructed, "upstream", path)?;
            let upstream = header
                .upstreams
                .get(&name)
                .map_err(|e| anyhow!("{path}: {e}"))?;
            let entry_path = match constructed.get("path") {
                None | Some(Value::Scalar(Scalar::None)) => None,
                Some(Value::Scalar(Scalar::Text(p))) => {
                    let pb = PathBuf::from(p);
                    if !is_relative_inside(&pb) {
                        bail!(
                            "{path}: upstream path must be relative and stay inside the repository: {}",
                            pb.display()
                        );
                    }
                    Some(pb)
                }
                Some(_) => bail!("{path}: `path` must be a string"),
            };
            let full_path = match (&upstream.path, &entry_path) {
                (Some(base), Some(rel)) => Some(base.join(rel)),
                (Some(base), None) => Some(base.clone()),
                (None, Some(rel)) => Some(rel.clone()),
                (None, None) => None,
            };
            let upstream_ref = UpstreamRef {
                name,
                path: entry_path,
            };
            let kind = resolve_git_entry_kind(
                path,
                upstream.git.clone(),
                upstream.rev.clone(),
                full_path,
                options,
            )?;
            Ok((kind, Some(upstream_ref)))
        }
        other => bail!("{path}: unknown constructor `catasto.{other}`"),
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
            let (call_value, options) = split_entry_value(path, entry.value())?;
            let (kind, upstream) = parse_entry(path, call_value, options, &header)?;
            managed.push(Managed {
                path: path.into(),
                upstream,
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
