use super::*;

fn workspace(manifest: &str, files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("catasto.ctmt"), manifest).unwrap();
    for (path, content) in files {
        let file = dir.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }
    dir
}

const MANIFEST: &str = "\
::kind(catasto, version: \"1.0.0\")

\"b.tmt\": catasto.local(\"src/b.tmt\")
\"a.tmt\": catasto.local(\"src/a.tmt\")
";

#[test]
fn lock_records_hashes_sorted_by_path() {
    let dir = workspace(MANIFEST, &[("a.tmt", "abc"), ("b.tmt", "")]);

    let lock = lock(dir.path()).unwrap();

    let paths: Vec<_> = lock
        .locked
        .iter()
        .map(|l| l.path.to_str().unwrap())
        .collect();
    assert_eq!(paths, ["a.tmt", "b.tmt"]);
    assert_eq!(
        lock.locked[0].sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn written_lock_round_trips() {
    let dir = workspace(MANIFEST, &[("a.tmt", "abc"), ("b.tmt", "x")]);

    let written = lock(dir.path()).unwrap();

    assert_eq!(Lock::load(dir.path()).unwrap(), written);
}

#[test]
fn the_lock_text_is_json_keyed_by_path() {
    let dir = workspace(
        "::kind(catasto, version: \"1.0.0\")\n\n\"a.tmt\": catasto.local(\"src/a.tmt\")\n",
        &[("a.tmt", "abc")],
    );

    lock(dir.path()).unwrap();

    let text = std::fs::read_to_string(dir.path().join(LOCK_FILE)).unwrap();
    assert_eq!(
        text,
        "{\n  \"version\": 1,\n  \"files\": {\n    \"a.tmt\": {\n      \"sha256\": \
             \"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\",\n      \
             \"source\": \"src/a.tmt\"\n    }\n  }\n}\n"
    );
}

#[test]
fn a_file_named_version_does_not_collide_with_the_format() {
    let dir = workspace(
        "::kind(catasto, version: \"1.0.0\")\n\nversion: catasto.local(\"src/version\")\n",
        &[("version", "1")],
    );

    let written = lock(dir.path()).unwrap();

    assert_eq!(Lock::load(dir.path()).unwrap(), written);
    assert_eq!(written.locked[0].path, PathBuf::from("version"));
}

#[test]
fn lock_is_deterministic() {
    let dir = workspace(MANIFEST, &[("a.tmt", "abc"), ("b.tmt", "x")]);

    lock(dir.path()).unwrap();
    let first = std::fs::read(dir.path().join(LOCK_FILE)).unwrap();
    lock(dir.path()).unwrap();
    let second = std::fs::read(dir.path().join(LOCK_FILE)).unwrap();

    assert_eq!(first, second);
}

#[test]
fn lock_fails_when_a_managed_file_is_missing() {
    let dir = workspace(MANIFEST, &[("a.tmt", "abc")]);

    let err = lock(dir.path()).unwrap_err();

    assert!(err.to_string().contains("b.tmt"), "{err}");
    assert!(!dir.path().join(LOCK_FILE).exists());
}

const GIT_MANIFEST: &str = "\
::kind(catasto, version: \"1.0.0\")

\"w.tmt\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"w.tmt\")
";

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn locked_git_entry(commit: &str, rev: &str) -> String {
    format!(
        "{{\"version\":1,\"files\":{{\"w.tmt\":{{\"sha256\":\"x\",\"source\":\
             {{\"git\":\"https://example.com/v.git\",\"rev\":\"{rev}\",\"path\":\"w.tmt\"}},\
             \"commit\":\"{commit}\"}}}}}}"
    )
}

#[test]
fn lock_cannot_pin_a_git_source_by_itself() {
    let dir = workspace(GIT_MANIFEST, &[("w.tmt", "abc")]);

    let err = lock(dir.path()).unwrap_err();

    assert!(err.to_string().contains("run `catasto sync`"), "{err}");
}

#[test]
fn lock_keeps_the_commit_of_an_unchanged_git_source() {
    let dir = workspace(GIT_MANIFEST, &[("w.tmt", "abc")]);
    std::fs::write(dir.path().join(LOCK_FILE), locked_git_entry(COMMIT, "v1")).unwrap();

    let lock = lock(dir.path()).unwrap();

    assert_eq!(lock.locked[0].commit.as_deref(), Some(COMMIT));
    assert_eq!(Lock::load(dir.path()).unwrap(), lock);
}

#[test]
fn lock_drops_the_commit_when_the_rev_changed() {
    let dir = workspace(GIT_MANIFEST, &[("w.tmt", "abc")]);
    std::fs::write(dir.path().join(LOCK_FILE), locked_git_entry(COMMIT, "v0")).unwrap();

    assert!(lock(dir.path()).is_err());
}

#[test]
fn path_sources_have_no_commit_in_the_lock() {
    let dir = workspace(MANIFEST, &[("a.tmt", "abc"), ("b.tmt", "x")]);

    lock(dir.path()).unwrap();

    let text = std::fs::read_to_string(dir.path().join(LOCK_FILE)).unwrap();
    assert!(!text.contains("commit"), "{text}");
}

#[test]
fn missing_lock_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(Lock::load(dir.path()).unwrap(), Lock::default());
}

#[test]
fn a_lock_spelled_in_another_case_is_named_not_read_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Catasto.lock"), "").unwrap();
    let err = Lock::load(dir.path()).unwrap_err().to_string();
    assert!(
        err.contains("found Catasto.lock") && err.contains("rename it"),
        "{err}"
    );
}

