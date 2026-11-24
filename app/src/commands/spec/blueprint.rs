//! Cotomet blueprint generator from `catasto-manifest` schema definitions.

use tomet_spec::PrinterConfig;

/// Renders the complete formatted `catasto@1.0.0.blueprint.ctmt` schema string.
pub fn render(config: &PrinterConfig) -> String {
    let spec = catasto_manifest::blueprint_spec();
    tomet_spec::blueprint::render(&spec, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tomet_semantics::Bindings;
    use tomet_validator_blueprint::{
        extract_ctmt_blueprint_schema, validate_ctmt_against_blueprint, validate_ctmt_blueprint,
    };

    fn rendered() -> String {
        render(&PrinterConfig::default())
    }

    #[test]
    fn blueprint_parses_and_passes_self_check() {
        let text = rendered();
        let bp_ast = tomet_ast_parser::parse_data_file(&text)
            .expect("generated blueprint parses as valid cotomet");
        let schema = extract_ctmt_blueprint_schema(&bp_ast)
            .expect("generated blueprint extracts CtmtBlueprintSchema");
        assert_eq!(schema.target_kind, "catasto");

        let vocab = catasto_manifest::manifest::vocabulary().clone();
        let bindings = Bindings::for_document(&bp_ast, [vocab]);
        let errs = validate_ctmt_blueprint(&bp_ast, &bindings);
        assert!(
            errs.is_empty(),
            "blueprint self-check errors: {errs:?}\n{text}"
        );
    }

    #[test]
    fn blueprint_is_deterministic() {
        assert_eq!(rendered(), rendered());
    }

    #[test]
    fn blueprint_validates_repo_manifest_and_edge_cases() {
        let bp_ast = tomet_ast_parser::parse_data_file(&rendered()).expect("blueprint parses");
        let vocab = catasto_manifest::manifest::vocabulary().clone();

        // 1. Repository's own `catasto.ctmt` (uses ::upstream, ::zone with files/dirs, ::zone without files/dirs).
        let repo_manifest =
            tomet_ast_parser::parse_data_file(include_str!("../../../../catasto.ctmt"))
                .expect("repo catasto.ctmt parses");
        let repo_bindings = Bindings::for_document(&repo_manifest, [vocab.clone()]);
        let repo_errs = validate_ctmt_against_blueprint(&repo_manifest, &bp_ast, &repo_bindings);
        assert!(
            repo_errs.is_empty(),
            "repo catasto.ctmt validation errors: {repo_errs:?}"
        );

        // 2. Empty manifest (only ::kind) is valid.
        let empty = tomet_ast_parser::parse_data_file("::kind(catasto, version: \"1.0.0\")\n")
            .expect("empty manifest parses");
        let empty_bindings = Bindings::for_document(&empty, [vocab.clone()]);
        let empty_errs = validate_ctmt_against_blueprint(&empty, &bp_ast, &empty_bindings);
        assert!(
            empty_errs.is_empty(),
            "empty manifest validation errors: {empty_errs:?}"
        );

        // 3. All valid entry forms, including tuple with and without `executable`, and full `::zone` fields.
        let full = tomet_ast_parser::parse_data_file(
            r#"::kind(catasto, version: "1.0.0")
::upstream(
  common: catasto.git("https://example.com/common.git", rev: main, path: base)
)
::zone(
  ".": (
    require: "README.md"
    allow: ("README.md", "src", "scripts")
    deny: "*.tmp"
    ignore: (".git", "target")
    recursive: false
    files: (require: "README.md", allow: ("README.md", "*.sh"), deny: "*.bak")
    dirs: (allow: ("src", "scripts"))
  )
)

"vendor/lib": (catasto.git("https://example.com/lib.git", rev: main), submodule: true)
"a.txt": catasto.git("https://example.com/a.git", rev: main, path: "a.txt")
"b.txt": catasto.upstream(common, path: "b.txt")
"c.txt": catasto.local("local/c.txt")
"scripts/run.sh": (catasto.local("local/run.sh"), executable: true)
"scripts/link.sh": (catasto.upstream(common, path: "link.sh"), symlink: keep)
"scripts/plain.sh": (catasto.upstream(common, path: "plain.sh"),)
"#,
        )
        .expect("full manifest parses");
        let full_bindings = Bindings::for_document(&full, [vocab.clone()]);
        let full_errs = validate_ctmt_against_blueprint(&full, &bp_ast, &full_bindings);
        assert!(
            full_errs.is_empty(),
            "full manifest validation errors: {full_errs:?}"
        );

        // 4. Invalid entries are rejected.
        let invalid = tomet_ast_parser::parse_data_file(
            r#"::kind(catasto, version: "1.0.0")
::upstream(common: "not-a-git-call")
::zone(
  "src": (
    files: (unknown_scope_key: "x")
    unknown_zone_key: true
  )
)

"bad_entry": "not-a-call"
"bad_unknown_ctor_tuple": (catasto.repo("https://example.com/lib.git"), executable: true)
"bad_exec": (catasto.local("a.txt"), executable: "yes")
"#,
        )
        .expect("invalid manifest parses");
        let bad_bindings = Bindings::for_document(&invalid, [vocab]);
        let bad_errs = validate_ctmt_against_blueprint(&invalid, &bp_ast, &bad_bindings);
        assert_eq!(bad_errs.len(), 6, "expected 6 errors, got: {bad_errs:?}");
    }
}
