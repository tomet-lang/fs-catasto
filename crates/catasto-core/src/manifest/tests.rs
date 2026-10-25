use std::path::PathBuf;

use super::*;

fn path_source(path: &str) -> Source {
    Source::Path(path.into())
}

#[test]
fn parses_managed_entries_in_the_order_written() {
    let manifest = Manifest::parse(
        r#"
            "b/writ.vocabulary.tmt": { source: "../vocabularies/writ.vocabulary.tmt" }
            "a/rust.vocabulary.tmt": { source: "../vocabularies/rust.vocabulary.tmt" }
            "#,
    )
    .unwrap();

    assert_eq!(
        manifest.managed,
        vec![
            Managed {
                path: "b/writ.vocabulary.tmt".into(),
                source: path_source("../vocabularies/writ.vocabulary.tmt"),
            },
            Managed {
                path: "a/rust.vocabulary.tmt".into(),
                source: path_source("../vocabularies/rust.vocabulary.tmt"),
            },
        ]
    );
}

#[test]
fn comments_and_unquoted_values_are_fine() {
    let manifest = Manifest::parse(
        "// shared vocabularies\n\
             \"a.tmt\": { source: ../v/a.tmt } // the writ one\n",
    )
    .unwrap();

    assert_eq!(manifest.managed[0].source, path_source("../v/a.tmt"));
}

#[test]
fn a_key_that_needs_no_quotes_may_be_bare() {
    let manifest = Manifest::parse("a.tmt: { source: s }").unwrap();
    assert_eq!(manifest.managed[0].path, PathBuf::from("a.tmt"));
}

#[test]
fn empty_text_is_an_empty_manifest() {
    assert_eq!(Manifest::parse("").unwrap(), Manifest::default());
    assert_eq!(Manifest::parse("\n  \n").unwrap(), Manifest::default());
}

#[test]
fn a_manifest_with_only_comments_is_empty() {
    assert_eq!(
        Manifest::parse("// nothing managed yet\n").unwrap(),
        Manifest::default()
    );
}

const VOCABULARIES: &str =
    "::upstreams{\n  vocabularies: { git: \"https://example.com/v.git\", rev: main }\n}\n";

#[test]
fn an_upstream_reads_as_the_entry_written_in_full() {
    let named = Manifest::parse(&format!(
        "{VOCABULARIES}\n\"a.tmt\": {{ source: {{ upstream: vocabularies, path: \"x/a.tmt\" }} }}\n"
    ))
    .unwrap();
    let written = Manifest::parse(
            "\"a.tmt\": { source: { git: \"https://example.com/v.git\", rev: main, path: \"x/a.tmt\" } }",
        )
        .unwrap();
    assert_eq!(named, written);
}

#[test]
fn a_root_is_joined_in_front_of_the_path() {
    let rooted = Manifest::parse(
        "::upstreams{ v: { git: \"https://example.com/v.git\", rev: main, root: vocabularies } }\n\
             \"a.tmt\": { source: { upstream: v, path: \"rust/a.tmt\" } }\n",
    )
    .unwrap();
    let written = Manifest::parse(
            "\"a.tmt\": { source: { git: \"https://example.com/v.git\", rev: main, path: \"vocabularies/rust/a.tmt\" } }",
        )
        .unwrap();
    assert_eq!(rooted, written);
}

#[test]
fn a_root_is_checked_where_it_is_declared() {
    for root in ["/etc", "../outside", "a/../b", "\"\""] {
        let err = Manifest::parse(&format!(
            "::upstreams{{ v: {{ git: \"https://example.com/v.git\", rev: main, root: {root} }} }}"
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
    let err = Manifest::parse(
        "::upstreams{ v: { git: \"https://example.com/v.git\", rev: main, root: vocabularies } }\n\
             \"a.tmt\": { source: { upstream: v, path: \"../secret.tmt\" } }\n",
    )
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
             \"b.tmt\": {{ source: {{ upstream: vocabularies, path: \"b.tmt\" }} }}\n\
             \"a.tmt\": {{ source: {{ upstream: vocabularies, path: \"a.tmt\" }} }}\n"
    ))
    .unwrap();
    let paths: Vec<_> = manifest.managed.iter().map(|m| m.path.clone()).collect();
    assert_eq!(paths, [PathBuf::from("b.tmt"), PathBuf::from("a.tmt")]);
    for managed in &manifest.managed {
        assert!(matches!(
            &managed.source,
            Source::Git { git, rev, .. } if git == "https://example.com/v.git" && rev == "main"
        ));
    }
}

#[test]
fn a_local_source_and_a_written_out_git_source_need_no_upstream() {
    let manifest = Manifest::parse(&format!(
            "{VOCABULARIES}\n\
             \"a.tmt\": {{ source: \"../a.tmt\" }}\n\
             \"b.tmt\": {{ source: {{ git: \"https://example.com/o.git\", rev: v1, path: \"b.tmt\" }} }}\n"
        ))
        .unwrap();
    assert_eq!(manifest.managed[0].source, path_source("../a.tmt"));
    assert!(matches!(manifest.managed[1].source, Source::Git { .. }));
}

#[test]
fn an_unknown_upstream_lists_the_declared_ones() {
    let err = Manifest::parse(&format!(
        "{VOCABULARIES}\n\"a.tmt\": {{ source: {{ upstream: vocabularis, path: \"a.tmt\" }} }}\n"
    ))
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "a.tmt: unknown upstream `vocabularis` (declared: vocabularies)"
    );
}

