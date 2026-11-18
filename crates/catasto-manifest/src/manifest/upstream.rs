use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use tomet_ast::{Declaration as AstDeclaration, Value};
use tomet_declaration::{Declaration as SemDeclaration, Host, check_declarations};
use tomet_scalar::Scalar;
use tomet_type::Version;

use super::types::{check_git, require_rev, require_text};
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

/// Header state parsed from `::use(catasto, ...)` and `::upstream(...)`.
#[derive(Default)]
pub(super) struct Header {
    pub(super) has_use: bool,
    pub(super) upstreams: Upstreams,
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
    /// Catasto recognizes `::use(catasto, ...)` and `::upstream(...)`.
    pub(super) fn from_header(header: &[AstDeclaration]) -> Result<Self> {
        let sem_decls: Vec<SemDeclaration> = header
            .iter()
            .filter_map(SemDeclaration::from_ast)
            .collect();
        if let Some((_, problem)) = check_declarations(&sem_decls, Host::Ctmt).into_iter().next() {
            bail!("{problem}");
        }

        let mut has_use = false;
        let mut upstream_decls = Vec::new();

        for (ast_decl, sem_decl) in header.iter().zip(sem_decls.iter()) {
            if ast_decl.name.is_bare_named("use") {
                validate_use_decl(sem_decl)?;
                if has_use {
                    bail!("`::use(catasto)` is declared twice");
                }
                has_use = true;
            } else if ast_decl.name.is_bare_named("upstream") {
                upstream_decls.push(ast_decl);
            } else {
                bail!(
                    "unknown declaration `::{}`; catasto reads `::use` and `::upstream`",
                    ast_decl.name
                );
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
            if !args.is_empty() && !has_use {
                bail!(
                    "`catasto` constructors require `::use(catasto, version: \"1.0.0\")` at the top of the manifest"
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

        Ok(Self {
            has_use,
            upstreams: Upstreams { named },
        })
    }
}

fn validate_use_decl(sem_decl: &SemDeclaration) -> Result<()> {
    let Some(view) = sem_decl.args_view() else {
        bail!("`::use` requires a vocabulary name, e.g. `::use(catasto, version: \"1.0.0\")`");
    };
    let Some(target_val) = view.get("target") else {
        bail!("`::use` requires a vocabulary name, e.g. `::use(catasto, version: \"1.0.0\")`");
    };
    let Some(Scalar::Text(target)) = target_val.as_scalar() else {
        bail!("`::use` vocabulary name must be a string, e.g. `::use(catasto, version: \"1.0.0\")`");
    };
    if target != "catasto" {
        bail!("unknown vocabulary `{target}`; catasto only reads `::use(catasto, ...)`");
    }
    if let Some(version_val) = view.get("version") {
        let version_text = version_val
            .as_scalar()
            .and_then(Scalar::text)
            .ok_or_else(|| anyhow!("`::use(catasto)` version must be a version string"))?;
        let version = Version::parse(version_text.trim())
            .ok_or_else(|| anyhow!("invalid `::use(catasto)` version `{version_text}`"))?;
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
                    let names: Vec<_> = self.named.iter().map(|(known, _)| known.as_str()).collect();
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
