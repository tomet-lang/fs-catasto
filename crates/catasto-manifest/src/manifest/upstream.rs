use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use tomet_ast::{Declaration as AstDeclaration, Name, Value};
use tomet_declaration::{Declaration as SemDeclaration, Host, check_declarations};
use tomet_scalar::Scalar;
use tomet_type::Version;

use super::types::{check_git, require_rev, require_text};
use super::zone::{Zone, parse_zone_decls};
use super::{is_inside_workspace, vocabulary};

/// What `::upstream` names: a git repository at a `rev`, and the directory
/// of it that the entries' `path`s are relative to.
pub(super) struct Upstream {
    pub(super) git: String,
    pub(super) rev: String,
    /// The directory inside the repository that `path` starts from. Without
    /// it, `path` starts at the root of the repository.
    pub(super) root: Option<PathBuf>,
}

/// Header state parsed from `::kind(catasto, ...)`, `::upstream(...)`, and `::zone(...)`.
#[derive(Default)]
pub(super) struct Header {
    /// Whether the manifest says it is a catasto document (`::kind(catasto, ...)`), which is what
    /// puts the vocabulary's attributes and constructors in scope.
    pub(super) has_kind: bool,
    pub(super) upstreams: Upstreams,
    pub(super) zones: Vec<Zone>,
}

/// The places an entry's `catasto.upstream(...)` can name.
///
/// Declared in `::upstream(...)` at the top of the manifest. Only the manifest
/// knows them: resolving `catasto.upstream(...)` expands each into `Source::Git`,
/// and the lockfile never sees the upstream name.
#[derive(Default)]
pub(super) struct Upstreams {
    /// In the order they were declared, which is the order an error lists them.
    pub(super) named: Vec<(String, Upstream)>,
}

impl Header {
    /// Reads and validates the declarations before the first entry.
    ///
    /// The manifest is a catasto document: `::kind(catasto, ...)` says so, and the other
    /// declarations are the attributes of the `catasto` vocabulary (its `::attribute` table),
    /// written bare (`::upstream`) or with the namespace (`::catasto.upstream`).
    pub(super) fn from_header(header: &[AstDeclaration]) -> Result<Self> {
        let sem_decls: Vec<SemDeclaration> =
            header.iter().filter_map(SemDeclaration::from_ast).collect();
        if let Some((_, problem)) = check_declarations(&sem_decls, Host::Ctmt)
            .into_iter()
            .next()
        {
            bail!("{problem}");
        }

        let mut has_kind = false;
        let mut upstream_decls = Vec::new();
        let mut zone_decls = Vec::new();

        for ast_decl in header {
            if ast_decl.name.is_bare_named("kind") {
                let sem_decl = SemDeclaration::from_ast(ast_decl)
                    .ok_or_else(|| anyhow!("malformed `::kind` declaration"))?;
                validate_kind_decl(&sem_decl)?;
                if has_kind {
                    bail!("`::kind(catasto)` is declared twice");
                }
                has_kind = true;
            } else if ast_decl.name.is_bare_named("use") {
                bail!(
                    "a manifest is a catasto document: write `::kind(catasto, version: \"1.0.0\")`, not `::use`"
                );
            } else {
                match catasto_attribute(&ast_decl.name) {
                    Some("upstream") => upstream_decls.push(ast_decl),
                    Some("zone") => zone_decls.push(ast_decl),
                    Some(other) => bail!(
                        "`::{other}` is in the catasto vocabulary, but this catasto does not read it"
                    ),
                    None => bail!(
                        "unknown declaration `::{}`; catasto reads `::kind` and {}",
                        ast_decl.name,
                        attribute_list()
                    ),
                }
            }
        }

        let mut named = Vec::new();
        let mut seen_names = HashSet::new();
        for decl in upstream_decls {
            if decl.data.value.is_some() || decl.content.is_some() {
                bail!(
                    "`::upstream` takes entries in parentheses, not `{{...}}`: `::upstream(name: catasto.git(\"…\", rev: main))`"
                );
            }
            let Some(args) = &decl.data.args else {
                bail!(
                    "`::upstream` needs named entries: `::upstream(name: catasto.git(\"…\", rev: main))`"
                );
            };
            if !args.has_no_unnamed() {
                bail!(
                    "`::upstream` entries must be named: `::upstream(name: catasto.git(\"…\", rev: main))`"
                );
            }
            if !args.is_empty() && !has_kind {
                bail!(
                    "`catasto` constructors require `::kind(catasto, version: \"1.0.0\")` at the top of the manifest"
                );
            }
            for (name, value) in args.named() {
                if !seen_names.insert(name.to_owned()) {
                    bail!("upstream `{name}` is declared twice");
                }
                let upstream = parse_upstream_entry(name, value)?;
                named.push((name.to_owned(), upstream));
            }
        }

        let zones = parse_zone_decls(&zone_decls)?;

        Ok(Self {
            has_kind,
            upstreams: Upstreams { named },
            zones,
        })
    }
}

