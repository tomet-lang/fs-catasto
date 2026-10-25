use super::*;
use crate::lockfile::lock;

const MANIFEST: &str = "\
\"vocab/writ.tmt\": { source: \"../vocabularies/writ.tmt\" }
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

const GIT_SOURCE: &str = "{ git: \"/does/not/exist.git\", rev: main, path: \"w.tmt\" }";

/// A vault with one git-sourced file whose lock entry records `commit`.
fn git_vault(commit: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.tmt"), "abc").unwrap();
    std::fs::write(
        dir.path().join("catasto.ctmt"),
        format!("\"w.tmt\": {{ source: {GIT_SOURCE} }}\n"),
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
            format!("\"link\": {{ source: {manifest_src} }}"),
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
        "{ git: \"https://example.com/v.git\", rev: main, path: \"link\", symlink: keep }";
    let default_manifest = "{ git: \"https://example.com/v.git\", rev: main, path: \"link\" }";
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
