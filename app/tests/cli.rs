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
            vault.join("catasto.ctmt"),
            "\"schema/writ.vocabulary.tmt\": { source: \"../vocabularies/writ.vocabulary.tmt\" }\n",
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

/// A vault whose one file comes from the `main` branch of a local git repository.
struct GitWorkspace {
    _tmp: tempfile::TempDir,
    vault: std::path::PathBuf,
    cache: std::path::PathBuf,
    remote: std::path::PathBuf,
}

impl GitWorkspace {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        let remote = tmp.path().join("remote");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::create_dir_all(&remote).unwrap();
        let ws = Self {
            vault,
            cache: tmp.path().join("cache"),
            remote,
            _tmp: tmp,
        };
        ws.git(&["init", "--quiet", "-b", "main"]);
        ws.commit("one");
        std::fs::write(
            ws.vault.join("catasto.ctmt"),
            format!(
                "\"schema/w.tmt\": {{ source: {{ git: {:?}, rev: main, path: \"w.tmt\" }} }}\n",
                ws.remote
            ),
        )
        .unwrap();
        ws
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(&self.remote)
            .args([
                "-c",
                "user.name=catasto-test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}");
    }

    fn commit(&self, content: &str) {
        std::fs::write(self.remote.join("w.tmt"), content).unwrap();
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "-m", content]);
    }

    #[cfg(unix)]
    fn commit_symlink(&self, path: &str, target: &str) {
        let link = self.remote.join(path);
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(target, &link).unwrap();
        self.git(&["add", "--all"]);
        self.git(&[
            "commit",
            "--quiet",
            "-m",
            &format!("link {path} -> {target}"),
        ]);
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_catasto"))
            .arg("-C")
            .arg(&self.vault)
            .args(args)
            .env("CATASTO_CACHE_DIR", &self.cache)
            .output()
            .unwrap()
    }

    fn content(&self) -> String {
        std::fs::read_to_string(self.vault.join("schema/w.tmt")).unwrap()
    }
}

#[test]
fn git_sync_pins_a_commit_and_check_works_offline() {
    let ws = GitWorkspace::new();

    assert_eq!(ws.run(&["sync"]).status.code(), Some(0));
    assert_eq!(ws.content(), "one");
    let lock = std::fs::read_to_string(ws.vault.join("catasto.lock")).unwrap();
    assert!(lock.contains("\"commit\": \""), "{lock}");

    std::fs::remove_dir_all(&ws.remote).unwrap();
    assert_eq!(ws.run(&["check"]).status.code(), Some(0));
}

#[test]
fn git_sync_stays_pinned_until_update() {
    let ws = GitWorkspace::new();
    ws.run(&["sync"]);
    ws.commit("two");

    assert_eq!(ws.run(&["sync"]).status.code(), Some(0));
    assert_eq!(ws.content(), "one");

    assert_eq!(ws.run(&["sync", "--update"]).status.code(), Some(0));
    assert_eq!(ws.content(), "two");
    assert_eq!(ws.run(&["check"]).status.code(), Some(0));
}

#[test]
fn git_sync_reports_an_unknown_rev() {
    let ws = GitWorkspace::new();
    let manifest = std::fs::read_to_string(ws.vault.join("catasto.ctmt")).unwrap();
    std::fs::write(
        ws.vault.join("catasto.ctmt"),
        manifest.replace("rev: main", "rev: nope"),
    )
    .unwrap();

    let synced = ws.run(&["sync"]);

    assert_eq!(synced.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&synced.stderr);
    assert!(stderr.contains("source of schema/w.tmt"), "{stderr}");
    assert!(!ws.vault.join("schema/w.tmt").exists());
}

#[test]
#[cfg(unix)]
fn e2e_git_symlink_follow() {
    let ws = GitWorkspace::new();
    ws.commit_symlink("link.txt", "w.tmt");
    std::fs::write(
        ws.vault.join("catasto.ctmt"),
        format!(
            "\"followed.txt\": {{ source: {{ git: {:?}, rev: main, path: \"link.txt\" }} }}\n",
            ws.remote
        ),
    )
    .unwrap();

    let synced = ws.run(&["sync"]);
    assert_eq!(synced.status.code(), Some(0));
    let out = ws.vault.join("followed.txt");
    let meta = std::fs::symlink_metadata(&out).unwrap();
    assert!(meta.is_file());
    assert!(!meta.is_symlink());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "one");

    let checked = ws.run(&["check"]);
    assert_eq!(checked.status.code(), Some(0));

    let listed = ws.run(&["list"]);
    assert_eq!(listed.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(stdout.contains("followed.txt"), "{stdout}");
    assert!(stdout.contains("ok"), "{stdout}");
}

#[test]
#[cfg(unix)]
fn e2e_git_symlink_keep() {
    let ws = GitWorkspace::new();
    ws.commit_symlink("link.txt", "target.txt");
    std::fs::write(
        ws.vault.join("catasto.ctmt"),
        format!(
            "\"kept.txt\": {{ source: {{ git: {:?}, rev: main, path: \"link.txt\", symlink: keep }} }}\n",
            ws.remote
        ),
    )
    .unwrap();

    let synced = ws.run(&["sync"]);
    assert_eq!(synced.status.code(), Some(0));
    let out = ws.vault.join("kept.txt");
    let meta = std::fs::symlink_metadata(&out).unwrap();
    assert!(meta.is_symlink());
    assert_eq!(
        std::fs::read_link(&out).unwrap().to_str().unwrap(),
        "target.txt"
    );

    let checked = ws.run(&["check"]);
    assert_eq!(checked.status.code(), Some(0));

    let listed = ws.run(&["list"]);
    assert_eq!(listed.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(stdout.contains("kept.txt"), "{stdout}");
    assert!(stdout.contains("symlink: keep"), "{stdout}");
    assert!(stdout.contains("ok"), "{stdout}");

    let listed_json = ws.run(&["list", "--json"]);
    assert_eq!(listed_json.status.code(), Some(0));
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&listed_json.stdout)).unwrap();
    assert_eq!(json[0]["symlink"], "keep");
}
