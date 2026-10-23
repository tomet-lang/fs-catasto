//! Integration tests for `catasto`.

use std::process::Command;

#[test]
fn help_describes_catasto() {
    let output = Command::new(env!("CARGO_BIN_EXE_catasto"))
        .arg("--help")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("catasto"), "{help}");
    assert!(help.contains("check"), "{help}");
}

#[test]
fn check_current_directory_exits_0() {
    let output = Command::new(env!("CARGO_BIN_EXE_catasto"))
        .args(["check", "."])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
}

/// A vault at `<tmp>/vault` declaring one file sourced from `<tmp>/vocabularies`.
struct Workspace {
    _tmp: tempfile::TempDir,
    vault: std::path::PathBuf,
    source: std::path::PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        let source = tmp.path().join("vocabularies/writ.vocabulary.tmt");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(
            vault.join("catasto.toml"),
            "[[managed]]\n\
             path = \"schema/writ.vocabulary.tmt\"\n\
             source = { path = \"../vocabularies/writ.vocabulary.tmt\" }\n",
        )
        .unwrap();
        std::fs::write(&source, "canonical").unwrap();
        Self {
            _tmp: tmp,
            vault,
            source,
        }
    }

    fn run(&self, subcommand: &str) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_catasto"))
            .arg(subcommand)
            .arg(&self.vault)
            .output()
            .unwrap()
    }

    fn managed_file(&self) -> std::path::PathBuf {
        self.vault.join("schema/writ.vocabulary.tmt")
    }
}

#[test]
fn sync_fetches_then_check_passes() {
    let ws = Workspace::new();

    let synced = ws.run("sync");

    assert_eq!(synced.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&synced.stdout),
        "synced 1 file(s)\n"
    );
    assert_eq!(
        std::fs::read_to_string(ws.managed_file()).unwrap(),
        "canonical"
    );
    assert!(ws.vault.join("catasto.lock").is_file());
    assert_eq!(ws.run("check").status.code(), Some(0));
}

#[test]
fn check_reports_a_modified_file_and_sync_restores_it() {
    let ws = Workspace::new();
    ws.run("sync");
    std::fs::write(ws.managed_file(), "edited").unwrap();

    let checked = ws.run("check");

    assert_eq!(checked.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(
        stderr.contains("schema/writ.vocabulary.tmt: modified"),
        "{stderr}"
    );

    assert_eq!(ws.run("sync").status.code(), Some(0));
    assert_eq!(ws.run("check").status.code(), Some(0));
}

#[test]
fn check_reports_a_stale_file_when_the_source_changes() {
    let ws = Workspace::new();
    ws.run("sync");
    std::fs::write(&ws.source, "canonical v2").unwrap();

    let checked = ws.run("check");

    assert_eq!(checked.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(stderr.contains("stale"), "{stderr}");
}

#[test]
fn lock_accepts_a_local_edit() {
    let ws = Workspace::new();
    ws.run("sync");
    std::fs::write(&ws.source, "canonical v2").unwrap();
    std::fs::write(ws.managed_file(), "mine").unwrap();

    let locked = ws.run("lock");

    assert_eq!(locked.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&locked.stdout),
        "locked 1 file(s)\n"
    );
    // The edit is now the locked content, so only the source disagrees with it.
    let stderr = String::from_utf8_lossy(&ws.run("check").stderr).into_owned();
    assert!(stderr.contains("stale"), "{stderr}");
    assert!(!stderr.contains("modified"), "{stderr}");
}

#[test]
fn sync_fails_when_the_source_is_missing() {
    let ws = Workspace::new();
    std::fs::remove_file(&ws.source).unwrap();

    let synced = ws.run("sync");

    assert_eq!(synced.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&synced.stderr);
    assert!(
        stderr.starts_with("catasto: source of schema/writ.vocabulary.tmt"),
        "{stderr}"
    );
}
