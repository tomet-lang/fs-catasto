use std::path::PathBuf;

use tomet_semantics::{Vocabulary, constructor_problems_in};

use super::*;

const USE: &str = "::use(catasto, version: \"1.0.0\")\n";
const VOCABULARIES: &str = "::use(catasto, version: \"1.0.0\")\n\
     ::upstream(\n  vocabularies: catasto.git(\"https://example.com/v.git\", rev: main)\n)\n";

fn path_source(path: &str) -> Source {
    Source::Path(path.into())
}

#[test]
fn canonical_vocabulary_parses_with_zero_problems() {
    let doc = tomet_ast_parser::parse_file(VOCABULARY_TMT).unwrap();
    let vocab = Vocabulary::from_file(&doc).expect("canonical catasto vocabulary");
    assert_eq!(vocab.namespace, "catasto");
    assert!(
        vocab.param_problems().is_empty(),
        "param problems: {:?}",
        vocab.param_problems()
    );
    let ctor_problems = constructor_problems_in(&doc);
    assert!(
        ctor_problems.is_empty(),
        "constructor problems: {ctor_problems:?}"
    );
    assert_eq!(
        vocab.constructors.keys().collect::<Vec<_>>(),
        ["git", "local", "repo", "upstream"]
    );
}

#[test]
fn parses_managed_entries_in_the_order_written() {
    let manifest = Manifest::parse(&format!(
        "{USE}\
         \"b/writ.vocabulary.tmt\": catasto.local(\"../vocabularies/writ.vocabulary.tmt\")\n\
         \"a/rust.vocabulary.tmt\": catasto.local(\"../vocabularies/rust.vocabulary.tmt\")\n"
    ))
    .unwrap();

    assert_eq!(
        manifest.managed,
        vec![
            Managed {
                path: "b/writ.vocabulary.tmt".into(),
                kind: ManagedKind::File {
                    source: path_source("../vocabularies/writ.vocabulary.tmt"),
                    executable: None,
                },
            },
            Managed {
                path: "a/rust.vocabulary.tmt".into(),
                kind: ManagedKind::File {
                    source: path_source("../vocabularies/rust.vocabulary.tmt"),
                    executable: None,
                },
            },
        ]
    );
}

#[test]
fn comments_and_unquoted_values_are_fine() {
    let manifest = Manifest::parse(&format!(
        "{USE}\
         // shared vocabularies\n\
         \"a.tmt\": catasto.local(../v/a.tmt) // the writ one\n"
    ))
    .unwrap();

    assert_eq!(
        manifest.managed[0].source(),
        Some(&path_source("../v/a.tmt"))
    );
}

#[test]
fn a_key_that_needs_no_quotes_may_be_bare() {
    let manifest = Manifest::parse(&format!("{USE}a.tmt: catasto.local(s)\n")).unwrap();
    assert_eq!(manifest.managed[0].path, PathBuf::from("a.tmt"));
}

#[test]
fn empty_text_is_an_empty_manifest() {
    assert_eq!(Manifest::parse("").unwrap(), Manifest::default());
    assert_eq!(Manifest::parse("\n  \n").unwrap(), Manifest::default());
}

#[test]
fn a_manifest_with_only_comments_or_use_is_empty() {
    assert_eq!(
        Manifest::parse("// nothing managed yet\n").unwrap(),
        Manifest::default()
    );
    assert_eq!(Manifest::parse(USE).unwrap(), Manifest::default());
    assert_eq!(
        Manifest::parse("::use(catasto)\n").unwrap(),
        Manifest::default()
    );
}

#[test]
fn use_declaration_supports_compatible_version_forms() {
    for decl in [
        "::use(catasto)",
        "::use(catasto, version: \"1.0.0\")",
        "::use(catasto, \"1.0.0\")",
        "::use(catasto, 1.0.0)",
        "::use(catasto, version: \"1.0\")",
        "::use(catasto, version: 1.0)",
        "::use(catasto, version: \"1\")",
        "::use(catasto, version: 1)",
        "::use(catasto, 1)",
    ] {
        let text = format!("{decl}\n\"a.tmt\": catasto.local(\"s\")\n");
        assert!(Manifest::parse(&text).is_ok(), "failed for {decl}");
    }
}