#[test]
fn an_older_toml_lock_says_what_to_do() {
    let err =
        Lock::parse("# Generated by `catasto lock`. Do not edit.\n\n[[locked]]\n").unwrap_err();
    assert!(
        err.to_string().contains("older catasto wrote TOML"),
        "{err}"
    );
}

#[test]
fn a_lock_of_another_version_is_rejected() {
    let err = Lock::parse("{\"version\":2,\"files\":{}}").unwrap_err();
    assert!(
        err.to_string().contains("unsupported lock version 2"),
        "{err}"
    );
}

#[test]
fn unknown_fields_are_rejected() {
    assert!(Lock::parse("{\"version\":1,\"files\":{},\"extra\":1}").is_err());
    assert!(
        Lock::parse(
            "{\"version\":1,\"files\":{\"a\":{\"sha256\":\"x\",\"source\":\"s\",\"extra\":1}}}"
        )
        .is_err()
    );
}

#[test]
fn lock_round_trip_with_and_without_symlink_keep() {
    let text_without = "{\n  \"version\": 1,\n  \"files\": {\n    \"a\": {\n      \"sha256\": \"abc\",\n      \"source\": {\n        \"git\": \"https://example.com/a.git\",\n        \"rev\": \"main\",\n        \"path\": \"a\"\n      },\n      \"commit\": \"0123456789abcdef0123456789abcdef01234567\"\n    }\n  }\n}\n";
    let parsed = Lock::parse(text_without).unwrap();
    assert_eq!(parsed.render().unwrap(), text_without);

    let text_with = "{\n  \"version\": 1,\n  \"files\": {\n    \"a\": {\n      \"sha256\": \"abc\",\n      \"source\": {\n        \"git\": \"https://example.com/a.git\",\n        \"rev\": \"main\",\n        \"path\": \"a\"\n      },\n      \"commit\": \"0123456789abcdef0123456789abcdef01234567\",\n      \"symlink\": \"keep\"\n    }\n  }\n}\n";
    let parsed_keep = Lock::parse(text_with).unwrap();
    assert_eq!(parsed_keep.render().unwrap(), text_with);
}

