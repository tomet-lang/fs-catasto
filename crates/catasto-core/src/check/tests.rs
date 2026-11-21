use super::*;
use crate::lockfile::lock;

const MANIFEST: &str = "\
::kind(catasto, version: \"1.0.0\")

\"vocab/writ.tmt\": catasto.local(\"../vocabularies/writ.tmt\")
";

/// A vault at `<tmp>/vault` with a sibling `<tmp>/vocabularies`, already locked.
struct Fixture {
    _tmp: tempfile::TempDir,
    vault: PathBuf,
    source: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        let source = tmp.path().join("vocabularies/writ.tmt");
        std::fs::create_dir_all(vault.join("vocab")).unwrap();
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(vault.join("catasto.ctmt"), MANIFEST).unwrap();
        std::fs::write(vault.join("vocab/writ.tmt"), "v1").unwrap();
        std::fs::write(&source, "v1").unwrap();
        lock(&vault).unwrap();
        Self {
            _tmp: tmp,
            vault,
            source,
        }
    }

    fn kinds(&self) -> Vec<ViolationKind> {
        check(&self.vault)
            .unwrap()
            .violations
            .into_iter()
            .map(|v| v.kind)
            .collect()
    }
}

#[test]
fn clean_workspace_has_no_violations() {
    assert!(Fixture::new().kinds().is_empty());
}

#[test]
fn detects_a_modified_file() {
    let fx = Fixture::new();
    std::fs::write(fx.vault.join("vocab/writ.tmt"), "edited").unwrap();

    assert_eq!(fx.kinds(), [ViolationKind::Modified]);
}

#[test]
fn detects_a_missing_file() {
    let fx = Fixture::new();
    std::fs::remove_file(fx.vault.join("vocab/writ.tmt")).unwrap();

    assert_eq!(fx.kinds(), [ViolationKind::Missing]);
}

#[test]
fn detects_a_stale_file_when_the_source_moved_on() {
    let fx = Fixture::new();
    std::fs::write(&fx.source, "v2").unwrap();

    assert_eq!(fx.kinds(), [ViolationKind::Stale]);
}

#[test]
fn reports_modified_and_stale_together() {
    let fx = Fixture::new();
    std::fs::write(fx.vault.join("vocab/writ.tmt"), "edited").unwrap();
    std::fs::write(&fx.source, "v2").unwrap();

    assert_eq!(fx.kinds(), [ViolationKind::Modified, ViolationKind::Stale]);
}

#[test]
fn an_unreachable_source_is_not_stale() {
    let fx = Fixture::new();
    std::fs::remove_file(&fx.source).unwrap();

    assert!(fx.kinds().is_empty());
}

#[test]
fn detects_a_declaration_missing_from_the_lock() {
    let fx = Fixture::new();
    std::fs::remove_file(fx.vault.join("catasto.lock")).unwrap();

    assert_eq!(fx.kinds(), [ViolationKind::Unlocked]);
}

#[test]
fn a_changed_source_declaration_needs_relocking() {
    let fx = Fixture::new();
    std::fs::write(
        fx.vault.join("catasto.ctmt"),
        MANIFEST.replace("../vocabularies/writ.tmt", "../elsewhere/writ.tmt"),
    )
    .unwrap();

    assert_eq!(fx.kinds(), [ViolationKind::Unlocked]);
}

#[test]
fn detects_a_locked_file_that_is_no_longer_declared() {
    let fx = Fixture::new();
    std::fs::remove_file(fx.vault.join("catasto.ctmt")).unwrap();

    assert_eq!(fx.kinds(), [ViolationKind::Orphaned]);
}

#[test]
fn unmanaged_files_are_ignored() {
    let fx = Fixture::new();
    std::fs::write(fx.vault.join("vocab/other.tmt"), "mine").unwrap();

    assert!(fx.kinds().is_empty());
}

#[test]
fn a_workspace_without_catasto_files_is_clean() {
    let dir = tempfile::tempdir().unwrap();
    assert!(check(dir.path()).unwrap().is_ok());
}