#[test]
fn use_declaration_rejects_incompatible_versions_and_unknown_vocabularies() {
    for bad in [
        "::use(catasto, version: \"2.0.0\")",
        "::use(catasto, version: \"0.9.0\")",
        "::use(catasto, version: \"1.1.0\")",
        "::use(catasto, version: \"1.0.1\")",
        "::use(catasto, version: \"1.0.0-alpha\")",
        "::use(catasto, version: bad)",
        "::use(other)",
        "::use()",
    ] {
        assert!(
            Manifest::parse(&format!("{bad}\n")).is_err(),
            "accepted {bad}"
        );
    }
}

#[test]
fn constructors_require_use_catasto_in_header() {
    let err = Manifest::parse("\"a.tmt\": catasto.local(\"s\")\n").unwrap_err();
    assert!(err.to_string().contains("::use(catasto"), "{err}");

    let err =
        Manifest::parse("::upstream(v: catasto.git(\"https://example.com/v.git\", rev: main))\n")
            .unwrap_err();
    assert!(err.to_string().contains("::use(catasto"), "{err}");
}

#[test]
fn an_upstream_reads_as_the_entry_written_in_full() {
    let named = Manifest::parse(&format!(
        "{VOCABULARIES}\n\"a.tmt\": catasto.upstream(vocabularies, path: \"x/a.tmt\")\n"
    ))
    .unwrap();
    let written = Manifest::parse(&format!(
        "{USE}\"a.tmt\": catasto.git(\"https://example.com/v.git\", rev: main, path: \"x/a.tmt\")\n"
    ))
    .unwrap();
    assert_eq!(named, written);
}

#[test]
fn a_root_is_joined_in_front_of_the_path() {
    let rooted = Manifest::parse(&format!(
        "{USE}\
         ::upstream(v: catasto.git(\"https://example.com/v.git\", rev: main, root: vocabularies))\n\
         \"a.tmt\": catasto.upstream(v, path: \"rust/a.tmt\")\n"
    ))
    .unwrap();
    let written = Manifest::parse(&format!(
        "{USE}\"a.tmt\": catasto.git(\"https://example.com/v.git\", rev: main, path: \"vocabularies/rust/a.tmt\")\n"
    ))
    .unwrap();
    assert_eq!(rooted, written);
}

