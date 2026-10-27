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

#[test]
fn add_or_replace_entry_keeps_position_comment_and_extra_fields() {
    let manifest = "\
// A comment before the entries
\"first.tmt\": {
  source: \"old/first.tmt\"
}

// A comment on the target entry
\"target.tmt\": {
  source: \"old/target.tmt\",
  extra: \"preserve-me\"
}

// A comment on the third entry
\"third.tmt\": {
  source: \"old/third.tmt\"
}
";

    let new_source = SourceText::Upstream {
        name: "vocabularies".into(),
        path: "new/target.tmt".into(),
        keep_symlink: false,
    };

    // 1. without force: fails with AlreadyDeclared
    let err = add_or_replace_entry(manifest, "target.tmt", &new_source, false).unwrap_err();
    let already = err.downcast_ref::<AlreadyDeclared>().expect("downcast to AlreadyDeclared");
    assert_eq!(already.dest, "target.tmt");
    assert_eq!(
        err.to_string(),
        "target.tmt is already declared (use --force to replace its source)"
    );

    // 2. with force: replaces source and keeps position, comment and extra field
    let (replaced_text, outcome) =
        add_or_replace_entry(manifest, "target.tmt", &new_source, true).unwrap();
    assert_eq!(outcome, AddOutcome::Replaced);
    assert_eq!(
        replaced_text,
        "\
// A comment before the entries
\"first.tmt\": {
  source: \"old/first.tmt\"
}

// A comment on the target entry
\"target.tmt\": {
  source: { upstream: vocabularies, path: \"new/target.tmt\" },
  extra: \"preserve-me\"
}

// A comment on the third entry
\"third.tmt\": {
  source: \"old/third.tmt\"
}
"
    );

    // 3. replacing with an equal source returns the same text (Unchanged)
    let (unchanged_text, outcome) =
        add_or_replace_entry(&replaced_text, "target.tmt", &new_source, true).unwrap();
    assert_eq!(outcome, AddOutcome::Unchanged);
    assert_eq!(unchanged_text, replaced_text);
}

#[test]
fn add_or_replace_upstream_multiline_block() {
    let manifest = "\
::upstreams{
  // comment above first
  first: { git: \"https://example.com/first.git\", rev: v1 }
  // comment above middle
  middle: { git: \"https://example.com/middle.git\", rev: v1 }
  // comment above last
  last: { git: \"https://example.com/last.git\", rev: v1 }
}

\"a.tmt\": { source: { upstream: middle, path: \"a.tmt\" } }
";

    let new_middle = UpstreamText {
        git: "https://example.com/new-middle.git".into(),
        rev: "v2".into(),
        root: Some("subdir".into()),
    };

    // 1. without force: fails with AlreadyDeclared
    let err = add_or_replace_upstream(manifest, "middle", &new_middle, false).unwrap_err();
    let already = err
        .downcast_ref::<AlreadyDeclared>()
        .expect("downcast to AlreadyDeclared");
    assert_eq!(already.dest, "middle");
    assert_eq!(already.kind, DeclaredKind::Upstream);
    assert_eq!(
        err.to_string(),
        "upstream `middle` is already declared (use --force to replace it)"
    );

    // 2. with force on middle: replaces definition, keeps comment and position
    let (replaced_middle, outcome) =
        add_or_replace_upstream(manifest, "middle", &new_middle, true).unwrap();
    assert_eq!(outcome, AddOutcome::Replaced);
    assert_eq!(
        replaced_middle,
        "\
::upstreams{
  // comment above first
  first: { git: \"https://example.com/first.git\", rev: v1 }
  // comment above middle
  middle: { git: \"https://example.com/new-middle.git\", rev: v2, root: subdir }
  // comment above last
  last: { git: \"https://example.com/last.git\", rev: v1 }
}

\"a.tmt\": { source: { upstream: middle, path: \"a.tmt\" } }
"
    );
    assert!(Manifest::parse(&replaced_middle).is_ok());

    // 3. identical definition returns same text (Unchanged)
    let (unchanged, outcome) =
        add_or_replace_upstream(&replaced_middle, "middle", &new_middle, true).unwrap();
    assert_eq!(outcome, AddOutcome::Unchanged);
    assert_eq!(unchanged, replaced_middle);

    // 4. replace first
    let new_first = UpstreamText {
        git: "https://example.com/new-first.git".into(),
        rev: "main".into(),
        root: None,
    };
    let (replaced_first, outcome) =
        add_or_replace_upstream(&replaced_middle, "first", &new_first, true).unwrap();
    assert_eq!(outcome, AddOutcome::Replaced);
    assert_eq!(
        replaced_first,
        "\
::upstreams{
  // comment above first
  first: { git: \"https://example.com/new-first.git\", rev: main }
  // comment above middle
  middle: { git: \"https://example.com/new-middle.git\", rev: v2, root: subdir }
  // comment above last
  last: { git: \"https://example.com/last.git\", rev: v1 }
}

\"a.tmt\": { source: { upstream: middle, path: \"a.tmt\" } }
"
    );
    assert!(Manifest::parse(&replaced_first).is_ok());

    // 5. replace last
    let new_last = UpstreamText {
        git: "https://example.com/new-last.git".into(),
        rev: "v3".into(),
        root: None,
    };
    let (replaced_last, outcome) =
        add_or_replace_upstream(&replaced_first, "last", &new_last, true).unwrap();
    assert_eq!(outcome, AddOutcome::Replaced);
    assert_eq!(
        replaced_last,
        "\
::upstreams{
  // comment above first
  first: { git: \"https://example.com/new-first.git\", rev: main }
  // comment above middle
  middle: { git: \"https://example.com/new-middle.git\", rev: v2, root: subdir }
  // comment above last
  last: { git: \"https://example.com/new-last.git\", rev: v3 }
}

\"a.tmt\": { source: { upstream: middle, path: \"a.tmt\" } }
"
    );
    assert!(Manifest::parse(&replaced_last).is_ok());
}

#[test]
fn add_or_replace_upstream_oneline_block() {
    let manifest = "::upstreams{ a: { git: \"https://x/a.git\", rev: main }, b: { git: \"https://x/b.git\", rev: main } }\n";
    let new_b = UpstreamText {
        git: "https://x/b2.git".into(),
        rev: "v2".into(),
        root: None,
    };

    let (replaced, outcome) = add_or_replace_upstream(manifest, "b", &new_b, true).unwrap();
    assert_eq!(outcome, AddOutcome::Replaced);
    assert_eq!(
        replaced,
        "\
::upstreams{
  a: { git: \"https://x/a.git\", rev: main }
  b: { git: \"https://x/b2.git\", rev: v2 }
}
"
    );
    assert!(Manifest::parse(&replaced).is_ok());

    let (unchanged, outcome) = add_or_replace_upstream(&replaced, "b", &new_b, true).unwrap();
    assert_eq!(outcome, AddOutcome::Unchanged);
    assert_eq!(unchanged, replaced);
}