/// The attribute of the catasto vocabulary that `name` resolves to: a bare name or one in the
/// `catasto` namespace, found in the vocabulary's `::attribute` table.
fn catasto_attribute(name: &Name) -> Option<&'static str> {
    if !matches!(name.namespace.as_deref(), None | Some("catasto")) {
        return None;
    }
    vocabulary()
        .attributes
        .get_key_value(name.name.as_str())
        .map(|(declared, _)| declared.as_str())
}

/// The attributes a manifest may write, for an error message: "`::upstream` and `::zone`".
fn attribute_list() -> String {
    let names: Vec<String> = vocabulary()
        .attributes
        .keys()
        .map(|name| format!("`::{name}`"))
        .collect();
    match names.split_last() {
        None => "nothing else".to_owned(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

fn validate_kind_decl(sem_decl: &SemDeclaration) -> Result<()> {
    let Some(view) = sem_decl.args_view() else {
        bail!("`::kind` requires a name, e.g. `::kind(catasto, version: \"1.0.0\")`");
    };
    let Some(kind_val) = view.get("kind") else {
        bail!("`::kind` requires a name, e.g. `::kind(catasto, version: \"1.0.0\")`");
    };
    let Some(Scalar::Text(kind)) = kind_val.as_scalar() else {
        bail!("`::kind` name must be a string, e.g. `::kind(catasto, version: \"1.0.0\")`");
    };
    if kind != "catasto" {
        bail!("a manifest is of kind `catasto`, not `{kind}`: write `::kind(catasto, ...)`");
    }
    if let Some(version_val) = view.get("version") {
        let version_text = version_val
            .as_scalar()
            .and_then(Scalar::text)
            .ok_or_else(|| anyhow!("`::kind(catasto)` version must be a version string"))?;
        let version = Version::parse(version_text.trim())
            .ok_or_else(|| anyhow!("invalid `::kind(catasto)` version `{version_text}`"))?;
        if version.major() != 1
            || version.minor() != 0
            || version.patch() != 0
            || version.is_prerelease()
        {
            bail!(
                "unsupported catasto vocabulary version `{version_text}`; this catasto supports `1.0.0`"
            );
        }
    }
    Ok(())
}

fn parse_upstream_entry(name: &str, value: &Value) -> Result<Upstream> {
    let context = format!("upstream `{name}`");
    let constructed = vocabulary()
        .read_call(value)
        .map_err(|e| anyhow!("{context}: {e}"))?;
    if constructed.name != "git" {
        bail!(
            "{context}: must use `catasto.git(...)`, not `catasto.{}`",
            constructed.name
        );
    }

    if let Value::Call(_, raw_args) = value {
        for forbidden in ["path", "symlink"] {
            if raw_args.contains_key(forbidden) {
                bail!("{context}: `catasto.git` inside `::upstream` cannot specify `{forbidden}`");
            }
        }
    }

    let git = require_text(&constructed, "url", &context)?;
    let rev = require_rev(&constructed, &context)?;
    check_git(&git, &rev).map_err(|e| anyhow!("{context}: {e}"))?;

    let root = match constructed.get("root") {
        None | Some(Value::Scalar(Scalar::None)) => None,
        Some(Value::Scalar(Scalar::Text(r))) => {
            let root_path = PathBuf::from(r);
            if !is_inside_workspace(&root_path) {
                bail!(
                    "{context}: `root` must be relative and stay inside the repository: {}",
                    root_path.display()
                );
            }
            Some(root_path)
        }
        Some(_) => bail!("{context}: `root` must be a string"),
    };

    Ok(Upstream { git, rev, root })
}

impl Upstreams {
    pub(super) fn get(&self, name: &str) -> Result<&Upstream> {
        self.named
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, u)| u)
            .ok_or_else(|| {
                let known = if self.named.is_empty() {
                    "none is declared; add `::upstream(...)` at the top".to_owned()
                } else {
                    let names: Vec<_> =
                        self.named.iter().map(|(known, _)| known.as_str()).collect();
                    format!("declared: {}", names.join(", "))
                };
                anyhow!("unknown upstream `{name}` ({known})")
            })
    }
}

/// One place named in `::upstream(...)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamInfo {
    pub name: String,
    pub git: String,
    pub rev: String,
    /// The directory of the repository its entries' paths start from.
    pub root: Option<PathBuf>,
}

/// The places `::upstream(...)` names in `text`, in the order they were written.
pub fn upstreams_of(text: &str) -> Result<Vec<UpstreamInfo>> {
    let file = cotomet::parse_file(text).map_err(|e| anyhow!("{e}"))?;
    let header = Header::from_header(&file.header)?;
    Ok(header
        .upstreams
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