#[test]
fn a_missing_root_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(check(&dir.path().join("nope")).is_err());
}

const GIT_SOURCE: &str = "catasto.git(\"/does/not/exist.git\", rev: main, path: \"w.tmt\")";

/// A vault with one git-sourced file whose lock entry records `commit`.
fn git_vault(commit: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.tmt"), "abc").unwrap();
    std::fs::write(
        dir.path().join("catasto.ctmt"),
        format!("::kind(catasto, version: \"1.0.0\")\n\n\"w.tmt\": {GIT_SOURCE}\n"),
    )
    .unwrap();
    let commit = commit
        .map(|c| format!(",\"commit\":\"{c}\""))
        .unwrap_or_default();
    std::fs::write(
            dir.path().join("catasto.lock"),
            format!(
                "{{\"version\":1,\"files\":{{\"w.tmt\":{{\"sha256\":\
                 \"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\",\
                 \"source\":{{\"git\":\"/does/not/exist.git\",\"rev\":\"main\",\"path\":\"w.tmt\"}}{commit}}}}}}}"
            ),
        )
        .unwrap();
    dir
}

#[test]
fn a_git_source_checks_offline() {
    let dir = git_vault(Some("0123456789abcdef0123456789abcdef01234567"));

    assert!(check(dir.path()).unwrap().is_ok());
}

#[test]
fn a_git_entry_without_a_commit_needs_syncing() {
    let dir = git_vault(None);

    let kinds: Vec<_> = check(dir.path())
        .unwrap()
        .violations
        .into_iter()
        .map(|v| v.kind)
        .collect();

    assert_eq!(kinds, [ViolationKind::Unlocked]);
}

#[test]
fn a_modified_git_file_is_still_detected_offline() {
    let dir = git_vault(Some("0123456789abcdef0123456789abcdef01234567"));
    std::fs::write(dir.path().join("w.tmt"), "edited").unwrap();

    let kinds: Vec<_> = check(dir.path())
        .unwrap()
        .violations
        .into_iter()
        .map(|v| v.kind)
        .collect();

    assert_eq!(kinds, [ViolationKind::Modified]);
}

