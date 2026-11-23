//! Manifest schema specifications for `catasto.ctmt`.
//!
//! Provides [`ConfigGroupSpec`] representations of `::upstream` and manifest path entries
//! using `#[derive(Config)]` from `tomet-spec`.

use std::path::PathBuf;

use tomet_spec::Config;
use tomet_spec::blueprint::{
    CtmtBlueprintSpec, EnumVariantExpr, HeaderAttrSpec, SeatExpr, TupleSpec, TypeArg,
    TypeArgValue, ValueSpec,
};
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

/// Manifest entry declaring a managed workspace file, directory, or submodule.
///
/// Each top-level entry in `catasto.ctmt` binds a workspace-relative path
/// to a `catasto.{upstream,git,local}(...)` constructor call; file and directory entries
/// may optionally pair the call with `executable` in a tuple `(call, executable: bool)`.
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

    /// Whether a managed file is placed executable (`true`) or not (`false`), whatever the source says. Left out, the
    /// source decides: a git file's `100755` or not, and a local file is never executable. Rejected when `submodule: true`.
    #[config(key = "executable", type = "boolean")]
    pub executable: Option<bool>,
}

/// Directory schema rules declared in `::zone(...)`.
///
/// A zone maps a workspace-relative directory path pattern (using `<?...>` holes, or `"."` for
/// the workspace root) to constraints on child entries of each matching directory (and, when
/// `recursive: true`, its non-ignored subdirectories).
#[derive(Debug, Clone, Default, PartialEq, Config)]
#[config(group = "zone")]
pub struct ZoneSpec {
    /// Entry patterns that must each be matched by at least one non-ignored child entry.
    #[config(key = "require", type = "pattern | (pattern, ...)")]
    pub require: Vec<String>,

    /// Entry patterns forming an allowlist; when present, every non-ignored child entry must match at least one.
    #[config(key = "allow", type = "pattern | (pattern, ...)")]
    pub allow: Vec<String>,

    /// Entry patterns that no non-ignored child entry may match.
    #[config(key = "deny", type = "pattern | (pattern, ...)")]
    pub deny: Vec<String>,

    /// Entry patterns excluded from `require`, `allow`, `deny`, and recursive traversal.
    #[config(key = "ignore", type = "pattern | (pattern, ...)")]
    pub ignore: Vec<String>,

    /// Whether this zone recursively applies to non-ignored subdirectories.
    #[config(key = "recursive", default = "false", type = "boolean")]
    pub recursive: bool,

    /// Nested constraints (`require`, `allow`, `deny`) applied only to non-directory entries.
    #[config(key = "files", type = "(require: ..., allow: ..., deny: ...)")]
    pub files: Option<String>,

    /// Nested constraints (`require`, `allow`, `deny`) applied only to directory entries.
    #[config(key = "dirs", type = "(require: ..., allow: ..., deny: ...)")]
    pub dirs: Option<String>,
}

/// Returns all configuration and declaration specification groups for `catasto.ctmt`.
pub fn all_manifest_specs() -> Vec<ConfigGroupSpec> {
    vec![
        UpstreamSpec::spec(),
        ZoneSpec::spec(),
        ManifestEntrySpec::spec(),
    ]
}

