use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;

use super::is_inside_workspace;
use super::types::{check_git, quoted_rev};

/// What `::upstreams` names: a git repository at a `rev`, and the directory
/// of it that the entries' `path`s are relative to.
#[derive(Deserialize)]
#[serde(
    deny_unknown_fields,
    expecting = "a map with `git`, `rev` and optionally `root`"
)]
pub(super) struct Upstream {
    pub(super) git: String,
    #[serde(deserialize_with = "quoted_rev")]
    pub(super) rev: String,
    /// The directory inside the repository that `path` starts from. Without
    /// it, `path` starts at the root of the repository.
    pub(super) root: Option<PathBuf>,
}

/// The places an entry's `source` can name with `upstream:`.
///
/// Declared once, as a map from a name to where it is, at the top of the
/// manifest. Only the manifest knows them: [`Upstreams::resolve`] writes each
/// into the entries that use it, and the lockfile never sees the name.
#[derive(Default)]
pub(super) struct Upstreams {
    /// In the order they were declared, which is the order an error lists them.
    pub(super) named: Vec<(String, Upstream)>,
}

impl Upstreams {
    /// Reads the declarations before the first entry. `::upstreams` is the
    /// only one catasto knows, and it may appear once.
    pub(super) fn from_header(header: &[cotomet::syntax::Declaration]) -> Result<Self> {
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
            if declaration.args.is_some() {
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
    pub(super) fn resolve(&self, body: cotomet::Value) -> Result<cotomet::Value> {
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