#[test]
#[cfg(unix)]
fn check_symlink_keep_and_default_table() {
    use catasto_digest::sha256_hex;

    let commit = "0123456789abcdef0123456789abcdef01234567";
    let target_hash = sha256_hex(b"target.txt");

    // Helper to setup a vault with custom manifest and lock
    let setup_vault = |manifest_src: &str, lock_src: &str, locked_hash: &str| {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("catasto.ctmt"),
            format!("::kind(catasto, version: \"1.0.0\")\n\n\"link\": {manifest_src}\n"),
        )
        .unwrap();
        std::fs::write(
                tmp.path().join("catasto.lock"),
                format!(
                    "{{\"version\":1,\"files\":{{\"link\":{{\"sha256\":\"{locked_hash}\",\"source\":{lock_src},\"commit\":\"{commit}\"}}}}}}"
                ),
            )
            .unwrap();
        tmp
    };

    let keep_manifest =
        "catasto.git(\"https://example.com/v.git\", rev: main, path: \"link\", symlink: keep)";
    let default_manifest = "catasto.git(\"https://example.com/v.git\", rev: main, path: \"link\")";
    let keep_lock = "{\"git\":\"https://example.com/v.git\",\"rev\":\"main\",\"path\":\"link\",\"symlink\":\"keep\"}";
    let default_lock = "{\"git\":\"https://example.com/v.git\",\"rev\":\"main\",\"path\":\"link\"}";

    // === Keep entry tests ===
    // 1. absent -> Missing
    let v = setup_vault(keep_manifest, keep_lock, &target_hash);
    assert_eq!(
        check(v.path()).unwrap().violations[0].kind,
        ViolationKind::Missing
    );

    // 2. present but not a symlink (regular file) -> Modified
    let v = setup_vault(keep_manifest, keep_lock, &target_hash);
    std::fs::write(v.path().join("link"), "regular content").unwrap();
    assert_eq!(
        check(v.path()).unwrap().violations[0].kind,
        ViolationKind::Modified
    );

    // 3. symlink whose read_link digest differs -> Modified
    let v = setup_vault(keep_manifest, keep_lock, &target_hash);
    std::os::unix::fs::symlink("other.txt", v.path().join("link")).unwrap();
    assert_eq!(
        check(v.path()).unwrap().violations[0].kind,
        ViolationKind::Modified
    );

    // 4. symlink matching digest (even if dangling target!) -> Clean
    let v = setup_vault(keep_manifest, keep_lock, &target_hash);
    std::os::unix::fs::symlink("target.txt", v.path().join("link")).unwrap();
    assert!(check(v.path()).unwrap().is_ok());

    // === Default entry tests ===
    // 5. absent -> Missing
    let v = setup_vault(default_manifest, default_lock, &target_hash);
    assert_eq!(
        check(v.path()).unwrap().violations[0].kind,
        ViolationKind::Missing
    );

    // 6. destination is a symlink -> Modified
    let v = setup_vault(default_manifest, default_lock, &target_hash);
    std::os::unix::fs::symlink("target.txt", v.path().join("link")).unwrap();
    assert_eq!(
        check(v.path()).unwrap().violations[0].kind,
        ViolationKind::Modified
    );

    // 7. regular file with matching digest -> Clean
    let v = setup_vault(default_manifest, default_lock, &target_hash);
    std::fs::write(v.path().join("link"), "target.txt").unwrap();
    assert!(check(v.path()).unwrap().is_ok());

    // 8. regular file with wrong digest -> Modified
    let v = setup_vault(default_manifest, default_lock, &target_hash);
    std::fs::write(v.path().join("link"), "wrong").unwrap();
    assert_eq!(
        check(v.path()).unwrap().violations[0].kind,
        ViolationKind::Modified
    );

    // === Toggling symlink option -> Unlocked ===
    // Manifest says keep, lock says default
    let v = setup_vault(keep_manifest, default_lock, &target_hash);
    std::os::unix::fs::symlink("target.txt", v.path().join("link")).unwrap();
    assert_eq!(
        check(v.path()).unwrap().violations[0].kind,
        ViolationKind::Unlocked
    );

    // Manifest says default, lock says keep
    let v = setup_vault(default_manifest, keep_lock, &target_hash);
    std::fs::write(v.path().join("link"), "target.txt").unwrap();
    assert_eq!(
        check(v.path()).unwrap().violations[0].kind,
        ViolationKind::Unlocked
    );
}