#[test]
fn an_upstream_with_none_declared_says_where_to_declare() {
    let err =
        Manifest::parse("\"a.tmt\": { source: { upstream: v, path: \"a.tmt\" } }").unwrap_err();
    assert!(err.to_string().contains("add `::upstreams{ … }`"), "{err}");
}

#[test]
fn an_upstream_does_not_mix_with_git_or_rev() {
    for extra in ["git: \"https://example.com/o.git\"", "rev: v2"] {
        let err = Manifest::parse(&format!(
                "{VOCABULARIES}\n\"a.tmt\": {{ source: {{ upstream: vocabularies, {extra}, path: \"a.tmt\" }} }}\n"
            ))
            .unwrap_err();
        assert!(err.to_string().contains("`upstream` already says"), "{err}");
    }
}

#[test]
fn an_upstream_needs_a_path_like_a_git_source_does() {
    let err = Manifest::parse(&format!(
        "{VOCABULARIES}\n\"a.tmt\": {{ source: {{ upstream: vocabularies }} }}\n"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("missing field `path`"), "{err}");
}

#[test]
fn an_upstream_is_checked_where_it_is_declared() {
    let err = Manifest::parse("::upstreams{ v: { git: \"git@example.com:o/v.git\", rev: main } }")
        .unwrap_err();
    assert!(err.to_string().starts_with("upstream `v`: "), "{err}");

    let err = Manifest::parse("::upstreams{ v: { git: \"https://example.com/v.git\", rev: 1.2 } }")
        .unwrap_err();
    assert!(err.to_string().contains("write it in quotes"), "{err}");

    let err =
        Manifest::parse("::upstreams{ v: { git: \"https://example.com/v.git\" } }").unwrap_err();
    assert!(err.to_string().contains("missing field `rev`"), "{err}");
}

#[test]
fn the_upstreams_declaration_is_one_map_and_the_only_declaration() {
    let err = Manifest::parse(&format!("{VOCABULARIES}{VOCABULARIES}")).unwrap_err();
    assert!(err.to_string().contains("declared twice"), "{err}");

    let err = Manifest::parse("::upstreams(v)").unwrap_err();
    assert!(err.to_string().contains("takes a map"), "{err}");

    let err = Manifest::parse("::upstreams").unwrap_err();
    assert!(err.to_string().contains("needs a map"), "{err}");

    let err = Manifest::parse("::kind(catalog)").unwrap_err();
    assert!(
        err.to_string().contains("unknown declaration `::kind`"),
        "{err}"
    );
}

#[test]
fn an_entry_is_always_a_map() {
    let err = Manifest::parse("\"a\": \"s\"").unwrap_err();
    assert!(err.to_string().contains("a: invalid type"), "{err}");
    assert!(err.to_string().contains("a map with a `source`"), "{err}");
}

#[test]
fn an_entry_needs_a_source() {
    let err = Manifest::parse("\"a.tmt\": {}").unwrap_err();
    assert_eq!(err.to_string(), "\"a.tmt\": missing field `source`");
}

#[test]
fn rejects_unknown_fields() {
    let err = Manifest::parse("\"a\": { source: \"b\", owner: x }");
    assert!(err.is_err());
}

#[test]
fn rejects_unknown_source_kind() {
    let err = Manifest::parse("\"a\": { source: { ftp: \"b\" } }");
    assert!(err.is_err());
}

#[test]
fn rejects_an_empty_source_path() {
    assert!(Manifest::parse("\"a\": { source: \"\" }").is_err());
}

#[test]
fn parses_a_git_source() {
    let manifest = Manifest::parse(
        "\"a\": { source: { git: \"https://example.com/v.git\", rev: v1, path: \"dir/w.tmt\" } }",
    )
    .unwrap();

    assert_eq!(
        manifest.managed[0].source,
        Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "v1".into(),
            path: "dir/w.tmt".into(),
            symlink: Symlink::Follow,
        }
    );
}

