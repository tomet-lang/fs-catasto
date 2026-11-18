use super::*;
use crate::Manifest;

const VOCAB: &str = "\
// The files catasto manages in this workspace.
::use(catasto, version: \"1.0.0\")
::upstream(
  vocabularies: catasto.git(\"https://example.com/v.git\", rev: main, root: vocabularies)
)

// the rust one
\"schema/rust.tmt\": catasto.upstream(vocabularies, path: \"rust/rust.tmt\")
\"schema/writ.tmt\": catasto.upstream(vocabularies, path: \"writ/writ.tmt\")
";

fn up(name: &str, path: &str) -> SourceText {
    SourceText::Upstream {
        name: name.into(),
        path: path.into(),
        keep_symlink: false,
    }
}

#[test]
fn scanning_finds_the_declarations_and_the_entries() {
    let layout = scan(VOCAB).unwrap();
    let decls: Vec<_> = layout.declarations.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(decls, ["use", "upstream"]);
    let keys: Vec<_> = layout.entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(keys, ["schema/rust.tmt", "schema/writ.tmt"]);
}

#[test]
fn an_entry_is_added_after_the_last_and_the_rest_is_untouched() {
    let text = add_entry(VOCAB, "schema/ui.tmt", &up("vocabularies", "ui/ui.tmt")).unwrap();
    assert!(text.starts_with(VOCAB), "{text}");
    assert!(text.ends_with(
        "\"schema/ui.tmt\": catasto.upstream(vocabularies, path: \"ui/ui.tmt\")\n"
    ));
    assert!(Manifest::parse(&text).is_ok());
}

#[test]
fn the_first_entry_inserts_use_and_goes_below_a_blank_line() {
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
        "// nothing yet\n\n::use(catasto, version: \"1.0.0\")\n\n\"a.tmt\": catasto.local(\"../a.tmt\")\n"
    );
    assert!(Manifest::parse(&text).is_ok());
}

#[test]
fn adding_to_empty_manifest_inserts_use_and_entry() {
    let text = add_entry(
        "",
        "a.tmt",
        &SourceText::Local {
            path: "../a.tmt".into(),
        },
    )
    .unwrap();
    assert_eq!(
        text,
        "::use(catasto, version: \"1.0.0\")\n\n\"a.tmt\": catasto.local(\"../a.tmt\")\n"
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
    let text = "// about this file\n::use(catasto, version: \"1.0.0\")\n\n\"a.tmt\": catasto.local(\"x\")\n";
    assert_eq!(
        remove_entry(text, "a.tmt").unwrap(),
        "// about this file\n::use(catasto, version: \"1.0.0\")\n\n"
    );
}

#[test]
fn removing_what_is_not_there_says_so() {
    let err = remove_entry(VOCAB, "nope.tmt").unwrap_err();
    assert!(err.to_string().contains("not declared"), "{err}");
}

#[test]
fn a_value_with_parens_and_strings_inside_is_read_whole() {
    let text = "::use(catasto, version: \"1.0.0\")\n\n\"a\": (\n  catasto.git(\"https://x/).git\", rev: main, path: \"p // q\"), // trailing\n  executable: true\n)\n\"b\": catasto.local(\"y\")\n";
    let layout = scan(text).unwrap();
    assert_eq!(layout.entries.len(), 2);
    assert_eq!(layout.entries[1].key, "b");
    let after = remove_entry(text, "a").unwrap();
    assert_eq!(
        after,
        "::use(catasto, version: \"1.0.0\")\n\n\"b\": catasto.local(\"y\")\n"
    );
}

#[test]
fn an_upstream_goes_above_the_closing_paren_of_a_block_on_lines_of_its_own() {
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
    assert!(
        text.contains(
            "  vocabularies: catasto.git(\"https://example.com/v.git\", rev: main, root: vocabularies)\n  shared: catasto.git(\"https://example.com/s.git\", rev: v1)\n)\n"
        ),
        "{text}"
    );
    assert!(Manifest::parse(&text).is_ok());
}

#[test]
fn a_block_on_one_line_is_written_out_when_it_gains_a_place() {
    let one = "::use(catasto, version: \"1.0.0\")\n::upstream(a: catasto.git(\"https://x/a.git\", rev: main))\n\n\"f\": catasto.upstream(a, path: \"p\")\n";
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
            "::use(catasto, version: \"1.0.0\")\n::upstream(\n  a: catasto.git(\"https://x/a.git\", rev: main)\n  b: catasto.git(\"https://x/b.git\", rev: main, root: dir/sub)\n)\n"
        ),
        "{text}"
    );
    assert!(Manifest::parse(&text).is_ok(), "{text}");
}