/// Returns the cotomet blueprint schema (`catasto@1.0.0.blueprint.ctmt`) for `catasto.ctmt`.
///
/// Reflects the exact header attributes (`::upstream`, `::zone`) and body entry shapes
/// accepted by [`crate::Manifest::parse`]:
///
/// - `::upstream(<$string(optional: true)>: <$catasto.git>)`
/// - `::zone(<$string(optional: true)>: (...))` with `require`, `allow`, `deny`, `ignore`,
///   `recursive`, and nested `files` / `dirs` `(require, allow, deny)` rule tuples
/// - Top-level entries mapping workspace paths (`<$string>`) to a source constructor
///   (`catasto.git`, `catasto.upstream`, `catasto.local`), or a tuple pairing a
///   source constructor with optional `executable: bool`
pub fn blueprint_spec() -> CtmtBlueprintSpec {
    let zone_pattern_list = SeatExpr::enum_of([
        EnumVariantExpr::ty("string"),
        EnumVariantExpr::type_with("list", [TypeArg::positional(TypeArgValue::ty("string"))]),
    ])
    .optional();

    let entry_rules_tuple = TupleSpec::map()
        .fixed("require", zone_pattern_list.clone())
        .fixed("allow", zone_pattern_list.clone())
        .fixed("deny", zone_pattern_list.clone());
    let optional_entry_rules =
        SeatExpr::enum_of([EnumVariantExpr::tuple(entry_rules_tuple)]).optional();

    let file_sources = || {
        [
            EnumVariantExpr::constructor("catasto", "git"),
            EnumVariantExpr::constructor("catasto", "upstream"),
            EnumVariantExpr::constructor("catasto", "local"),
        ]
    };

    let top_level_entry = SeatExpr::enum_of([
        EnumVariantExpr::enum_of(file_sources()),
        EnumVariantExpr::tuple(
            TupleSpec::new()
                .positional(ValueSpec::Seat(SeatExpr::enum_of(file_sources())))
                .fixed("executable", SeatExpr::bool().optional()),
        ),
    ]);

    CtmtBlueprintSpec::new("catasto")
        .header_attr(
            HeaderAttrSpec::new(
                "upstream",
                TupleSpec::map().dynamic(
                    SeatExpr::string().optional(),
                    SeatExpr::constructor("catasto", "git"),
                ),
            )
            .comment("Named upstream git repositories."),
        )
        .header_attr(
            HeaderAttrSpec::new(
                "zone",
                TupleSpec::map().dynamic(
                    SeatExpr::string().optional(),
                    TupleSpec::map()
                        .fixed("require", zone_pattern_list.clone())
                        .fixed("allow", zone_pattern_list.clone())
                        .fixed("deny", zone_pattern_list.clone())
                        .fixed("ignore", zone_pattern_list)
                        .fixed("recursive", SeatExpr::bool().optional())
                        .fixed("files", optional_entry_rules.clone())
                        .fixed("dirs", optional_entry_rules),
                ),
            )
            .comment("Directory schema and path matching rules."),
        )
        .body(TupleSpec::map().dynamic(SeatExpr::string(), top_level_entry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tomet_spec::PrinterConfig;

    #[test]
    fn manifest_specs_render_valid_tmt() {
        let specs = all_manifest_specs();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].name, "upstream");
        assert_eq!(specs[1].name, "zone");
        assert_eq!(specs[2].name, "entry");

        let rendered = tomet_spec::config::render(
            &specs,
            "doc-testManifestSpec",
            "Manifest Reference",
            &["Intro paragraph."],
            &PrinterConfig::default(),
        );

        assert!(rendered.contains("==[ upstream ]") || rendered.contains("==[upstream]"));
        assert!(rendered.contains("==[ zone ]") || rendered.contains("==[zone]"));
        assert!(rendered.contains("==[ entry ]") || rendered.contains("==[entry]"));
        assert!(rendered.contains("`url`"));
        assert!(rendered.contains("`require`"));
        assert!(rendered.contains("`recursive`"));
        assert!(rendered.contains("`files`"));
        assert!(rendered.contains("`dirs`"));
        assert!(rendered.contains("`source`"));
        assert!(rendered.contains("`executable`"));
    }

    #[test]
    fn blueprint_spec_self_checks_and_validates_repo_manifest() {
        let spec = blueprint_spec();
        let rendered = tomet_spec::blueprint::render(&spec, &PrinterConfig::default());
        let bp_ast = tomet_ast_parser::parse_data_file(&rendered)
            .expect("rendered blueprint parses as valid cotomet");

        let vocab = crate::manifest::vocabulary().clone();
        let bp_bindings = tomet_semantics::Bindings::for_document(&bp_ast, [vocab.clone()]);
        let bp_errors = tomet_validator_blueprint::validate_ctmt_blueprint(&bp_ast, &bp_bindings);
        assert!(
            bp_errors.is_empty(),
            "blueprint self-check failed: {bp_errors:?}\n{rendered}"
        );

        let repo_manifest_src = include_str!("../../../catasto.ctmt");
        let repo_manifest =
            tomet_ast_parser::parse_data_file(repo_manifest_src).expect("repo catasto.ctmt parses");
        let doc_bindings = tomet_semantics::Bindings::for_document(&repo_manifest, [vocab]);
        let doc_errors = tomet_validator_blueprint::validate_ctmt_against_blueprint(
            &repo_manifest,
            &bp_ast,
            &doc_bindings,
        );
        assert!(
            doc_errors.is_empty(),
            "repo catasto.ctmt failed blueprint validation: {doc_errors:?}"
        );
    }
}