#[test]
#[cfg(unix)]
fn lock_fails_when_destination_kind_does_not_match_symlink_declaration() {
    let keep_manifest = "::kind(catasto, version: \"1.0.0\")\n\n\"link\": (catasto.git(\"https://example.com/v.git\", rev: v1, path: \"link\"), symlink: keep)\n";
    let default_manifest = "::kind(catasto, version: \"1.0.0\")\n\n\"link\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"link\")\n";

    // 1. keep declared, but destination is regular file -> error
    let dir1 = workspace(keep_manifest, &[("link", "regular file")]);
    std::fs::write(
            dir1.path().join(LOCK_FILE),
            "{\"version\":1,\"files\":{\"link\":{\"sha256\":\"x\",\"source\":{\"git\":\"https://example.com/v.git\",\"rev\":\"v1\",\"path\":\"link\"},\"commit\":\"0123456789abcdef0123456789abcdef01234567\",\"symlink\":\"keep\"}}}",
        )
        .unwrap();
    let err1 = lock(dir1.path()).unwrap_err();
    assert!(
        err1.to_string()
            .contains("`symlink: keep` declared for link, but destination is not a symlink"),
        "{err1}"
    );

    // 2. default declared, but destination is symlink -> error
    let dir2 = workspace(default_manifest, &[]);
    std::os::unix::fs::symlink("target", dir2.path().join("link")).unwrap();
    std::fs::write(
            dir2.path().join(LOCK_FILE),
            "{\"version\":1,\"files\":{\"link\":{\"sha256\":\"x\",\"source\":{\"git\":\"https://example.com/v.git\",\"rev\":\"v1\",\"path\":\"link\"},\"commit\":\"0123456789abcdef0123456789abcdef01234567\"}}}",
        )
        .unwrap();
    let err2 = lock(dir2.path()).unwrap_err();
    assert!(
        err2.to_string()
            .contains("managed file link is a symlink; expected a regular file"),
        "{err2}"
    );
}

#[test]
fn mode_executable_round_trips_and_absent_for_normal_file() {
    let text_with_mode = "{\n  \"version\": 1,\n  \"files\": {\n    \"run.sh\": {\n      \"sha256\": \"abc\",\n      \"source\": {\n        \"git\": \"https://example.com/a.git\",\n        \"rev\": \"main\",\n        \"path\": \"run.sh\"\n      },\n      \"commit\": \"0123456789abcdef0123456789abcdef01234567\",\n      \"mode\": \"executable\"\n    }\n  }\n}\n";
    let parsed = Lock::parse(text_with_mode).unwrap();
    assert_eq!(parsed.locked[0].mode, Some(LockMode::Executable));
    assert_eq!(parsed.render().unwrap(), text_with_mode);

    let text_without_mode = "{\n  \"version\": 1,\n  \"files\": {\n    \"run.sh\": {\n      \"sha256\": \"abc\",\n      \"source\": {\n        \"git\": \"https://example.com/a.git\",\n        \"rev\": \"main\",\n        \"path\": \"run.sh\"\n      },\n      \"commit\": \"0123456789abcdef0123456789abcdef01234567\"\n    }\n  }\n}\n";
    let parsed_normal = Lock::parse(text_without_mode).unwrap();
    assert_eq!(parsed_normal.locked[0].mode, None);
    assert_eq!(parsed_normal.render().unwrap(), text_without_mode);
}