#[test]
fn the_first_upstream_creates_the_declaration_and_use_above_the_entries() {
    let text = add_upstream(
        "// header\n\n\"f\": catasto.local(\"x\")\n",
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
            "// header\n\n::use(catasto, version: \"1.0.0\")\n::upstream(\n  v: catasto.git(\"https://x/v.git\", rev: main)\n)\n\n\"f\""
        ),
        "{text}"
    );
    assert!(Manifest::parse(&text).is_ok(), "{text}");
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
    assert!(text.contains("::use(catasto, version: \"1.0.0\")"), "{text}");
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

    let none = remove_upstream(
        "::use(catasto, version: \"1.0.0\")\n::upstream(\n  a: catasto.git(\"https://x/a.git\", rev: main)\n)\n\n\"f\": catasto.local(\"x\")\n",
        "a",
    )
    .unwrap();
    assert_eq!(
        none,
        "::use(catasto, version: \"1.0.0\")\n\n\"f\": catasto.local(\"x\")\n"
    );
}

#[test]
fn who_uses_an_upstream_is_found_in_the_entries() {
    assert_eq!(
        upstream_is_used(VOCAB, "vocabularies").unwrap(),
        ["schema/rust.tmt", "schema/writ.tmt"]
    );
    assert!(upstream_is_used(VOCAB, "other").unwrap().is_empty());

    let mixed = "::use(catasto, version: \"1.0.0\")\n\
                 ::upstream(a: catasto.git(\"https://x/a.git\", rev: main))\n\
                 // catasto.upstream(a, path: \"ignored-comment\")\n\
                 \"p1\": (catasto.upstream(upstream: \"a\", path: \"one\"), executable: true)\n\
                 \"p2\": catasto.local(\"upstream: a\")\n";
    assert_eq!(upstream_is_used(mixed, "a").unwrap(), ["p1"]);
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
        "\"CLAUDE.md\": catasto.upstream(commonfiles, path: \"CLAUDE.md\")\n"
    );
    assert_eq!(
        entry_text("CLAUDE.md", &upstream_keep),
        "\"CLAUDE.md\": catasto.upstream(commonfiles, path: \"CLAUDE.md\", symlink: keep)\n"
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
        "\"bin/tool\": catasto.git(\"https://example.com/repo.git\", rev: main, path: \"bin/tool\")\n"
    );
    assert_eq!(
        entry_text("bin/tool", &git_keep),
        "\"bin/tool\": catasto.git(\"https://example.com/repo.git\", rev: main, path: \"bin/tool\", symlink: keep)\n"
    );

    // Verify added entry with symlink: keep parses cleanly
    let text = add_entry(
        VOCAB,
        "link.tmt",
        &SourceText::Upstream {
            name: "vocabularies".into(),
            path: "rust/rust.tmt".into(),
            keep_symlink: true,
        },
    )
    .unwrap();
    assert!(Manifest::parse(&text).is_ok());
}