#[test]
#[cfg(unix)]
fn check_detects_executable_bit_drift_in_both_directions() {
    use std::os::unix::fs::PermissionsExt;

    let manifest = "\
::kind(catasto, version: \"1.0.0\")

\"normal.txt\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"normal.txt\")
\"tool.sh\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"tool.sh\")
";
    let hash_normal = catasto_digest::sha256_hex(b"normal content");
    let hash_tool = catasto_digest::sha256_hex(b"tool content");
    let lock_text = format!(
        "{{\n  \"version\": 1,\n  \"files\": {{\n    \
         \"normal.txt\": {{\n      \"sha256\": \"{hash_normal}\",\n      \
         \"source\": {{\n        \"git\": \"https://example.com/v.git\",\n        \"rev\": \"v1\",\n        \"path\": \"normal.txt\"\n      }},\n      \
         \"commit\": \"0123456789abcdef0123456789abcdef01234567\"\n    }},\n    \
         \"tool.sh\": {{\n      \"sha256\": \"{hash_tool}\",\n      \
         \"source\": {{\n        \"git\": \"https://example.com/v.git\",\n        \"rev\": \"v1\",\n        \"path\": \"tool.sh\"\n      }},\n      \
         \"commit\": \"0123456789abcdef0123456789abcdef01234567\",\n      \
         \"mode\": \"executable\"\n    }}\n  }}\n}}\n"
    );

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("catasto.ctmt"), manifest).unwrap();
    std::fs::write(root.join("catasto.lock"), &lock_text).unwrap();

    let normal_path = root.join("normal.txt");
    let tool_path = root.join("tool.sh");
    std::fs::write(&normal_path, "normal content").unwrap();
    std::fs::write(&tool_path, "tool content").unwrap();
    std::fs::set_permissions(&normal_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::set_permissions(&tool_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    // 1. Initial state matches lock -> clean
    let rep = check(root).unwrap();
    assert!(rep.is_ok(), "{:?}", rep.violations);

    // 2. normal.txt gets chmod +x (differs from lock which is not executable) -> Modified
    std::fs::set_permissions(&normal_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let rep = check(root).unwrap();
    assert_eq!(rep.violations.len(), 1);
    assert_eq!(rep.violations[0].path, Path::new("normal.txt"));
    assert_eq!(rep.violations[0].kind, ViolationKind::Modified);

    // Revert normal.txt
    std::fs::set_permissions(&normal_path, std::fs::Permissions::from_mode(0o644)).unwrap();

    // 3. tool.sh gets chmod -x (differs from lock which is executable) -> Modified
    std::fs::set_permissions(&tool_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let rep = check(root).unwrap();
    assert_eq!(rep.violations.len(), 1);
    assert_eq!(rep.violations[0].path, Path::new("tool.sh"));
    assert_eq!(rep.violations[0].kind, ViolationKind::Modified);
}

#[test]
#[cfg(unix)]
fn old_lock_without_mode_is_clean_when_destination_is_normal_file() {
    use std::os::unix::fs::PermissionsExt;

    // An old lock for tool.sh has no mode field
    let manifest = "::kind(catasto, version: \"1.0.0\")\n\n\"tool.sh\": catasto.git(\"https://example.com/v.git\", rev: v1, path: \"tool.sh\")\n";
    let hash = catasto_digest::sha256_hex(b"#!/bin/sh\n");
    let lock_text = format!(
        "{{\n  \"version\": 1,\n  \"files\": {{\n    \
         \"tool.sh\": {{\n      \"sha256\": \"{hash}\",\n      \
         \"source\": {{\n        \"git\": \"https://example.com/v.git\",\n        \"rev\": \"v1\",\n        \"path\": \"tool.sh\"\n      }},\n      \
         \"commit\": \"0123456789abcdef0123456789abcdef01234567\"\n    }}\n  }}\n}}\n"
    );

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("catasto.ctmt"), manifest).unwrap();
    std::fs::write(root.join("catasto.lock"), &lock_text).unwrap();

    let tool_path = root.join("tool.sh");
    std::fs::write(&tool_path, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&tool_path, std::fs::Permissions::from_mode(0o644)).unwrap();

    // Destination is normal file on disk, lock has no mode -> check succeeds (no network consulted)
    let rep = check(root).unwrap();
    assert!(rep.is_ok(), "{:?}", rep.violations);
}

#[test]
fn check_repo_detects_unlocked_missing_and_modified_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();

    let upstream = catasto_git::testing::TestRepo::new();
    let head = upstream.commit("README.md", "hello");
    let other = catasto_git::testing::TestRepo::new();
    other.commit("README.md", "other");

    std::fs::write(
        vault.join("catasto.ctmt"),
        format!(
            "::kind(catasto, version: \"1.0.0\")\n\n\"deps/tool\": catasto.repo({:?}, branch: main)\n",
            upstream.url()
        ),
    )
    .unwrap();

    // 1. No lockfile -> Unlocked
    let rep = check(&vault).unwrap();
    assert_eq!(rep.violations.len(), 1);
    assert_eq!(rep.violations[0].path, Path::new("deps/tool"));
    assert_eq!(rep.violations[0].kind, ViolationKind::Unlocked);

    // 2. Locked, but directory missing -> Missing
    let lock_text = format!(
        "{{\n  \"version\": 1,\n  \"files\": {{}},\n  \"repos\": {{\n    \"deps/tool\": {{\n      \"git\": {:?},\n      \"branch\": \"main\",\n      \"commit\": \"{head}\"\n    }}\n  }}\n}}\n",
        upstream.url()
    );
    std::fs::write(vault.join("catasto.lock"), lock_text).unwrap();
    let rep = check(&vault).unwrap();
    assert_eq!(rep.violations.len(), 1);
    assert_eq!(rep.violations[0].kind, ViolationKind::Missing);

    // 3. Cloned from wrong remote -> Modified
    catasto_git::clone_repo(&other.url(), &vault.join("deps/tool"), Some("main")).unwrap();
    let rep = check(&vault).unwrap();
    assert_eq!(rep.violations.len(), 1);
    assert_eq!(rep.violations[0].kind, ViolationKind::Modified);

    // 4. Cloned from expected remote -> Clean
    std::fs::remove_dir_all(vault.join("deps/tool")).unwrap();
    catasto_git::clone_repo(&upstream.url(), &vault.join("deps/tool"), Some("main")).unwrap();
    let rep = check(&vault).unwrap();
    assert!(rep.is_ok(), "{:?}", rep.violations);
}

#[test]
fn zone_clean_workspace_skips_git_and_respects_ignore() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(
        root.join("catasto.ctmt"),
        "\
::kind(catasto, version: \"1.0.0\")
::zone(
  \".\": (
    require: (\"Cargo.toml\")
    allow: (\"catasto.ctmt\", \"Cargo.toml\", \"crates\", \"target\")
    ignore: (\"target\")
  )
  \"crates/<?kebab(crate)>/src\": (
    require: (\"lib.rs\")
    allow: (\"<?snake>.rs\", \"mod.rs\")
    deny: (\"<?>.bak\")
    ignore: (\"generated\")
  )
)
",
    )
    .unwrap();

    std::fs::create_dir_all(root.join(".git/objects")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::write(root.join("target/debug/bad.bak"), "ignored").unwrap();

    let src = root.join("crates/my-crate/src");
    std::fs::create_dir_all(src.join("generated")).unwrap();
    std::fs::write(src.join("lib.rs"), "").unwrap();
    std::fs::write(src.join("mod.rs"), "").unwrap();
    std::fs::write(src.join("helper_fn.rs"), "").unwrap();
    std::fs::write(src.join("generated/NotSnake.bak"), "ignored").unwrap();

    // Non-kebab crate directory is not matched by the zone target pattern.
    let non_kebab_src = root.join("crates/not_kebab/src");
    std::fs::create_dir_all(&non_kebab_src).unwrap();
    std::fs::write(non_kebab_src.join("Anything.bak"), "ignored").unwrap();

    let rep = check(root).unwrap();
    assert!(rep.is_ok(), "unexpected violations: {:?}", rep.violations);
}

#[test]
fn zone_detects_require_allow_and_deny_violations() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(
        root.join("catasto.ctmt"),
        "\
::zone(
  \".\": (
    require: (\"README.md\")
  )
  \"crates/<?kebab(crate)>/src\": (
    require: (\"lib.rs\")
    allow: (\"<?snake>.rs\", \"mod.rs\")
    deny: (\"<?>.bak\")
    ignore: (\"generated\")
  )
)
",
    )
    .unwrap();

    let src = root.join("crates/alpha-pkg/src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("BadName.rs"), "").unwrap();
    std::fs::write(src.join("old.bak"), "").unwrap();

    let rep = check(root).unwrap();
    assert!(!rep.is_ok());

    assert_eq!(
        rep.violations,
        vec![
            Violation {
                path: PathBuf::from("."),
                kind: ViolationKind::ZoneMissingRequired {
                    zone: ".".into(),
                    pattern: "README.md".into(),
                },
            },
            Violation {
                path: PathBuf::from("crates/alpha-pkg/src"),
                kind: ViolationKind::ZoneMissingRequired {
                    zone: "crates/<?kebab(crate)>/src".into(),
                    pattern: "lib.rs".into(),
                },
            },
            Violation {
                path: PathBuf::from("crates/alpha-pkg/src/BadName.rs"),
                kind: ViolationKind::ZoneDisallowed {
                    zone: "crates/<?kebab(crate)>/src".into(),
                },
            },
            Violation {
                path: PathBuf::from("crates/alpha-pkg/src/old.bak"),
                kind: ViolationKind::ZoneDisallowed {
                    zone: "crates/<?kebab(crate)>/src".into(),
                },
            },
            Violation {
                path: PathBuf::from("crates/alpha-pkg/src/old.bak"),
                kind: ViolationKind::ZoneDenied {
                    zone: "crates/<?kebab(crate)>/src".into(),
                    pattern: "<?>.bak".into(),
                },
            },
        ]
    );
}

#[test]
fn zone_empty_allow_and_ignored_required_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(
        root.join("catasto.ctmt"),
        "\
::zone(
  \"empty\": (allow: (), ignore: (\".keep\"))
  \"req\": (require: (\"must.txt\"), ignore: (\"must.txt\"))
)
",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("empty")).unwrap();
    std::fs::write(root.join("empty/.keep"), "").unwrap();
    std::fs::write(root.join("empty/stray.txt"), "").unwrap();

    std::fs::create_dir_all(root.join("req")).unwrap();
    // Ignored entry does not satisfy `require`.
    std::fs::write(root.join("req/must.txt"), "").unwrap();

    let rep = check(root).unwrap();
    assert_eq!(
        rep.violations,
        vec![
            Violation {
                path: PathBuf::from("empty/stray.txt"),
                kind: ViolationKind::ZoneDisallowed {
                    zone: "empty".into(),
                },
            },
            Violation {
                path: PathBuf::from("req"),
                kind: ViolationKind::ZoneMissingRequired {
                    zone: "req".into(),
                    pattern: "must.txt".into(),
                },
            },
        ]
    );

    std::fs::remove_file(root.join("empty/stray.txt")).unwrap();
    std::fs::write(
        root.join("catasto.ctmt"),
        "::zone(\"empty\": (allow: (), ignore: (\".keep\")))\n",
    )
    .unwrap();
    assert!(check(root).unwrap().is_ok());
}

#[test]
fn zone_multiple_matching_zones_and_zero_matching_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(
        root.join("catasto.ctmt"),
        "\
::zone(
  \"crates/<?kebab>/src\": (require: \"lib.rs\")
  \"crates/foo/src\": (deny: \"secret.txt\")
  \"nonexistent/<?kebab>\": (require: \"README.md\")
)
",
    )
    .unwrap();

    let foo_src = root.join("crates/foo/src");
    std::fs::create_dir_all(&foo_src).unwrap();
    std::fs::write(foo_src.join("lib.rs"), "").unwrap();
    std::fs::write(foo_src.join("secret.txt"), "").unwrap();

    let rep = check(root).unwrap();
    assert_eq!(
        rep.violations,
        vec![Violation {
            path: PathBuf::from("crates/foo/src/secret.txt"),
            kind: ViolationKind::ZoneDenied {
                zone: "crates/foo/src".into(),
                pattern: "secret.txt".into(),
            },
        }]
    );
}

#[test]
#[cfg(unix)]
fn zone_does_not_recurse_into_symlinked_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let external = tempfile::tempdir().unwrap();
    std::fs::write(external.path().join("bad.bak"), "").unwrap();

    std::fs::write(
        root.join("catasto.ctmt"),
        "::zone(\"sub/<?seg>\": (deny: \"<?>.bak\"))\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::os::unix::fs::symlink(external.path(), root.join("sub/link")).unwrap();

    let rep = check(root).unwrap();
    assert!(rep.is_ok(), "{:?}", rep.violations);
}

#[test]
fn zone_violations_do_not_corrupt_managed_list_status() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let src_dir = tmp.path().join("src");
    std::fs::create_dir_all(vault.join("notes")).unwrap();
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("a.txt"), "hello").unwrap();
    std::fs::write(vault.join("notes/a.txt"), "hello").unwrap();
    std::fs::write(
        vault.join("catasto.ctmt"),
        "\
::kind(catasto, version: \"1.0.0\")
::zone(\"notes\": (deny: \"a.txt\"))

\"notes/a.txt\": catasto.local(\"../src/a.txt\")
",
    )
    .unwrap();
    lock(&vault).unwrap();

    let rep = check(&vault).unwrap();
    assert_eq!(rep.violations.len(), 1);
    assert!(rep.violations[0].kind.is_zone());

    let rows = crate::manage::list(&vault).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, None);
}

#[test]
fn zone_recursive_propagates_without_double_evaluation() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(
        root.join("catasto.ctmt"),
        "\
::zone(
  \"nix\": (
    recursive: true
    files: (allow: (\"<?kebab>.nix\"))
    dirs: (allow: (\"<?kebab>\"))
  )
  \"flat\": (
    files: (allow: (\"<?kebab>.txt\"))
  )
  \"multi/<?tree>\": (
    recursive: true
    deny: (\"<?>.tmp\")
  )
)
",
    )
    .unwrap();

    // Valid nested recursive tree + invalid entries in nested subdir
    std::fs::create_dir_all(root.join("nix/pkgs/sub")).unwrap();
    std::fs::write(root.join("nix/default.nix"), "").unwrap();
    std::fs::write(root.join("nix/pkgs/catasto.nix"), "").unwrap();
    std::fs::write(root.join("nix/pkgs/sub/BadFile.nix"), "").unwrap();

    // Non-recursive zone does not propagate into `flat/nested`
    std::fs::create_dir_all(root.join("flat/nested")).unwrap();
    std::fs::write(root.join("flat/ok.txt"), "").unwrap();
    std::fs::write(root.join("flat/nested/BadName.bin"), "").unwrap();

    // Both inherited and target-matched on `multi/a/b`: evaluated only once
    std::fs::create_dir_all(root.join("multi/a/b")).unwrap();
    std::fs::write(root.join("multi/a/b/scratch.tmp"), "").unwrap();

    let rep = check(root).unwrap();
    assert_eq!(
        rep.violations,
        vec![
            Violation {
                path: PathBuf::from("multi/a/b/scratch.tmp"),
                kind: ViolationKind::ZoneDenied {
                    zone: "multi/<?tree>".into(),
                    pattern: "<?>.tmp".into(),
                },
            },
            Violation {
                path: PathBuf::from("nix/pkgs/sub/BadFile.nix"),
                kind: ViolationKind::ZoneDisallowed { zone: "nix".into() },
            },
        ]
    );
}

#[test]
fn zone_files_and_dirs_apply_by_entry_kind() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(
        root.join("catasto.ctmt"),
        "\
::zone(
  \"docs\": (
    deny: (\"secret\")
    files: (
      require: (\"index.md\")
      allow: (\"<?kebab>.md\")
      deny: (\"draft.md\")
    )
    dirs: (
      require: (\"assets\")
      allow: (\"<?snake>\")
      deny: (\"tmp_dir\")
    )
  )
)
",
    )
    .unwrap();

    let docs = root.join("docs");
    // A directory named `index.md` does NOT satisfy `files.require`, and a file named `assets`
    // does NOT satisfy `dirs.require`.
    std::fs::create_dir_all(docs.join("index.md")).unwrap();
    std::fs::create_dir_all(docs.join("tmp_dir")).unwrap();
    std::fs::create_dir_all(docs.join("BadDir")).unwrap();
    std::fs::write(docs.join("assets"), "").unwrap();
    std::fs::write(docs.join("draft.md"), "").unwrap();
    std::fs::write(docs.join("secret"), "").unwrap();

    let rep = check(root).unwrap();
    assert_eq!(
        rep.violations,
        vec![
            // Top-level deny hits `secret`
            Violation {
                path: PathBuf::from("docs/secret"),
                kind: ViolationKind::ZoneDenied {
                    zone: "docs".into(),
                    pattern: "secret".into(),
                },
            },
            // `files.require` not satisfied by dir `index.md`
            Violation {
                path: PathBuf::from("docs"),
                kind: ViolationKind::ZoneMissingRequired {
                    zone: "docs".into(),
                    pattern: "index.md".into(),
                },
            },
            // `files.allow` rejects `assets`, `secret`
            Violation {
                path: PathBuf::from("docs/assets"),
                kind: ViolationKind::ZoneDisallowed {
                    zone: "docs".into(),
                },
            },
            Violation {
                path: PathBuf::from("docs/secret"),
                kind: ViolationKind::ZoneDisallowed {
                    zone: "docs".into(),
                },
            },
            // `files.deny` rejects `draft.md`
            Violation {
                path: PathBuf::from("docs/draft.md"),
                kind: ViolationKind::ZoneDenied {
                    zone: "docs".into(),
                    pattern: "draft.md".into(),
                },
            },
            // `dirs.require` not satisfied by file `assets`
            Violation {
                path: PathBuf::from("docs"),
                kind: ViolationKind::ZoneMissingRequired {
                    zone: "docs".into(),
                    pattern: "assets".into(),
                },
            },
            // `dirs.allow` rejects `BadDir`, `index.md`
            Violation {
                path: PathBuf::from("docs/BadDir"),
                kind: ViolationKind::ZoneDisallowed {
                    zone: "docs".into(),
                },
            },
            Violation {
                path: PathBuf::from("docs/index.md"),
                kind: ViolationKind::ZoneDisallowed {
                    zone: "docs".into(),
                },
            },
            // `dirs.deny` rejects `tmp_dir`
            Violation {
                path: PathBuf::from("docs/tmp_dir"),
                kind: ViolationKind::ZoneDenied {
                    zone: "docs".into(),
                    pattern: "tmp_dir".into(),
                },
            },
        ]
    );
}

#[test]
fn zone_ignore_stops_recursive_inheritance_per_zone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(
        root.join("catasto.ctmt"),
        "\
::zone(
  \"solo\": (
    recursive: true
    allow: (\"<?kebab>.txt\", \"ok\")
    ignore: (\"vendor\")
  )
  \"shared\": (
    recursive: true
    files: (allow: (\"<?kebab>.txt\"))
    ignore: (\"vendor\")
  )
  \"shared/ok\": (
    dirs: (allow: (\"vendor\"))
  )
  \"shared/ok/vendor\": (
    files: (allow: (\"<?snake>.bin\"))
  )
)
",
    )
    .unwrap();

    // `solo/vendor` and its descendants are completely skipped by `solo`
    std::fs::create_dir_all(root.join("solo/vendor/deep")).unwrap();
    std::fs::write(root.join("solo/vendor/Bad.bin"), "").unwrap();
    std::fs::write(root.join("solo/vendor/deep/Bad.bin"), "").unwrap();
    std::fs::create_dir_all(root.join("solo/ok")).unwrap();
    std::fs::write(root.join("solo/ok/valid.txt"), "").unwrap();

    // `shared/vendor` is ignored by `shared` (the only active zone on `shared`), so traversal
    // skips `shared/vendor` altogether.
    std::fs::create_dir_all(root.join("shared/vendor")).unwrap();
    std::fs::write(root.join("shared/vendor/ANYTHING.bin"), "").unwrap();

    // At `shared/ok`, both `shared` (inherited, ignores `vendor`) and `shared/ok` (does not ignore
    // `vendor`) are active. Descending into `shared/ok/vendor` must NOT inherit `shared`, while
    // still evaluating the `shared/ok/vendor` zone itself.
    std::fs::create_dir_all(root.join("shared/ok/vendor")).unwrap();
    std::fs::write(root.join("shared/ok/vendor/good_data.bin"), "").unwrap();

    let rep = check(root).unwrap();
    assert!(rep.is_ok(), "unexpected violations: {:?}", rep.violations);

    std::fs::write(root.join("shared/ok/vendor/Bad.bin"), "").unwrap();
    let rep = check(root).unwrap();
    assert_eq!(
        rep.violations,
        vec![Violation {
            path: PathBuf::from("shared/ok/vendor/Bad.bin"),
            kind: ViolationKind::ZoneDisallowed {
                zone: "shared/ok/vendor".into(),
            },
        }]
    );
}
