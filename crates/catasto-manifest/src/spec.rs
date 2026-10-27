//! Manifest schema specifications for `catasto.ctmt`.
//!
//! Provides [`ConfigGroupSpec`] representations of `::upstreams` and manifest path entries
//! using `#[derive(Config)]` from `tomet-spec`.

use std::path::PathBuf;

use tomet_spec::Config;
use tomet_spec::schema::{ConfigGroupSpec, ConfigSpecProvider};

/// Upstream repository definitions declared in `::upstreams`.
///
/// An upstream declares a named Git repository at a pinned revision, optionally
/// with a `root` directory inside the repository from which entry paths start.
#[derive(Debug, Clone, Default, PartialEq, Config)]
#[config(group = "upstreams")]
pub struct UpstreamSpec {
    /// Repository URL (`https://` URL or workspace-relative path).
    #[config(key = "git", type = "string")]
    pub git: String,

    /// Pinned tag, branch, or commit hash. Quote numeric revisions (e.g. `rev: "1.2"`).
    #[config(key = "rev", default = "main", type = "string")]
    pub rev: String,

    /// Directory inside the upstream repository that entry paths are relative to.
    #[config(key = "root", type = "path")]
    pub root: Option<PathBuf>,
}

/// Manifest file entry declaring a managed workspace file.
///
/// Each top-level map entry in `catasto.ctmt` binds a workspace-relative file path
/// to its canonical content origin (`source`).
#[derive(Debug, Clone, Default, PartialEq, Config)]
#[config(group = "entry")]
pub struct ManifestEntrySpec {
    /// Where canonical content comes from: a local path string, or a map with `git`, `rev`, `path`, or `upstream`.
    #[config(key = "source", type = "string | map")]
    pub source: String,

    /// Git repository URL (`https://` URL or workspace-relative local path).
    #[config(key = "source.git", type = "string")]
    pub source_git: Option<String>,

    /// Tag, branch, or commit hash. Quote numeric revisions (e.g. `rev: "1.2"`).
    #[config(key = "source.rev", default = "main", type = "string")]
    pub source_rev: Option<String>,

    /// Location of the file inside the upstream Git repository.
    #[config(key = "source.path", type = "path")]
    pub source_path: Option<PathBuf>,

    /// Name of an upstream repository declared in `::upstreams`.
    #[config(key = "source.upstream", type = "string")]
    pub source_upstream: Option<String>,

    /// Symlink preservation mode in git sources (`follow` to resolve target content, or `keep` to preserve symlink itself).
    #[config(
        key = "source.symlink",
        default = "follow",
        type = "\"follow\" | \"keep\""
    )]
    pub source_symlink: Option<String>,
}

/// Returns all configuration and declaration specification groups for `catasto.ctmt`.
pub fn all_manifest_specs() -> Vec<ConfigGroupSpec> {
    vec![UpstreamSpec::spec(), ManifestEntrySpec::spec()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tomet_spec::PrinterConfig;

    #[test]
    fn manifest_specs_render_valid_tmt() {
        let specs = all_manifest_specs();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "upstreams");
        assert_eq!(specs[1].name, "entry");

        let rendered = tomet_spec::config::render(
            &specs,
            "doc-testManifestSpec",
            "Manifest Reference",
            &["Intro paragraph."],
            &PrinterConfig::default(),
        );

        assert!(rendered.contains("==[ upstreams ]") || rendered.contains("==[upstreams]"));
        assert!(rendered.contains("==[ entry ]") || rendered.contains("==[entry]"));
        assert!(rendered.contains("`git`"));
        assert!(rendered.contains("`source`"));
        assert!(rendered.contains("`source.symlink`"));
    }
}
