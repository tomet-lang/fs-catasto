use std::path::PathBuf;

use tomet_semantics::{Vocabulary, entry_problems_in};

use super::*;

const KIND: &str = "::kind(catasto, version: \"1.0.0\")\n";
const VOCABULARIES: &str = "::kind(catasto, version: \"1.0.0\")\n\
     ::upstream(\n  vocabularies: catasto.git(\"https://example.com/v.git\", rev: main)\n)\n";

fn path_source(path: &str) -> Source {
    Source::Path(path.into())
}

#[test]
fn canonical_vocabulary_parses_with_zero_problems() {
    let doc = tomet_ast_parser::parse_file(VOCABULARY_TMT).unwrap();
    let vocab = Vocabulary::from_file(&doc).expect("canonical catasto vocabulary");
    assert_eq!(vocab.namespace, "catasto");
    let problems = entry_problems_in(&doc);
    assert!(problems.is_empty(), "entry problems: {problems:?}");
    assert_eq!(
        vocab.attributes.keys().collect::<Vec<_>>(),
        ["upstream", "zone"]
    );
    assert_eq!(
        vocab.constructors.keys().collect::<Vec<_>>(),
        ["git", "local", "upstream"]
    );
}

#[test]
fn parses_managed_entries_in_the_order_written() {
    let manifest = Manifest::parse(&format!(
        "{KIND}\
         \"b/writ.vocabulary.tmt\": catasto.local(\"../vocabularies/writ.vocabulary.tmt\")\n\
         \"a/rust.vocabulary.tmt\": catasto.local(\"../vocabularies/rust.vocabulary.tmt\")\n"
    ))
    .unwrap();

    assert_eq!(
        manifest.managed,
        vec![
            Managed {
                path: "b/writ.vocabulary.tmt".into(),
                upstream: None,
                kind: ManagedKind::File {
                    source: path_source("../vocabularies/writ.vocabulary.tmt"),
                    executable: None,
                    symlink: Symlink::Follow,
                },
            },
            Managed {
                path: "a/rust.vocabulary.tmt".into(),
                upstream: None,
                kind: ManagedKind::File {
                    source: path_source("../vocabularies/rust.vocabulary.tmt"),
                    executable: None,
                    symlink: Symlink::Follow,
                },
            },
        ]
    );
}