#[test]
fn a_root_is_checked_where_it_is_declared() {
    for root in ["/etc", "../outside", "a/../b", "\"\""] {
        let err = Manifest::parse(&format!(
            "{USE}::upstream(v: catasto.git(\"https://example.com/v.git\", rev: main, root: {root}))\n"
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
    let err = Manifest::parse(&format!(
        "{USE}\
         ::upstream(v: catasto.git(\"https://example.com/v.git\", rev: main, root: vocabularies))\n\
         \"a.tmt\": catasto.upstream(v, path: \"../secret.tmt\")\n"
    ))
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
         \"b.tmt\": catasto.upstream(vocabularies, path: \"b.tmt\")\n\
         \"a.tmt\": catasto.upstream(vocabularies, path: \"a.tmt\")\n"
    ))
    .unwrap();
    let paths: Vec<_> = manifest.managed.iter().map(|m| m.path.clone()).collect();
    assert_eq!(paths, [PathBuf::from("b.tmt"), PathBuf::from("a.tmt")]);
    for managed in &manifest.managed {
        assert!(matches!(
            managed.source(),
            Some(Source::Git { git, rev, .. }) if git == "https://example.com/v.git" && rev == "main"
        ));
    }
}

#[test]
fn multiple_upstream_declarations_are_allowed_if_names_are_unique() {
    let manifest = Manifest::parse(&format!(
        "{USE}\
         ::upstream(a: catasto.git(\"https://example.com/a.git\", rev: main))\n\
         ::upstream(b: catasto.git(\"https://example.com/b.git\", rev: v1))\n\
         \"x.tmt\": catasto.upstream(a, path: \"x.tmt\")\n\
         \"y.tmt\": catasto.upstream(b, path: \"y.tmt\")\n"
    ))
    .unwrap();
    assert_eq!(manifest.managed.len(), 2);

    let err = Manifest::parse(&format!(
        "{USE}\
         ::upstream(a: catasto.git(\"https://example.com/a.git\", rev: main))\n\
         ::upstream(a: catasto.git(\"https://example.com/b.git\", rev: v1))\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("declared twice"), "{err}");
}

#[test]
fn a_local_source_and_a_written_out_git_source_need_no_upstream() {
    let manifest = Manifest::parse(&format!(
        "{VOCABULARIES}\n\
         \"a.tmt\": catasto.local(\"../a.tmt\")\n\
         \"b.tmt\": catasto.git(\"https://example.com/o.git\", rev: v1, path: \"b.tmt\")\n"
    ))
    .unwrap();
    assert_eq!(manifest.managed[0].source(), Some(&path_source("../a.tmt")));
    assert!(matches!(
        manifest.managed[1].source(),
        Some(Source::Git { .. })
    ));
}

#[test]
fn an_unknown_upstream_lists_the_declared_ones() {
    let err = Manifest::parse(&format!(
        "{VOCABULARIES}\n\"a.tmt\": catasto.upstream(vocabularis, path: \"a.tmt\")\n"
    ))
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "a.tmt: unknown upstream `vocabularis` (declared: vocabularies)"
    );
}

#[test]
fn an_upstream_with_none_declared_says_where_to_declare() {
    let err = Manifest::parse(&format!(
        "{USE}\"a.tmt\": catasto.upstream(v, path: \"a.tmt\")\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("add `::upstream(...)`"), "{err}");
}

#[test]
fn an_upstream_constructor_rejects_git_or_rev_arguments() {
    for extra in ["url: \"https://example.com/o.git\"", "rev: v2"] {
        let err = Manifest::parse(&format!(
            "{VOCABULARIES}\n\"a.tmt\": catasto.upstream(vocabularies, {extra}, path: \"a.tmt\")\n"
        ))
        .unwrap_err();
        assert!(err.to_string().contains("has no argument"), "{err}");
    }
}

#[test]
fn an_upstream_call_needs_a_path() {
    let err = Manifest::parse(&format!(
        "{VOCABULARIES}\n\"a.tmt\": catasto.upstream(vocabularies)\n"
    ))
    .unwrap_err();
    assert!(
        err.to_string().contains("requires the argument `path`"),
        "{err}"
    );
}

#[test]
fn an_upstream_is_checked_where_it_is_declared() {
    let err = Manifest::parse(&format!(
        "{USE}::upstream(v: catasto.git(\"git@example.com:o/v.git\", rev: main))\n"
    ))
    .unwrap_err();
    assert!(err.to_string().starts_with("upstream `v`: "), "{err}");

    let err = Manifest::parse(&format!(
        "{USE}::upstream(v: catasto.git(\"https://example.com/v.git\", rev: 1.2))\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("write it in quotes"), "{err}");

    let err = Manifest::parse(&format!(
        "{USE}::upstream(v: catasto.git(\"https://example.com/v.git\", path: \"a\"))\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("cannot specify `path`"), "{err}");

    let err = Manifest::parse(&format!(
        "{USE}::upstream(v: catasto.git(\"https://example.com/v.git\", symlink: keep))\n"
    ))
    .unwrap_err();
    assert!(
        err.to_string().contains("cannot specify `symlink`"),
        "{err}"
    );

    let err = Manifest::parse(&format!("{USE}::upstream(v: catasto.local(\"a\"))\n")).unwrap_err();
    assert!(
        err.to_string().contains("must use `catasto.git(...)`"),
        "{err}"
    );
}

#[test]
fn rejects_legacy_upstreams_and_unknown_declarations() {
    assert!(
        Manifest::parse("::upstreams{ v: { git: \"https://example.com/v.git\", rev: main } }\n")
            .is_err()
    );

    let err = Manifest::parse(&format!(
        "{USE}::upstreams(v: catasto.git(\"https://example.com/v.git\", rev: main))\n"
    ))
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("unknown declaration `::upstreams`"),
        "{err}"
    );

    let err = Manifest::parse("::kind(catalog)\n").unwrap_err();
    assert!(
        err.to_string().contains("unknown declaration `::kind`"),
        "{err}"
    );

    let err =
        Manifest::parse(&format!("{USE}::upstream(){{ v: catasto.git(\"u\") }}\n")).unwrap_err();
    assert!(
        err.to_string().contains("takes entries in parentheses"),
        "{err}"
    );
}

#[test]
fn rejects_legacy_entry_shapes_and_unknown_tuple_fields() {
    for bad in [
        "\"a\": \"s\"",
        "\"a\": { source: \"s\" }",
        "\"a\": (executable: true)",
        "\"a\": (catasto.local(\"s\"), catasto.local(\"t\"))",
        "\"a\": (catasto.local(\"s\"), owner: x)",
        "\"a\": (\"s\", executable: true)",
    ] {
        assert!(
            Manifest::parse(&format!("{USE}{bad}\n")).is_err(),
            "accepted {bad}"
        );
    }
}

#[test]
fn an_entry_may_declare_its_executable_mode() {
    let manifest = Manifest::parse(&format!(
        "{USE}\
         \"a.sh\": (catasto.local(\"s/a.sh\"), executable: true)\n\
         \"b.txt\": (catasto.local(\"s/b.txt\"), executable: false)\n\
         \"c.txt\": catasto.local(\"s/c.txt\")\n"
    ))
    .unwrap();

    let declared: Vec<Option<bool>> = manifest.managed.iter().map(|m| m.executable()).collect();
    assert_eq!(declared, vec![Some(true), Some(false), None]);
}

#[test]
fn a_declared_mode_wins_over_the_sources_and_an_absent_one_leaves_it_to_the_source() {
    let manifest = Manifest::parse(&format!(
        "{USE}\
         \"a\": (catasto.local(s), executable: true)\n\
         \"b\": (catasto.local(s), executable: false)\n\
         \"c\": catasto.local(s)\n"
    ))
    .unwrap();
    let [a, b, c] = &manifest.managed[..] else {
        panic!("three entries")
    };

    assert!(a.is_executable(false));
    assert!(!b.is_executable(true));
    assert!(c.is_executable(true));
    assert!(!c.is_executable(false));
}

#[test]
fn the_executable_mode_must_be_a_boolean() {
    let err = Manifest::parse(&format!(
        "{USE}\"a\": (catasto.local(s), executable: yes)\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("executable"), "{err}");
    assert!(
        Manifest::parse(&format!(
            "{USE}\"a\": (catasto.local(s), executable: \"true\")\n"
        ))
        .is_err()
    );
}

#[test]
fn rejects_unknown_namespace_or_constructor_and_empty_local_path() {
    assert!(Manifest::parse(&format!("{USE}\"a\": other.local(\"s\")\n")).is_err());
    assert!(Manifest::parse(&format!("{USE}\"a\": catasto.ftp(\"b\")\n")).is_err());
    assert!(Manifest::parse(&format!("{USE}\"a\": catasto.local(\"\")\n")).is_err());
}

#[test]
fn parses_a_git_source() {
    let manifest = Manifest::parse(&format!(
        "{USE}\"a\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"dir/w.tmt\")\n"
    ))
    .unwrap();

    assert_eq!(
        manifest.managed[0].source(),
        Some(&Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "v1".into(),
            path: "dir/w.tmt".into(),
            symlink: Symlink::Follow,
        })
    );
}

#[test]
fn parses_symlink_keep_and_follow_in_git_source_and_upstream() {
    let manifest = Manifest::parse(&format!(
        "{VOCABULARIES}\n\
         \"a\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"a\", symlink: keep)\n\
         \"b\": catasto.upstream(vocabularies, path: \"b\", symlink: keep)\n\
         \"c\": catasto.upstream(vocabularies, path: \"c\", symlink: follow)\n"
    ))
    .unwrap();

    assert_eq!(
        manifest.managed[0].source(),
        Some(&Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "v1".into(),
            path: "a".into(),
            symlink: Symlink::Keep,
        })
    );
    assert_eq!(
        manifest.managed[1].source(),
        Some(&Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "main".into(),
            path: "b".into(),
            symlink: Symlink::Keep,
        })
    );
    assert_eq!(
        manifest.managed[2].source(),
        Some(&Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "main".into(),
            path: "c".into(),
            symlink: Symlink::Follow,
        })
    );
}

#[test]
fn rejects_invalid_symlink_values() {
    for bad in ["true", "false", "123", "\"\"", "preserve"] {
        let err = Manifest::parse(&format!(
            "{USE}\"a\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"a\", symlink: {bad})\n"
        ))
        .unwrap_err();
        assert!(
            err.to_string().contains("symlink"),
            "expected error mentioning symlink, got: {err}"
        );
    }
}

#[test]
fn git_source_rejects_unknown_arguments_and_root_in_entry() {
    let err = Manifest::parse(&format!(
        "{USE}\"a\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"a\", symlink: keep, extra: x)\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("has no argument `extra`"), "{err}");

    let err = Manifest::parse(&format!(
        "{USE}\"a\": catasto.git(\"https://example.com/v.git\", rev: v1, root: sub, path: \"a\")\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("cannot specify `root`"), "{err}");
}

#[test]
fn a_rev_that_reads_as_a_number_must_be_quoted() {
    let text = |rev: &str| format!("{USE}\"a\": catasto.git(\"../v\", rev: {rev}, path: \"p\")\n");

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
        let text = format!("{USE}\"a\": catasto.git(\"{url}\", rev: v, path: \"p\")\n");
        assert!(Manifest::parse(&text).is_err(), "accepted {url}");
    }
}

#[test]
fn accepts_https_and_relative_local_git_urls() {
    for url in ["https://github.com/tomet-lang/twrit.git", "../twrit"] {
        let text = format!("{USE}\"a\": catasto.git(\"{url}\", rev: v, path: \"p\")\n");
        assert!(Manifest::parse(&text).is_ok(), "rejected {url}");
    }
}

#[test]
fn rejects_malformed_git_sources() {
    for call in [
        "catasto.git(\"https://example.com/u.git\", rev: \"v\")",
        "catasto.git(rev: \"v\", path: \"p\")",
        "catasto.git(\"-u\", rev: \"v\", path: \"p\")",
        "catasto.git(\"u\", rev: \"-v\", path: \"p\")",
        "catasto.git(\"\", rev: \"v\", path: \"p\")",
        "catasto.git(\"../u\", rev: \"v\", path: \"../p\")",
        "catasto.git(\"../u\", rev: \"v\", path: \"/p\")",
        "catasto.git()",
    ] {
        let text = format!("{USE}\"a\": {call}\n");
        assert!(Manifest::parse(&text).is_err(), "accepted {call}");
    }
}

#[test]
fn rejects_paths_that_escape_the_workspace() {
    for bad in ["../a", "/etc/a", "a/../../b", ""] {
        let text = format!("{USE}\"{bad}\": catasto.local(\"s\")\n");
        assert!(Manifest::parse(&text).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn rejects_duplicate_paths() {
    let text = format!("{USE}\"a\": catasto.local(\"s\")\n\"a\": catasto.local(\"t\")\n");
    let err = Manifest::parse(&text).unwrap_err();
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
            symlink: Symlink::Follow,
        },
        Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "v1".into(),
            path: "dir/w.tmt".into(),
            symlink: Symlink::Keep,
        },
    ] {
        let json = serde_json::to_string(&source).unwrap();
        assert_eq!(serde_json::from_str::<Source>(&json).unwrap(), source);
    }

    // Verify that Symlink::Follow does not emit `"symlink"` in JSON
    let default_git = Source::Git {
        git: "https://example.com/v.git".into(),
        rev: "v1".into(),
        path: "dir/w.tmt".into(),
        symlink: Symlink::Follow,
    };
    let json = serde_json::to_string(&default_git).unwrap();
    assert!(
        !json.contains("symlink"),
        "default git source emitted symlink in JSON: {json}"
    );

    let keep_git = Source::Git {
        git: "https://example.com/v.git".into(),
        rev: "v1".into(),
        path: "dir/w.tmt".into(),
        symlink: Symlink::Keep,
    };
    let json_keep = serde_json::to_string(&keep_git).unwrap();
    assert!(json_keep.contains("\"symlink\":\"keep\""), "{json_keep}");
}

#[test]
fn upstream_with_symlink_keep_resolves_identically_to_full_form() {
    let manifest = Manifest::parse(&format!(
        "{VOCABULARIES}\n\
         \"a.tmt\": catasto.upstream(vocabularies, path: \"a.tmt\", symlink: keep)\n\
         \"b.tmt\": catasto.git(\"https://example.com/v.git\", rev: main, path: \"a.tmt\", symlink: keep)\n"
    ))
    .unwrap();

    assert_eq!(manifest.managed[0].source(), manifest.managed[1].source());
    assert_eq!(
        serde_json::to_string(manifest.managed[0].source().unwrap()).unwrap(),
        serde_json::to_string(manifest.managed[1].source().unwrap()).unwrap()
    );
}

#[test]
fn parses_repo_entries_with_and_without_branch_and_submodule() {
    let manifest = Manifest::parse(&format!(
        "{USE}\
         \"tools/sisho\": catasto.repo(\"https://example.com/sisho.git\", branch: main, submodule: true)\n\
         \"vendor/lib\": catasto.repo(\"../lib\")\n"
    ))
    .unwrap();

    assert!(manifest.managed[0].is_repo());
    assert!(!manifest.managed[0].is_file());
    assert_eq!(manifest.managed[0].source(), None);
    assert_eq!(
        manifest.managed[0].repo(),
        Some(&Repo {
            git: "https://example.com/sisho.git".into(),
            branch: Some("main".into()),
            submodule: true,
        })
    );
    assert_eq!(
        manifest.managed[1].repo(),
        Some(&Repo {
            git: "../lib".into(),
            branch: None,
            submodule: false,
        })
    );
}

#[test]
fn rejects_executable_on_repo_entry() {
    for flag in ["true", "false"] {
        let err = Manifest::parse(&format!(
            "{USE}\"tools/sisho\": (catasto.repo(\"https://example.com/sisho.git\"), executable: {flag})\n"
        ))
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("catasto.repo(...) cannot declare executable"),
            "{err}"
        );
    }
}

#[test]
fn rejects_invalid_repo_url_and_branch() {
    let err = Manifest::parse(&format!(
        "{USE}\"tools/sisho\": catasto.repo(\"git@github.com:org/repo.git\")\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("ssh"), "{err}");

    let err = Manifest::parse(&format!(
        "{USE}\"tools/sisho\": catasto.repo(\"https://example.com/sisho.git\", branch: 1.2)\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("write it in quotes"), "{err}");

    for bad_branch in ["\"\"", "\"-flag\""] {
        let err = Manifest::parse(&format!(
            "{USE}\"tools/sisho\": catasto.repo(\"https://example.com/sisho.git\", branch: {bad_branch})\n"
        ))
        .unwrap_err();
        assert!(err.to_string().contains("invalid branch name"), "{err}");
    }
}

#[test]
fn rejects_entries_inside_a_declared_repo() {
    let err = Manifest::parse(&format!(
        "{USE}\
         \"tools/sisho\": catasto.repo(\"https://example.com/sisho.git\")\n\
         \"tools/sisho/README.md\": catasto.local(\"../README.md\")\n"
    ))
    .unwrap_err();
    assert!(
        err.to_string().contains("cannot be inside declared repo"),
        "{err}"
    );

    // Order independent: file declared before repo is also rejected.
    let err = Manifest::parse(&format!(
        "{USE}\
         \"tools/sisho/sub/a\": catasto.local(\"../a\")\n\
         \"./tools/sisho\": catasto.repo(\"https://example.com/sisho.git\")\n"
    ))
    .unwrap_err();
    assert!(
        err.to_string().contains("cannot be inside declared repo"),
        "{err}"
    );

    // Sibling prefix like tools/sisho-extra is NOT inside tools/sisho.
    assert!(
        Manifest::parse(&format!(
            "{USE}\
             \"tools/sisho\": catasto.repo(\"https://example.com/sisho.git\")\n\
             \"tools/sisho-extra/file\": catasto.local(\"../a\")\n"
        ))
        .is_ok()
    );
}