#[test]
fn lock_preserves_existing_mode() {
    let manifest = "::kind(catasto, version: \"1.0.0\")\n\n\"run.sh\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"run.sh\")\n";
    let dir = workspace(manifest, &[("run.sh", "#!/bin/sh")]);
    let initial_lock = "{\n  \"version\": 1,\n  \"files\": {\n    \"run.sh\": {\n      \"sha256\": \"x\",\n      \"source\": {\n        \"git\": \"https://example.com/v.git\",\n        \"rev\": \"v1\",\n        \"path\": \"run.sh\"\n      },\n      \"commit\": \"0123456789abcdef0123456789abcdef01234567\",\n      \"mode\": \"executable\"\n    }\n  }\n}\n";
    std::fs::write(dir.path().join(LOCK_FILE), initial_lock).unwrap();

    let updated = lock(dir.path()).unwrap();
    assert_eq!(updated.locked[0].mode, Some(LockMode::Executable));
    assert_eq!(
        updated.locked[0].commit.as_deref(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
    let rendered = std::fs::read_to_string(dir.path().join(LOCK_FILE)).unwrap();
    assert!(rendered.contains("\"mode\": \"executable\""), "{rendered}");
}

#[test]
fn lock_records_submodule_entries_in_submodules_section() {
    let manifest = "\
::kind(catasto, version: \"1.0.0\")

\"tools/sisho\": (catasto.git(\"https://github.com/tomet-lang/sisho.git\", rev: main, path: \"crates/sisho\"), submodule: true)
";
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("catasto.ctmt"), manifest).unwrap();

    let locked = lock(dir.path()).unwrap();
    assert_eq!(locked.locked.len(), 0);
    assert_eq!(locked.submodules.len(), 1);
    assert_eq!(
        locked.submodules[0],
        LockedSubmodule {
            path: "tools/sisho".into(),
            git: "https://github.com/tomet-lang/sisho.git".into(),
            rev: "main".into(),
            subpath: Some("crates/sisho".into()),
            commit: None,
        }
    );

    let text = std::fs::read_to_string(dir.path().join(LOCK_FILE)).unwrap();
    assert!(text.contains("\"submodules\": {"), "{text}");
    assert!(
        text.contains("\"git\": \"https://github.com/tomet-lang/sisho.git\""),
        "{text}"
    );
    assert!(text.contains("\"rev\": \"main\""), "{text}");
    assert!(text.contains("\"path\": \"crates/sisho\""), "{text}");

    let loaded = Lock::load(dir.path()).unwrap();
    assert_eq!(loaded, locked);
}

#[test]
fn submodule_lock_preserves_existing_commit() {
    let manifest = "\
::kind(catasto, version: \"1.0.0\")

\"sisho\": (catasto.git(\"https://github.com/tomet-lang/sisho.git\", rev: main), submodule: true)
";
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("catasto.ctmt"), manifest).unwrap();
    let initial_lock = r#"{
  "version": 1,
  "submodules": {
    "sisho": {
      "git": "https://github.com/tomet-lang/sisho.git",
      "rev": "main",
      "commit": "0123456789abcdef0123456789abcdef01234567"
    }
  }
}
"#;
    std::fs::write(dir.path().join(LOCK_FILE), initial_lock).unwrap();

    let updated = lock(dir.path()).unwrap();
    assert_eq!(
        updated.submodules[0].commit.as_deref(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
}

#[test]
fn lock_without_submodules_omits_submodules_key() {
    let lock = Lock::from_files(vec![Locked {
        path: "a.tmt".into(),
        source: Source::Path("src/a.tmt".into()),
        sha256: "abc".into(),
        commit: None,
        mode: None,
        symlink: Symlink::Follow,
    }]);
    let rendered = lock.render().unwrap();
    assert!(!rendered.contains("\"submodules\""), "{rendered}");
    assert!(!rendered.contains("\"repos\""), "{rendered}");
}

#[test]
#[cfg(unix)]
fn lock_hashes_managed_directory_and_omits_mode() {
    use std::os::unix::fs::PermissionsExt;

    let manifest = "\
::kind(catasto, version: \"1.0.0\")

\"dir\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"dir\")
";
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("catasto.ctmt"), manifest).unwrap();
    std::fs::create_dir_all(dir.path().join("dir/sub")).unwrap();
    std::fs::write(dir.path().join("dir/a.txt"), "hello").unwrap();
    let script = dir.path().join("dir/sub/run.sh");
    std::fs::write(&script, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let initial_lock = "{\n  \"version\": 1,\n  \"files\": {\n    \"dir\": {\n      \"sha256\": \"old\",\n      \"source\": {\n        \"git\": \"https://example.com/v.git\",\n        \"rev\": \"v1\",\n        \"path\": \"dir\"\n      },\n      \"commit\": \"0123456789abcdef0123456789abcdef01234567\"\n    }\n  }\n}\n";
    std::fs::write(dir.path().join(LOCK_FILE), initial_lock).unwrap();

    let updated = lock(dir.path()).unwrap();
    assert_eq!(updated.locked.len(), 1);
    assert_eq!(updated.locked[0].mode, None);
    assert_eq!(
        updated.locked[0].sha256,
        catasto_digest::sha256_dir(&dir.path().join("dir")).unwrap()
    );
    assert!(crate::check::check(dir.path()).unwrap().is_ok());
}