#[test]
fn comments_and_unquoted_values_are_fine() {
    let manifest = Manifest::parse(&format!(
        "{KIND}\
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
    let manifest = Manifest::parse(&format!("{KIND}a.tmt: catasto.local(s)\n")).unwrap();
    assert_eq!(manifest.managed[0].path, PathBuf::from("a.tmt"));
}

#[test]
fn empty_text_is_an_empty_manifest() {
    assert_eq!(Manifest::parse("").unwrap(), Manifest::default());
    assert_eq!(Manifest::parse("\n  \n").unwrap(), Manifest::default());
}

#[test]
fn a_manifest_with_only_comments_or_kind_is_empty() {
    assert_eq!(
        Manifest::parse("// nothing managed yet\n").unwrap(),
        Manifest::default()
    );
    assert_eq!(Manifest::parse(KIND).unwrap(), Manifest::default());
    assert_eq!(
        Manifest::parse("::kind(catasto)\n").unwrap(),
        Manifest::default()
    );
}

#[test]
fn kind_declaration_supports_compatible_version_forms() {
    for decl in [
        "::kind(catasto)",
        "::kind(catasto, version: \"1.0.0\")",
        "::kind(catasto, \"1.0.0\")",
        "::kind(catasto, 1.0.0)",
        "::kind(catasto, version: \"1.0\")",
        "::kind(catasto, version: 1.0)",
        "::kind(catasto, version: \"1\")",
        "::kind(catasto, version: 1)",
        "::kind(catasto, 1)",
    ] {
        let text = format!("{decl}\n\"a.tmt\": catasto.local(\"s\")\n");
        assert!(Manifest::parse(&text).is_ok(), "failed for {decl}");
    }
}

#[test]
fn kind_declaration_rejects_incompatible_versions_and_other_kinds() {
    for bad in [
        "::kind(catasto, version: \"2.0.0\")",
        "::kind(catasto, version: \"0.9.0\")",
        "::kind(catasto, version: \"1.1.0\")",
        "::kind(catasto, version: \"1.0.1\")",
        "::kind(catasto, version: \"1.0.0-alpha\")",
        "::kind(catasto, version: bad)",
        "::kind(other)",
        "::kind()",
    ] {
        assert!(
            Manifest::parse(&format!("{bad}\n")).is_err(),
            "accepted {bad}"
        );
    }
}

#[test]
fn constructors_require_kind_catasto_in_header() {
    let err = Manifest::parse("\"a.tmt\": catasto.local(\"s\")\n").unwrap_err();
    assert!(err.to_string().contains("::kind(catasto"), "{err}");

    let err =
        Manifest::parse("::upstream(v: catasto.git(\"https://example.com/v.git\", rev: main))\n")
            .unwrap_err();
    assert!(err.to_string().contains("::kind(catasto"), "{err}");
}

#[test]
fn use_catasto_is_refused_with_the_kind_to_write() {
    let err = Manifest::parse("::use(catasto, version: \"1.0.0\")\n").unwrap_err();
    assert!(
        err.to_string()
            .contains("write `::kind(catasto, version: \"1.0.0\")`"),
        "{err}"
    );
}

#[test]
fn an_attribute_reads_the_same_with_its_namespace() {
    let bare = Manifest::parse(&format!(
        "{VOCABULARIES}\n\"a.tmt\": catasto.upstream(vocabularies, path: \"x/a.tmt\")\n"
    ))
    .unwrap();
    let qualified = Manifest::parse(
        "::kind(catasto, version: \"1.0.0\")\n\
         ::catasto.upstream(\n  vocabularies: catasto.git(\"https://example.com/v.git\", rev: main)\n)\n\n\
         \"a.tmt\": catasto.upstream(vocabularies, path: \"x/a.tmt\")\n",
    )
    .unwrap();
    assert_eq!(bare, qualified);
}

#[test]
fn an_unknown_declaration_names_the_attributes_of_the_vocabulary() {
    let err = Manifest::parse(&format!("{KIND}::mirror(a: b)\n")).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("unknown declaration `::mirror`"),
        "{message}"
    );
    assert!(message.contains("`::upstream` and `::zone`"), "{message}");

    let err = Manifest::parse(&format!("{KIND}::other.upstream(a: b)\n")).unwrap_err();
    assert!(
        err.to_string()
            .contains("unknown declaration `::other.upstream`"),
        "{err}"
    );
}

#[test]
fn an_upstream_reads_as_the_entry_written_in_full_and_preserves_provenance() {
    let named = Manifest::parse(&format!(
        "{VOCABULARIES}\n\"a.tmt\": catasto.upstream(vocabularies, path: \"x/a.tmt\")\n"
    ))
    .unwrap();
    let written = Manifest::parse(&format!(
        "{KIND}\"a.tmt\": catasto.git(\"https://example.com/v.git\", rev: main, path: \"x/a.tmt\")\n"
    ))
    .unwrap();
    assert_eq!(named.managed[0].source(), written.managed[0].source());
    assert_eq!(
        named.managed[0].upstream(),
        Some(&UpstreamRef {
            name: "vocabularies".into(),
            path: Some("x/a.tmt".into()),
        })
    );
    assert_eq!(written.managed[0].upstream(), None);
}

#[test]
fn an_upstream_path_is_joined_in_front_of_the_entry_path() {
    let rooted = Manifest::parse(&format!(
        "{KIND}\
         ::upstream(v: catasto.git(\"https://example.com/v.git\", rev: main, path: vocabularies))\n\
         \"a.tmt\": catasto.upstream(v, path: \"rust/a.tmt\")\n"
    ))
    .unwrap();
    let written = Manifest::parse(&format!(
        "{KIND}\"a.tmt\": catasto.git(\"https://example.com/v.git\", rev: main, path: \"vocabularies/rust/a.tmt\")\n"
    ))
    .unwrap();
    assert_eq!(rooted.managed[0].source(), written.managed[0].source());
    assert_eq!(
        rooted.managed[0].upstream(),
        Some(&UpstreamRef {
            name: "v".into(),
            path: Some("rust/a.tmt".into()),
        })
    );
}

#[test]
fn an_upstream_path_is_checked_where_it_is_declared() {
    for bad_path in ["/etc", "../outside", "a/../b", "\"\""] {
        let err = Manifest::parse(&format!(
            "{KIND}::upstream(v: catasto.git(\"https://example.com/v.git\", rev: main, path: {bad_path}))\n"
        ))
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("git path must be relative and stay inside the repository"),
            "{bad_path}: {err}"
        );
    }
}

#[test]
fn a_path_cannot_climb_out_of_the_upstream_boundary() {
    for bad in ["../secret.tmt", "a/../../secret.tmt", "/etc/passwd", ""] {
        let err = Manifest::parse(&format!(
            "{KIND}\
             ::upstream(v: catasto.git(\"https://example.com/v.git\", rev: main, path: vocabularies))\n\
             \"a.tmt\": catasto.upstream(v, path: \"{bad}\")\n"
        ))
        .unwrap_err();
        assert!(
            err.to_string().contains("stay inside the repository"),
            "{bad}: {err}"
        );
    }
}

