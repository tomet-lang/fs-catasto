use super::*;
use crate::Manifest;

const VOCAB: &str = "\
// The files catasto manages in this workspace.
::upstreams{
  vocabularies: { git: \"https://example.com/v.git\", rev: main, root: vocabularies }
}

// the rust one
\"schema/rust.tmt\": {
  source: { upstream: vocabularies, path: \"rust/rust.tmt\" }
}
\"schema/writ.tmt\": {
  source: { upstream: vocabularies, path: \"writ/writ.tmt\" }
}
";

fn up(name: &str, path: &str) -> SourceText {
    SourceText::Upstream {
        name: name.into(),
        path: path.into(),
        keep_symlink: false,
    }
}

#[test]
fn scanning_finds_the_declaration_and_the_entries() {
    let layout = scan(VOCAB).unwrap();
    assert_eq!(layout.declarations.len(), 1);
    assert_eq!(layout.declarations[0].name, "upstreams");
    let keys: Vec<_> = layout.entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(keys, ["schema/rust.tmt", "schema/writ.tmt"]);
}

#[test]
fn an_entry_is_added_after_the_last_and_the_rest_is_untouched() {
    let text = add_entry(VOCAB, "schema/ui.tmt", &up("vocabularies", "ui/ui.tmt")).unwrap();
    assert!(text.starts_with(VOCAB), "{text}");
    assert!(text.ends_with(
        "\"schema/ui.tmt\": {\n  source: { upstream: vocabularies, path: \"ui/ui.tmt\" }\n}\n"
    ));
    assert!(Manifest::parse(&text).is_ok());
}

#[test]
fn the_first_entry_goes_below_a_blank_line() {
    let text = add_entry(
        "// nothing yet\n",
        "a.tmt",
        &SourceText::Local {
            path: "../a.tmt".into(),
        },
    )
    .unwrap();
    assert_eq!(
        text,
        "// nothing yet\n\n\"a.tmt\": {\n  source: \"../a.tmt\"\n}\n"
    );
    assert!(Manifest::parse(&text).is_ok());
}

#[test]
fn an_entry_is_not_added_twice() {
    let err = add_entry(VOCAB, "schema/rust.tmt", &up("v", "p")).unwrap_err();
    assert!(err.to_string().contains("already declared"), "{err}");
}

#[test]
fn a_removed_entry_takes_its_comment_lines_and_nothing_else() {
    let text = remove_entry(VOCAB, "schema/rust.tmt").unwrap();
    assert!(!text.contains("rust"), "{text}");
    assert!(!text.contains("// the rust one"), "{text}");
    assert!(text.contains("// The files catasto manages"));
    assert!(text.contains("schema/writ.tmt"));
    assert!(Manifest::parse(&text).is_ok());
}

#[test]
fn the_comment_that_opens_the_file_is_not_an_entrys() {
    let text = "// about this file\n\"a.tmt\": { source: \"x\" }\n";
    assert_eq!(remove_entry(text, "a.tmt").unwrap(), "// about this file\n");
}

#[test]
fn removing_what_is_not_there_says_so() {
    let err = remove_entry(VOCAB, "nope.tmt").unwrap_err();
    assert!(err.to_string().contains("not declared"), "{err}");
}

#[test]
fn a_value_with_braces_and_strings_inside_is_read_whole() {
    let text = "\"a\": {\n  source: { git: \"https://x/}.git\", rev: main, path: \"p // q\" } // trailing\n}\n\"b\": { source: \"y\" }\n";
    let layout = scan(text).unwrap();
    assert_eq!(layout.entries.len(), 2);
    assert_eq!(layout.entries[1].key, "b");
    let after = remove_entry(text, "a").unwrap();
    assert_eq!(after, "\"b\": { source: \"y\" }\n");
}

#[test]
fn an_upstream_goes_above_the_closing_brace_of_a_block_on_lines_of_its_own() {
    let text = add_upstream(
        VOCAB,
        "shared",
        &UpstreamText {
            git: "https://example.com/s.git".into(),
            rev: "v1".into(),
            root: None,
        },
    )
    .unwrap();
    assert!(text.contains(
            "  vocabularies: { git: \"https://example.com/v.git\", rev: main, root: vocabularies }\n  shared: { git: \"https://example.com/s.git\", rev: v1 }\n}\n"
        ), "{text}");
    assert!(Manifest::parse(&text).is_ok());
}

#[test]
fn a_block_on_one_line_is_written_out_when_it_gains_a_place() {
    let one = "::upstreams{ a: { git: \"https://x/a.git\", rev: main } }\n\"f\": { source: { upstream: a, path: \"p\" } }\n";
    let text = add_upstream(
        one,
        "b",
        &UpstreamText {
            git: "https://x/b.git".into(),
            rev: "main".into(),
            root: Some("dir/sub".into()),
        },
    )
    .unwrap();
    assert!(
            text.starts_with(
                "::upstreams{\n  a: { git: \"https://x/a.git\", rev: main }\n  b: { git: \"https://x/b.git\", rev: main, root: dir/sub }\n}\n"
            ),
            "{text}"
        );
    assert!(Manifest::parse(&text).is_ok(), "{text}");
}

#[test]
fn the_first_upstream_creates_the_declaration_above_the_entries() {
    let text = add_upstream(
        "// header\n\n\"f\": { source: \"x\" }\n",
        "v",
        &UpstreamText {
            git: "https://x/v.git".into(),
            rev: "main".into(),
            root: None,
        },
    )
    .unwrap();
    assert!(
        text.starts_with(
            "// header\n\n::upstreams{\n  v: { git: \"https://x/v.git\", rev: main }\n}\n\n\"f\""
        ),
        "{text}"
    );
    assert!(Manifest::parse(&text).is_ok());
}

#[test]
fn a_rev_that_reads_as_a_number_is_quoted() {
    let text = add_upstream(
        "",
        "v",
        &UpstreamText {
            git: "https://x/v.git".into(),
            rev: "1.2".into(),
            root: None,
        },
    )
    .unwrap();
    assert!(text.contains("rev: \"1.2\""), "{text}");
    assert!(Manifest::parse(&text).is_ok());
}

#[test]
fn an_upstream_is_removed_and_the_declaration_goes_with_the_last() {
    let two = add_upstream(
        VOCAB,
        "shared",
        &UpstreamText {
            git: "https://example.com/s.git".into(),
            rev: "main".into(),
            root: None,
        },
    )
    .unwrap();
    let one = remove_upstream(&two, "shared").unwrap();
    assert_eq!(one, VOCAB);

    let none = remove_upstream("::upstreams{\n  a: { git: \"https://x/a.git\", rev: main }\n}\n\n\"f\": { source: \"x\" }\n", "a").unwrap();
    assert_eq!(none, "\"f\": { source: \"x\" }\n");
}

#[test]
fn who_uses_an_upstream_is_found_in_the_entries() {
    assert_eq!(
        upstream_is_used(VOCAB, "vocabularies").unwrap(),
        ["schema/rust.tmt", "schema/writ.tmt"]
    );
    assert!(upstream_is_used(VOCAB, "other").unwrap().is_empty());
}

#[test]
fn a_name_with_punctuation_is_refused() {
    let err = add_upstream(
        "",
        "a b",
        &UpstreamText {
            git: "https://x".into(),
            rev: "main".into(),
            root: None,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("upstream name"), "{err}");
}

#[test]
fn entry_text_with_and_without_keep_symlink() {
    let upstream_follow = SourceText::Upstream {
        name: "commonfiles".into(),
        path: "CLAUDE.md".into(),
        keep_symlink: false,
    };
    let upstream_keep = SourceText::Upstream {
        name: "commonfiles".into(),
        path: "CLAUDE.md".into(),
        keep_symlink: true,
    };
    assert_eq!(
        entry_text("CLAUDE.md", &upstream_follow),
        "\"CLAUDE.md\": {\n  source: { upstream: commonfiles, path: \"CLAUDE.md\" }\n}\n"
    );
    assert_eq!(
        entry_text("CLAUDE.md", &upstream_keep),
        "\"CLAUDE.md\": {\n  source: { upstream: commonfiles, path: \"CLAUDE.md\", symlink: keep }\n}\n"
    );

    let git_follow = SourceText::Git {
        git: "https://example.com/repo.git".into(),
        rev: "main".into(),
        path: "bin/tool".into(),
        keep_symlink: false,
    };
    let git_keep = SourceText::Git {
        git: "https://example.com/repo.git".into(),
        rev: "main".into(),
        path: "bin/tool".into(),
        keep_symlink: true,
    };
    assert_eq!(
        entry_text("bin/tool", &git_follow),
        "\"bin/tool\": {\n  source: { git: \"https://example.com/repo.git\", rev: main, path: \"bin/tool\" }\n}\n"
    );
    assert_eq!(
        entry_text("bin/tool", &git_keep),
        "\"bin/tool\": {\n  source: { git: \"https://example.com/repo.git\", rev: main, path: \"bin/tool\", symlink: keep }\n}\n"
    );

    // Verify added entry with symlink: keep parses cleanly
    let text = add_entry(VOCAB, "link.tmt", &SourceText::Upstream {
        name: "vocabularies".into(),
        path: "rust/rust.tmt".into(),
        keep_symlink: true,
    }).unwrap();
    assert!(Manifest::parse(&text).is_ok());
}