#[test]
fn add_or_replace_entry_keeps_position_comment_and_executable_in_tuple() {
    let manifest = "\
// A comment before the entries
::use(catasto, version: \"1.0.0\")
::upstream(
  vocabularies: catasto.git(\"https://example.com/v.git\", rev: main)
)

\"first.tmt\": catasto.local(\"old/first.tmt\")

// A comment on the target entry
\"target.sh\": (
  catasto.local(\"old/target.sh\"),
  executable: true
)

// A comment on the bare target
\"bare.tmt\": catasto.local(\"old/bare.tmt\")
";

    let new_source = SourceText::Upstream {
        name: "vocabularies".into(),
        path: "new/target.sh".into(),
        keep_symlink: false,
    };

    // 1. without force: fails with AlreadyDeclared
    let err = add_or_replace_entry(manifest, "target.sh", &new_source, false).unwrap_err();
    let already = err
        .downcast_ref::<AlreadyDeclared>()
        .expect("downcast to AlreadyDeclared");
    assert_eq!(already.dest, "target.sh");
    assert_eq!(
        err.to_string(),
        "target.sh is already declared (use --force to replace its source)"
    );

    // 2. with force on tuple entry: replaces call and keeps comment and executable: true
    let (replaced_text, outcome) =
        add_or_replace_entry(manifest, "target.sh", &new_source, true).unwrap();
    assert_eq!(outcome, AddOutcome::Replaced);
    assert_eq!(
        replaced_text,
        "\
// A comment before the entries
::use(catasto, version: \"1.0.0\")
::upstream(
  vocabularies: catasto.git(\"https://example.com/v.git\", rev: main)
)

\"first.tmt\": catasto.local(\"old/first.tmt\")

// A comment on the target entry
\"target.sh\": (
  catasto.upstream(vocabularies, path: \"new/target.sh\"),
  executable: true
)

// A comment on the bare target
\"bare.tmt\": catasto.local(\"old/bare.tmt\")
"
    );
    assert!(Manifest::parse(&replaced_text).is_ok());

    // 3. replacing with an equal source returns the same text (Unchanged)
    let (unchanged_text, outcome) =
        add_or_replace_entry(&replaced_text, "target.sh", &new_source, true).unwrap();
    assert_eq!(outcome, AddOutcome::Unchanged);
    assert_eq!(unchanged_text, replaced_text);

    // 4. with force on bare call entry: replaces whole call
    let bare_source = SourceText::Local {
        path: "new/bare.tmt".into(),
    };
    let (replaced_bare, outcome) =
        add_or_replace_entry(&replaced_text, "bare.tmt", &bare_source, true).unwrap();
    assert_eq!(outcome, AddOutcome::Replaced);
    assert!(
        replaced_bare.contains("\"bare.tmt\": catasto.local(\"new/bare.tmt\")"),
        "{replaced_bare}"
    );
    assert!(Manifest::parse(&replaced_bare).is_ok());
}

#[test]
fn add_or_replace_upstream_multiline_block() {
    let manifest = "\
::use(catasto, version: \"1.0.0\")
::upstream(
  // comment above first
  first: catasto.git(\"https://example.com/first.git\", rev: v1)
  // comment above middle
  middle: catasto.git(\"https://example.com/middle.git\", rev: v1)
  // comment above last
  last: catasto.git(\"https://example.com/last.git\", rev: v1)
)

\"a.tmt\": catasto.upstream(middle, path: \"a.tmt\")
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
::use(catasto, version: \"1.0.0\")
::upstream(
  // comment above first
  first: catasto.git(\"https://example.com/first.git\", rev: v1)
  // comment above middle
  middle: catasto.git(\"https://example.com/new-middle.git\", rev: v2, root: subdir)
  // comment above last
  last: catasto.git(\"https://example.com/last.git\", rev: v1)
)

\"a.tmt\": catasto.upstream(middle, path: \"a.tmt\")
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
::use(catasto, version: \"1.0.0\")
::upstream(
  // comment above first
  first: catasto.git(\"https://example.com/new-first.git\", rev: main)
  // comment above middle
  middle: catasto.git(\"https://example.com/new-middle.git\", rev: v2, root: subdir)
  // comment above last
  last: catasto.git(\"https://example.com/last.git\", rev: v1)
)

\"a.tmt\": catasto.upstream(middle, path: \"a.tmt\")
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
::use(catasto, version: \"1.0.0\")
::upstream(
  // comment above first
  first: catasto.git(\"https://example.com/new-first.git\", rev: main)
  // comment above middle
  middle: catasto.git(\"https://example.com/new-middle.git\", rev: v2, root: subdir)
  // comment above last
  last: catasto.git(\"https://example.com/new-last.git\", rev: v3)
)

\"a.tmt\": catasto.upstream(middle, path: \"a.tmt\")
"
    );
    assert!(Manifest::parse(&replaced_last).is_ok());
}

#[test]
fn add_or_replace_upstream_oneline_block() {
    let manifest = "::use(catasto, version: \"1.0.0\")\n::upstream(a: catasto.git(\"https://x/a.git\", rev: main), b: catasto.git(\"https://x/b.git\", rev: main))\n";
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
::use(catasto, version: \"1.0.0\")
::upstream(
  a: catasto.git(\"https://x/a.git\", rev: main)
  b: catasto.git(\"https://x/b2.git\", rev: v2)
)
"
    );
    assert!(Manifest::parse(&replaced).is_ok());

    let (unchanged, outcome) = add_or_replace_upstream(&replaced, "b", &new_b, true).unwrap();
    assert_eq!(outcome, AddOutcome::Unchanged);
    assert_eq!(unchanged, replaced);
}