#[test]
fn an_upstream_call_may_omit_path_to_take_the_whole_upstream() {
    let manifest = Manifest::parse(&format!(
        "{KIND}\
         ::upstream(\n\
           with_path: catasto.git(\"https://example.com/v.git\", rev: main, path: vocabularies)\n\
           repo_root: catasto.git(\"https://example.com/r.git\", rev: v1)\n\
         )\n\
         \"dir_a\": catasto.upstream(with_path)\n\
         \"dir_b\": catasto.upstream(repo_root)\n"
    ))
    .unwrap();

    assert_eq!(
        manifest.managed[0].source(),
        Some(&Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "main".into(),
            path: Some("vocabularies".into()),
        })
    );
    assert_eq!(
        manifest.managed[0].upstream(),
        Some(&UpstreamRef {
            name: "with_path".into(),
            path: None,
        })
    );
    assert_eq!(
        manifest.managed[1].source(),
        Some(&Source::Git {
            git: "https://example.com/r.git".into(),
            rev: "v1".into(),
            path: None,
        })
    );
    assert_eq!(
        manifest.managed[1].upstream(),
        Some(&UpstreamRef {
            name: "repo_root".into(),
            path: None,
        })
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
        "{KIND}\
         ::upstream(a: catasto.git(\"https://example.com/a.git\", rev: main))\n\
         ::upstream(b: catasto.git(\"https://example.com/b.git\", rev: v1))\n\
         \"x.tmt\": catasto.upstream(a, path: \"x.tmt\")\n\
         \"y.tmt\": catasto.upstream(b, path: \"y.tmt\")\n"
    ))
    .unwrap();
    assert_eq!(manifest.managed.len(), 2);

    let err = Manifest::parse(&format!(
        "{KIND}\
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
        "{KIND}\"a.tmt\": catasto.upstream(v, path: \"a.tmt\")\n"
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
fn an_upstream_is_checked_where_it_is_declared() {
    let err = Manifest::parse(&format!(
        "{KIND}::upstream(v: catasto.git(\"git@example.com:o/v.git\", rev: main))\n"
    ))
    .unwrap_err();
    assert!(err.to_string().starts_with("upstream `v`: "), "{err}");

    let err = Manifest::parse(&format!(
        "{KIND}::upstream(v: catasto.git(\"https://example.com/v.git\", rev: 1.2))\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("write it in quotes"), "{err}");

    let err = Manifest::parse(&format!(
        "{KIND}::upstream(v: catasto.git(\"https://example.com/v.git\", root: \"a\"))\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("has no argument `root`"), "{err}");

    let err = Manifest::parse(&format!(
        "{KIND}::upstream(v: catasto.git(\"https://example.com/v.git\", symlink: keep))\n"
    ))
    .unwrap_err();
    assert!(
        err.to_string().contains("has no argument `symlink`"),
        "{err}"
    );

    let err = Manifest::parse(&format!(
        "{KIND}::upstream(v: catasto.git(\"https://example.com/v.git\", submodule: true))\n"
    ))
    .unwrap_err();
    assert!(
        err.to_string().contains("has no argument `submodule`"),
        "{err}"
    );

    let err = Manifest::parse(&format!("{KIND}::upstream(v: catasto.local(\"a\"))\n")).unwrap_err();
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
        "{KIND}::upstreams(v: catasto.git(\"https://example.com/v.git\", rev: main))\n"
    ))
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("unknown declaration `::upstreams`"),
        "{err}"
    );

    let err = Manifest::parse("::kind(catalog)\n").unwrap_err();
    assert!(
        err.to_string()
            .contains("a manifest is of kind `catasto`, not `catalog`"),
        "{err}"
    );

    let err =
        Manifest::parse(&format!("{KIND}::upstream(){{ v: catasto.git(\"u\") }}\n")).unwrap_err();
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
            Manifest::parse(&format!("{KIND}{bad}\n")).is_err(),
            "accepted {bad}"
        );
    }
}

#[test]
fn an_entry_may_declare_its_executable_mode() {
    let manifest = Manifest::parse(&format!(
        "{KIND}\
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
        "{KIND}\
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
        "{KIND}\"a\": (catasto.local(s), executable: yes)\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("executable"), "{err}");
    assert!(
        Manifest::parse(&format!(
            "{KIND}\"a\": (catasto.local(s), executable: \"true\")\n"
        ))
        .is_err()
    );
}

#[test]
fn rejects_unknown_namespace_or_constructor_and_empty_local_path() {
    assert!(Manifest::parse(&format!("{KIND}\"a\": other.local(\"s\")\n")).is_err());
    assert!(Manifest::parse(&format!("{KIND}\"a\": catasto.ftp(\"b\")\n")).is_err());
    assert!(Manifest::parse(&format!("{KIND}\"a\": catasto.local(\"\")\n")).is_err());
}

#[test]
fn parses_a_git_source() {
    let manifest = Manifest::parse(&format!(
        "{KIND}\"a\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"dir/w.tmt\")\n\
         \"b\": catasto.git(\"https://example.com/v.git\", rev: v1)\n"
    ))
    .unwrap();

    assert_eq!(
        manifest.managed[0].source(),
        Some(&Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "v1".into(),
            path: Some("dir/w.tmt".into()),
        })
    );
    assert_eq!(manifest.managed[0].symlink(), Some(Symlink::Follow));
    assert_eq!(
        manifest.managed[1].source(),
        Some(&Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "v1".into(),
            path: None,
        })
    );
    assert_eq!(manifest.managed[1].symlink(), Some(Symlink::Follow));
}

#[test]
fn parses_symlink_keep_and_follow_in_entry_tuple_for_git_and_upstream() {
    let manifest = Manifest::parse(&format!(
        "{VOCABULARIES}\n\
         \"a\": (catasto.git(\"https://example.com/v.git\", rev: v1, path: \"a\"), symlink: keep)\n\
         \"b\": (catasto.upstream(vocabularies, path: \"b\"), symlink: keep)\n\
         \"c\": (catasto.upstream(vocabularies, path: \"c\"), symlink: follow)\n\
         \"d\": (catasto.upstream(vocabularies), symlink: keep)\n\
         \"e\": (catasto.git(\"https://example.com/v.git\", rev: v1), symlink: keep)\n"
    ))
    .unwrap();

    assert_eq!(
        manifest.managed[0].source(),
        Some(&Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "v1".into(),
            path: Some("a".into()),
        })
    );
    assert_eq!(manifest.managed[0].symlink(), Some(Symlink::Keep));
    assert_eq!(
        manifest.managed[1].source(),
        Some(&Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "main".into(),
            path: Some("b".into()),
        })
    );
    assert_eq!(manifest.managed[1].symlink(), Some(Symlink::Keep));
    assert_eq!(
        manifest.managed[2].source(),
        Some(&Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "main".into(),
            path: Some("c".into()),
        })
    );
    assert_eq!(manifest.managed[2].symlink(), Some(Symlink::Follow));
    assert_eq!(
        manifest.managed[3].source(),
        Some(&Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "main".into(),
            path: None,
        })
    );
    assert_eq!(manifest.managed[3].symlink(), Some(Symlink::Keep));
    assert_eq!(manifest.managed[4].symlink(), Some(Symlink::Keep));
}

