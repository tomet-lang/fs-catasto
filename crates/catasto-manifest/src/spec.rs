//! Manifest schema specifications for `catasto.ctmt`.
//!
//! Provides [`ConfigGroupSpec`] representations of `::upstream` and manifest path entries
//! using `#[derive(Config)]` from `tomet-spec`.

use std::path::PathBuf;

use tomet_spec::Config;
use tomet_spec::schema::{ConfigGroupSpec, ConfigSpecProvider};

/// Upstream repository definitions declared in `::upstream(...)`.
///
/// An upstream declares a named `catasto.git(...)` repository at a pinned revision,
/// optionally with a `root` directory inside the repository from which entry paths start.
#[derive(Debug, Clone, Default, PartialEq, Config)]
#[config(group = "upstream")]
pub struct UpstreamSpec {
    /// Repository URL (`https://` URL or workspace-relative path), passed to `catasto.git(...)`.
    #[config(key = "url", type = "string")]
    pub url: String,

    /// Pinned tag, branch, or commit hash. Quote numeric revisions (e.g. `rev: "1.2"`).
    #[config(key = "rev", default = "main", type = "string")]
    pub rev: String,

    /// Directory inside the upstream repository that entry paths are relative to.
    #[config(key = "root", type = "path")]
    pub root: Option<PathBuf>,
}

/// Manifest file entry declaring a managed workspace file.
///
/// Each top-level entry in `catasto.ctmt` binds a workspace-relative file path
/// to a `catasto.{upstream,git,local}(...)` constructor call, optionally paired
/// with `executable` in a tuple `(call, executable: bool)`.
#[derive(Debug, Clone, Default, PartialEq, Config)]
#[config(group = "entry")]
pub struct ManifestEntrySpec {
    /// Constructor call declaring where canonical content comes from:
    /// `catasto.upstream(name, path: "...")`, `catasto.git(url, rev: ..., path: "...")`,
    /// or `catasto.local("...")`.
    #[config(
        key = "source",
        type = "catasto.upstream(...) | catasto.git(...) | catasto.local(...)"
    )]
    pub source: String,

    /// Whether the file is placed executable (`true`) or not (`false`), whatever the source says. Left out, the
    /// source decides: a git file's `100755` or not, and a local file is never executable.
    #[config(key = "executable", type = "boolean")]
    pub executable: Option<bool>,
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
        assert_eq!(specs[0].name, "upstream");
        assert_eq!(specs[1].name, "entry");

        let rendered = tomet_spec::config::render(
            &specs,
            "doc-testManifestSpec",
            "Manifest Reference",
            &["Intro paragraph."],
            &PrinterConfig::default(),
        );

        assert!(rendered.contains("==[ upstream ]") || rendered.contains("==[upstream]"));
        assert!(rendered.contains("==[ entry ]") || rendered.contains("==[entry]"));
        assert!(rendered.contains("`url`"));
        assert!(rendered.contains("`source`"));
        assert!(rendered.contains("`executable`"));
    }
}