#[test]
fn parses_symlink_keep_in_git_source_and_upstream() {
    let manifest = Manifest::parse(&format!(
            "{VOCABULARIES}\n\
             \"a\": {{ source: {{ git: \"https://example.com/v.git\", rev: v1, path: \"a\", symlink: keep }} }}\n\
             \"b\": {{ source: {{ upstream: vocabularies, path: \"b\", symlink: keep }} }}\n"
        ))
        .unwrap();

    assert_eq!(
        manifest.managed[0].source,
        Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "v1".into(),
            path: "a".into(),
            symlink: Symlink::Keep,
        }
    );
    assert_eq!(
        manifest.managed[1].source,
        Source::Git {
            git: "https://example.com/v.git".into(),
            rev: "main".into(),
            path: "b".into(),
            symlink: Symlink::Keep,
        }
    );
}

#[test]
fn rejects_invalid_symlink_values() {
    for bad in ["follow", "true", "false", "123", "\"\""] {
        let err = Manifest::parse(&format!(
                "\"a\": {{ source: {{ git: \"https://example.com/v.git\", rev: v1, path: \"a\", symlink: {bad} }} }}"
            ))
            .unwrap_err();
        assert!(
            err.to_string().contains("keep"),
            "expected error mentioning keep, got: {err}"
        );
    }
}

#[test]
fn git_source_rejects_unknown_fields_even_with_symlink() {
    let err = Manifest::parse(
            "\"a\": { source: { git: \"https://example.com/v.git\", rev: v1, path: \"a\", symlink: keep, extra: x } }",
        )
        .unwrap_err();
    assert!(err.to_string().contains("unknown field `extra`"), "{err}");
}

#[test]
fn a_rev_that_reads_as_a_number_must_be_quoted() {
    let text =
        |rev: &str| format!("\"a\": {{ source: {{ git: \"../v\", rev: {rev}, path: \"p\" }} }}");

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
        let text = format!("\"a\": {{ source: {{ git: \"{url}\", rev: v, path: \"p\" }} }}");
        assert!(Manifest::parse(&text).is_err(), "accepted {url}");
    }
}

#[test]
fn accepts_https_and_relative_local_git_urls() {
    for url in ["https://github.com/tomet-lang/twrit.git", "../twrit"] {
        let text = format!("\"a\": {{ source: {{ git: \"{url}\", rev: v, path: \"p\" }} }}");
        assert!(Manifest::parse(&text).is_ok(), "rejected {url}");
    }
}

#[test]
fn rejects_malformed_git_sources() {
    for source in [
        "{ git: \"u\", path: \"p\" }",
        "{ git: \"u\", rev: \"v\" }",
        "{ rev: \"v\", path: \"p\" }",
        "{ git: \"-u\", rev: \"v\", path: \"p\" }",
        "{ git: \"u\", rev: \"-v\", path: \"p\" }",
        "{ git: \"\", rev: \"v\", path: \"p\" }",
        "{ git: \"../u\", rev: \"v\", path: \"../p\" }",
        "{ git: \"../u\", rev: \"v\", path: \"/p\" }",
        "{ path: \"p\" }",
        "{}",
    ] {
        let text = format!("\"a\": {{ source: {source} }}");
        assert!(Manifest::parse(&text).is_err(), "accepted {source}");
    }
}

#[test]
fn rejects_paths_that_escape_the_workspace() {
    for bad in ["../a", "/etc/a", "a/../../b", ""] {
        let text = format!("\"{bad}\": {{ source: \"s\" }}");
        assert!(Manifest::parse(&text).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn rejects_duplicate_paths() {
    let text = "\"a\": { source: \"s\" }\n\"a\": { source: \"t\" }\n";
    let err = Manifest::parse(text).unwrap_err();
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

    // Verify that Symlink::Follow does not emit `"symlink"` in JSON (unchanged byte-for-byte)
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
             \"a.tmt\": {{ source: {{ upstream: vocabularies, path: \"a.tmt\", symlink: keep }} }}\n\
             \"b.tmt\": {{ source: {{ git: \"https://example.com/v.git\", rev: main, path: \"a.tmt\", symlink: keep }} }}\n"
        ))
        .unwrap();

    assert_eq!(manifest.managed[0].source, manifest.managed[1].source);
    assert_eq!(
        serde_json::to_string(&manifest.managed[0].source).unwrap(),
        serde_json::to_string(&manifest.managed[1].source).unwrap()
    );
}