#[test]
fn rejects_invalid_symlink_values() {
    for bad in ["true", "false", "123", "\"\"", "preserve"] {
        let err = Manifest::parse(&format!(
            "{KIND}\"a\": (catasto.git(\"https://example.com/v.git\", rev: v1, path: \"a\"), symlink: {bad})\n"
        ))
        .unwrap_err();
        assert!(
            err.to_string().contains("symlink"),
            "expected error mentioning symlink, got: {err}"
        );
    }
}

#[test]
fn rejects_symlink_and_submodule_on_local_source() {
    for sym in ["follow", "keep"] {
        let err = Manifest::parse(&format!(
            "{KIND}\"a\": (catasto.local(\"src/a\"), symlink: {sym})\n"
        ))
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("`catasto.local` cannot specify `symlink`"),
            "{err}"
        );
    }
    for sub in ["true", "false"] {
        let err = Manifest::parse(&format!(
            "{KIND}\"a\": (catasto.local(\"src/a\"), submodule: {sub})\n"
        ))
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("`catasto.local` cannot specify `submodule`"),
            "{err}"
        );
    }
}

#[test]
fn git_source_rejects_unknown_arguments_and_root_in_entry() {
    let err = Manifest::parse(&format!(
        "{KIND}\"a\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"a\", extra: x)\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("has no argument `extra`"), "{err}");

    let err = Manifest::parse(&format!(
        "{KIND}\"a\": catasto.git(\"https://example.com/v.git\", rev: v1, root: sub, path: \"a\")\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("has no argument `root`"), "{err}");

    let err = Manifest::parse(&format!(
        "{KIND}\"a\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"a\", symlink: keep)\n"
    ))
    .unwrap_err();
    assert!(
        err.to_string().contains("has no argument `symlink`"),
        "{err}"
    );

    let err = Manifest::parse(&format!(
        "{KIND}\"a\": catasto.git(\"https://example.com/v.git\", rev: v1, submodule: true)\n"
    ))
    .unwrap_err();
    assert!(
        err.to_string().contains("has no argument `submodule`"),
        "{err}"
    );
}

#[test]
fn a_rev_that_reads_as_a_number_must_be_quoted() {
    let text = |rev: &str| format!("{KIND}\"a\": catasto.git(\"../v\", rev: {rev}, path: \"p\")\n");

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
        let text = format!("{KIND}\"a\": catasto.git(\"{url}\", rev: v, path: \"p\")\n");
        assert!(Manifest::parse(&text).is_err(), "accepted {url}");
    }
}

#[test]
fn accepts_https_and_relative_local_git_urls() {
    for url in ["https://github.com/tomet-lang/twrit.git", "../twrit"] {
        let text = format!("{KIND}\"a\": catasto.git(\"{url}\", rev: v, path: \"p\")\n");
        assert!(Manifest::parse(&text).is_ok(), "rejected {url}");
    }
}

#[test]
fn rejects_malformed_git_sources() {
    for call in [
        "catasto.git(\"https://example.com/u.git\", rev: \"v\", symlink: keep)",
        "catasto.git(\"https://example.com/u.git\", rev: \"v\", submodule: yes)",
        "(catasto.git(\"https://example.com/u.git\", rev: \"v\"), submodule: yes)",
        "catasto.git(rev: \"v\", path: \"p\")",
        "catasto.git(\"-u\", rev: \"v\", path: \"p\")",
        "catasto.git(\"u\", rev: \"-v\", path: \"p\")",
        "catasto.git(\"\", rev: \"v\", path: \"p\")",
        "catasto.git(\"../u\", rev: \"v\", path: \"../p\")",
        "catasto.git(\"../u\", rev: \"v\", path: \"/p\")",
        "catasto.git()",
        "catasto.repo(\"https://example.com/u.git\")",
    ] {
        let text = format!("{KIND}\"a\": {call}\n");
        assert!(Manifest::parse(&text).is_err(), "accepted {call}");
    }
}

#[test]
fn rejects_paths_that_escape_the_workspace() {
    for bad in ["../a", "/etc/a", "a/../../b", ""] {
        let text = format!("{KIND}\"{bad}\": catasto.local(\"s\")\n");
        assert!(Manifest::parse(&text).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn rejects_duplicate_paths() {
    let text = format!("{KIND}\"a\": catasto.local(\"s\")\n\"a\": catasto.local(\"t\")\n");
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
            path: Some("dir/w.tmt".into()),
        },
        Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "v1".into(),
            path: None,
        },
    ] {
        let json = serde_json::to_string(&source).unwrap();
        assert_eq!(serde_json::from_str::<Source>(&json).unwrap(), source);
    }

    // Verify that path: None does not emit `"path"` in JSON
    let root_git = Source::Git {
        git: "https://example.com/v.git".into(),
        rev: "v1".into(),
        path: None,
    };
    let json_root = serde_json::to_string(&root_git).unwrap();
    assert!(
        !json_root.contains("\"path\""),
        "root git source emitted path in JSON: {json_root}"
    );
}

#[test]
fn upstream_with_symlink_keep_resolves_identically_to_full_form() {
    let manifest = Manifest::parse(&format!(
        "{VOCABULARIES}\n\
         \"a.tmt\": (catasto.upstream(vocabularies, path: \"a.tmt\"), symlink: keep)\n\
         \"b.tmt\": (catasto.git(\"https://example.com/v.git\", rev: main, path: \"a.tmt\"), symlink: keep)\n"
    ))
    .unwrap();

    assert_eq!(manifest.managed[0].source(), manifest.managed[1].source());
    assert_eq!(manifest.managed[0].symlink(), Some(Symlink::Keep));
    assert_eq!(manifest.managed[1].symlink(), Some(Symlink::Keep));
    assert_eq!(
        serde_json::to_string(manifest.managed[0].source().unwrap()).unwrap(),
        serde_json::to_string(manifest.managed[1].source().unwrap()).unwrap()
    );
}

#[test]
fn parses_submodule_entries_with_and_without_path_and_via_upstream() {
    let manifest = Manifest::parse(&format!(
        "{KIND}\
         ::upstream(sisho_up: catasto.git(\"https://example.com/sisho.git\", rev: v1, path: crates))\n\
         \"tools/sisho\": (catasto.git(\"https://example.com/sisho.git\", rev: v1), submodule: true)\n\
         \"vendor/lib\": (catasto.git(\"../lib\", path: \"sub/pkg\"), submodule: true)\n\
         \"vendor/plain\": (catasto.git(\"../plain\"), submodule: false)\n\
         \"vendor/from_up\": (catasto.upstream(sisho_up, path: \"sisho\"), submodule: true)\n"
    ))
    .unwrap();

    assert!(manifest.managed[0].is_submodule());
    assert!(!manifest.managed[0].is_file());
    assert_eq!(manifest.managed[0].source(), None);
    assert_eq!(manifest.managed[0].symlink(), None);
    assert_eq!(manifest.managed[0].upstream(), None);
    assert_eq!(
        manifest.managed[0].submodule(),
        Some(&Submodule {
            git: "https://example.com/sisho.git".into(),
            rev: "v1".into(),
            path: None,
        })
    );
    assert_eq!(
        manifest.managed[1].submodule(),
        Some(&Submodule {
            git: "../lib".into(),
            rev: "main".into(),
            path: Some("sub/pkg".into()),
        })
    );
    assert!(manifest.managed[2].is_file());
    assert!(!manifest.managed[2].is_submodule());
    assert_eq!(manifest.managed[2].symlink(), Some(Symlink::Follow));
    assert_eq!(
        manifest.managed[3].submodule(),
        Some(&Submodule {
            git: "https://example.com/sisho.git".into(),
            rev: "v1".into(),
            path: Some("crates/sisho".into()),
        })
    );
    assert_eq!(manifest.managed[3].symlink(), None);
    assert_eq!(
        manifest.managed[3].upstream(),
        Some(&UpstreamRef {
            name: "sisho_up".into(),
            path: Some("sisho".into()),
        })
    );
    assert_eq!(
        manifest.submodules().collect::<Vec<_>>(),
        vec![
            (
                Path::new("tools/sisho"),
                &Submodule {
                    git: "https://example.com/sisho.git".into(),
                    rev: "v1".into(),
                    path: None,
                }
            ),
            (
                Path::new("vendor/lib"),
                &Submodule {
                    git: "../lib".into(),
                    rev: "main".into(),
                    path: Some("sub/pkg".into()),
                }
            ),
            (
                Path::new("vendor/from_up"),
                &Submodule {
                    git: "https://example.com/sisho.git".into(),
                    rev: "v1".into(),
                    path: Some("crates/sisho".into()),
                }
            ),
        ]
    );
}

#[test]
fn rejects_executable_and_symlink_on_submodule_entry() {
    for flag in ["true", "false"] {
        let err = Manifest::parse(&format!(
            "{KIND}\"tools/sisho\": (catasto.git(\"https://example.com/sisho.git\"), submodule: true, executable: {flag})\n"
        ))
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("submodule entry (`submodule: true`) cannot declare `executable`"),
            "{err}"
        );
    }

    for sym in ["follow", "keep"] {
        let err = Manifest::parse(&format!(
            "{KIND}\"tools/sisho\": (catasto.git(\"https://example.com/sisho.git\", path: \"a\"), symlink: {sym}, submodule: true)\n"
        ))
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("submodule entry (`submodule: true`) cannot specify `symlink`"),
            "{err}"
        );
    }
}

#[test]
fn rejects_entries_inside_a_declared_submodule() {
    let err = Manifest::parse(&format!(
        "{KIND}\
         \"tools/sisho\": (catasto.git(\"https://example.com/sisho.git\"), submodule: true)\n\
         \"tools/sisho/README.md\": catasto.local(\"../README.md\")\n"
    ))
    .unwrap_err();
    assert!(
        err.to_string().contains("cannot be inside declared submodule"),
        "{err}"
    );

    // Order independent: file declared before submodule is also rejected.
    let err = Manifest::parse(&format!(
        "{KIND}\
         \"tools/sisho/sub/a\": catasto.local(\"../a\")\n\
         \"./tools/sisho\": (catasto.git(\"https://example.com/sisho.git\"), submodule: true)\n"
    ))
    .unwrap_err();
    assert!(
        err.to_string().contains("cannot be inside declared submodule"),
        "{err}"
    );

    // Sibling prefix like tools/sisho-extra is NOT inside tools/sisho.
    assert!(
        Manifest::parse(&format!(
            "{KIND}\
             \"tools/sisho\": (catasto.git(\"https://example.com/sisho.git\"), submodule: true)\n\
             \"tools/sisho-extra/file\": catasto.local(\"../a\")\n"
        ))
        .is_ok()
    );
}

#[test]
fn parses_zone_declarations_with_single_and_tuple_patterns() {
    let manifest = Manifest::parse(&format!(
        "{KIND}\
         ::zone(\n\
           \".\": (\n\
             require: \"Cargo.toml\"\n\
             ignore: (\".git\", \".gitignore\", \"target\")\n\
           )\n\
           \"crates/<?kebab(crate)>/src\": (\n\
             require: (\"lib.rs\")\n\
             allow: (\"<?snake>.rs\", \"mod.rs\")\n\
             deny: (\"<?>.bak\")\n\
             ignore: \"generated\"\n\
           )\n\
         )\n"
    ))
    .unwrap();

    assert_eq!(manifest.zones.len(), 2);

    let root = &manifest.zones[0];
    assert_eq!(root.target.source, ".");
    assert!(root.target.is_match("."));
    assert!(!root.target.is_match("crates"));
    assert!(!root.recursive);
    assert_eq!(root.entries.require.len(), 1);
    assert!(root.entries.require[0].is_match("Cargo.toml"));
    assert!(root.entries.allow.is_none());
    assert!(root.entries.deny.is_empty());
    assert!(root.files.is_empty());
    assert!(root.dirs.is_empty());
    assert_eq!(root.ignore.len(), 3);
    assert!(root.ignore[1].is_match(".gitignore"));

    let src = &manifest.zones[1];
    assert_eq!(src.target.source, "crates/<?kebab(crate)>/src");
    assert!(src.target.is_match("crates/catasto-core/src"));
    assert!(!src.target.is_match("crates/BadName/src"));
    assert!(!src.target.is_match("crates/a/b/src"));
    assert!(!src.recursive);
    assert!(src.entries.require[0].is_match("lib.rs"));
    let allow = src.entries.allow.as_ref().unwrap();
    assert_eq!(allow.len(), 2);
    assert!(allow[0].is_match("check_zone.rs"));
    assert!(!allow[0].is_match("BadFile.rs"));
    assert!(allow[1].is_match("mod.rs"));
    assert!(src.entries.deny[0].is_match("old.bak"));
    assert!(!src.entries.deny[0].is_match("a/b.bak"));
    assert!(src.ignore[0].is_match("generated"));
}

#[test]
fn allows_multiple_zone_declarations_and_zone_only_manifest() {
    let manifest = Manifest::parse(
        "::zone(\".\": (require: \"README.md\"))\n\
         ::zone(\"docs\": (allow: (\"<?kebab>.md\")))\n",
    )
    .unwrap();
    assert_eq!(manifest.zones.len(), 2);
    assert_eq!(manifest.zones[0].target.source, ".");
    assert_eq!(manifest.zones[1].target.source, "docs");
    assert!(manifest.managed.is_empty());
}

#[test]
fn zone_supports_empty_allow_tuple_to_forbid_all_non_ignored_entries() {
    let manifest = Manifest::parse(&format!(
        "{KIND}::zone(\"empty\": (allow: (), ignore: (\".keep\")))\n"
    ))
    .unwrap();
    assert_eq!(manifest.zones[0].entries.allow.as_ref().unwrap().len(), 0);
    assert_eq!(manifest.zones[0].ignore.len(), 1);
}

#[test]
fn zone_rejects_braces_prose_and_unnamed_entries() {
    for bad in [
        "::zone(){ \"src\": (require: \"lib.rs\") }\n",
        "::zone(\"src\": (require: \"lib.rs\")){ extra: true }\n",
        "::zone(\"src\": (require: \"lib.rs\"))[prose]\n",
        "::zone\n",
        "::zone(\"src\")\n",
        "::zone(\"src\": (require: \"lib.rs\"), \"extra\")\n",
    ] {
        assert!(
            Manifest::parse(&format!("{KIND}{bad}")).is_err(),
            "accepted: {bad}"
        );
    }
}

#[test]
fn zone_rejects_duplicate_target_keys_across_declarations() {
    let err = Manifest::parse(&format!(
        "{KIND}\
         ::zone(\"crates/<?kebab>/src\": (require: \"lib.rs\"))\n\
         ::zone(\"crates/<?kebab>/src\": (deny: \"<?>.bak\"))\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("declared twice"), "{err}");

    let err = Manifest::parse(&format!(
        "{KIND}::zone(\"./src\": (require: \"lib.rs\"), \"src\": (deny: \"<?>.bak\"))\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("declared twice"), "{err}");
}

#[test]
fn zone_rejects_empty_absolute_and_escaping_target_paths() {
    let empty_err =
        Manifest::parse(&format!("{KIND}::zone(\"\": (require: \"a\"))\n")).unwrap_err();
    assert!(
        empty_err.to_string().contains("must not be empty"),
        "unexpected error for empty target: {empty_err}"
    );

    for bad in [
        "\"/etc\"",
        "\"/\"",
        "\"..\"",
        "\"../outside\"",
        "\"crates/../outside\"",
        "\"crates//src\"",
        "\"crates/./src\"",
    ] {
        let err = Manifest::parse(&format!("{KIND}::zone({bad}: (require: \"a\"))\n")).unwrap_err();
        assert!(
            err.to_string().contains("zone target path"),
            "unexpected error for {bad}: {err}"
        );
    }
}

#[test]
fn zone_rejects_unknown_or_duplicate_spec_keys_and_invalid_patterns() {
    for bad in [
        "::zone(\"src\": \"lib.rs\")\n",
        "::zone(\"src\": (\"lib.rs\"))\n",
        "::zone(\"src\": (unknown: \"lib.rs\"))\n",
        "::zone(\"src\": (require: \"a\", require: \"b\"))\n",
        "::zone(\"src\": (require: 123))\n",
        "::zone(\"src\": (require: (123)))\n",
        "::zone(\"src\": (require: (named: \"a\")))\n",
        "::zone(\"src\": (require: \"\"))\n",
        "::zone(\"src\": (require: (\"\")))\n",
        "::zone(\"src\": (require: \"<?unknown_matcher>\"))\n",
        "::zone(\"<?unknown_matcher>\": (require: \"a\"))\n",
        "::zone(\"src\": (recursive: \"true\"))\n",
        "::zone(\"src\": (recursive: 1))\n",
        "::zone(\"src\": (recursive: true, recursive: false))\n",
        "::zone(\"src\": (files: \"a.rs\"))\n",
        "::zone(\"src\": (files: (\"a.rs\")))\n",
        "::zone(\"src\": (files: (ignore: \"a.rs\")))\n",
        "::zone(\"src\": (files: (recursive: true)))\n",
        "::zone(\"src\": (files: (unknown: \"a.rs\")))\n",
        "::zone(\"src\": (files: (allow: \"a.rs\", allow: \"b.rs\")))\n",
        "::zone(\"src\": (dirs: \"sub\"))\n",
        "::zone(\"src\": (dirs: (\"sub\")))\n",
        "::zone(\"src\": (dirs: (ignore: \"sub\")))\n",
        "::zone(\"src\": (dirs: (files: (allow: \"a\"))))\n",
        "::zone(\"src\": (dirs: (unknown: \"sub\")))\n",
        "::zone(\"src\": (dirs: (require: \"a\", require: \"b\")))\n",
    ] {
        assert!(
            Manifest::parse(&format!("{KIND}{bad}")).is_err(),
            "accepted: {bad}"
        );
    }
}

#[test]
fn parses_zone_recursive_files_and_dirs() {
    let manifest = Manifest::parse(&format!(
        "{KIND}\
         ::zone(\n\
           \"nix\": (\n\
             recursive: true\n\
             files: (\n\
               require: \"default.nix\"\n\
               allow: (\"<?kebab>.nix\")\n\
               deny: (\"<?>.tmp\")\n\
             )\n\
             dirs: (\n\
               allow: (\"<?kebab>\")\n\
               deny: (\"secret\")\n\
             )\n\
             ignore: (\"result\")\n\
           )\n\
           \"docs\": (\n\
             recursive: false\n\
             dirs: (allow: ())\n\
           )\n\
         )\n"
    ))
    .unwrap();

    assert_eq!(manifest.zones.len(), 2);

    let nix = &manifest.zones[0];
    assert!(nix.recursive);
    assert!(nix.entries.is_empty());
    assert_eq!(nix.files.require.len(), 1);
    assert!(nix.files.require[0].is_match("default.nix"));
    assert_eq!(nix.files.allow.as_ref().unwrap().len(), 1);
    assert!(nix.files.allow.as_ref().unwrap()[0].is_match("catasto.nix"));
    assert_eq!(nix.files.deny.len(), 1);
    assert!(nix.files.deny[0].is_match("scratch.tmp"));
    assert!(nix.dirs.require.is_empty());
    assert_eq!(nix.dirs.allow.as_ref().unwrap().len(), 1);
    assert!(nix.dirs.allow.as_ref().unwrap()[0].is_match("pkgs"));
    assert_eq!(nix.dirs.deny.len(), 1);
    assert!(nix.dirs.deny[0].is_match("secret"));
    assert_eq!(nix.ignore.len(), 1);

    let docs = &manifest.zones[1];
    assert!(!docs.recursive);
    assert!(docs.files.is_empty());
    assert_eq!(docs.dirs.allow.as_ref().unwrap().len(), 0);
}
