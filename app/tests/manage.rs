//! `init`, `add`, `remove`, `upstream` and `list` as a person runs them.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A vault, a git repository with two files, and a cache of its own.
struct World {
    _tmp: tempfile::TempDir,
    vault: PathBuf,
    remote: PathBuf,
    cache: PathBuf,
}

impl World {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        let remote = tmp.path().join("shared");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::create_dir_all(remote.join("sub")).unwrap();
        let world = Self {
            vault,
            cache: tmp.path().join("cache"),
            remote,
            _tmp: tmp,
        };
        world.git(&["init", "--quiet", "-b", "main"]);
        world.commit("a.tmt", "a one");
        world.commit("sub/b.tmt", "b one");
        world
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

    fn commit(&self, path: &str, content: &str) {
        std::fs::write(self.remote.join(path), content).unwrap();
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
        self.git(&["commit", "--quiet", "-m", &format!("link {path} -> {target}")]);
    }

    /// `catasto <args>` in the vault.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_catasto"))
            .args(args)
            .current_dir(&self.vault)
            .env("CATASTO_CACHE_DIR", &self.cache)
            .output()
            .unwrap()
    }

    /// Runs and expects success; returns standard output.
    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Runs and expects a failure; returns standard error.
    fn fails(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(!output.status.success(), "{args:?} should have failed");
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    fn url(&self) -> &str {
        self.remote.to_str().unwrap()
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.vault.join(path)).unwrap()
    }

    fn exists(&self, path: impl AsRef<Path>) -> bool {
        self.vault.join(path).exists()
    }

    /// `init` and one named place.
    fn ready(&self) {
        self.ok(&["init"]);
        self.ok(&["upstream", "add", "shared", self.url()]);
    }
}

#[test]
fn from_nothing_to_a_checked_file_with_four_commands() {
    let w = World::new();

    assert!(w.ok(&["init"]).contains("created"));
    assert!(
        w.ok(&["upstream", "add", "shared", w.url()])
            .contains("added upstream shared")
    );
    let added = w.ok(&["add", "docs/a.tmt", "--upstream", "shared", "a.tmt"]);
    assert!(added.starts_with("added docs/a.tmt, locked at "), "{added}");

    assert_eq!(w.read("docs/a.tmt"), "a one");
    assert!(w.exists("catasto.lock"));
    assert!(w.run(&["check"]).status.success());

    let listed = w.ok(&["list"]);
    assert!(listed.contains("docs/a.tmt"), "{listed}");
    assert!(listed.contains("shared:a.tmt"), "{listed}");
    assert!(listed.contains("ok"), "{listed}");
}

#[test]
fn the_comments_the_manifest_opened_with_survive_every_edit() {
    let w = World::new();
    w.ready();
    w.ok(&["add", "a.tmt", "--upstream", "shared", "a.tmt"]);
    w.ok(&["add", "b.tmt", "--upstream", "shared", "sub/b.tmt"]);
    w.ok(&["remove", "a.tmt"]);

    let manifest = w.read("catasto.ctmt");
    assert!(
        manifest.starts_with("// The files catasto manages"),
        "{manifest}"
    );
    assert!(manifest.contains("::upstreams{"), "{manifest}");
    assert!(manifest.contains("\"b.tmt\""), "{manifest}");
    assert!(!manifest.contains("\"a.tmt\""), "{manifest}");
}

#[test]
fn a_file_can_also_come_from_a_repository_written_out_or_from_disk() {
    let w = World::new();
    w.ok(&["init"]);
    w.ok(&["add", "x.tmt", "--git", w.url(), "--rev", "main", "a.tmt"]);
    std::fs::create_dir_all(w.vault.join("../local")).unwrap();
    std::fs::write(w.vault.join("../local/l.tmt"), "local").unwrap();
    w.ok(&["add", "l.tmt", "--local", "../local/l.tmt"]);

    assert_eq!(w.read("x.tmt"), "a one");
    assert_eq!(w.read("l.tmt"), "local");
    let listed = w.ok(&["list"]);
    assert!(listed.contains("local ../local/l.tmt"), "{listed}");
    assert!(listed.contains("git "), "{listed}");
    assert!(w.run(&["check"]).status.success());
}

#[test]
fn a_failed_add_leaves_nothing_behind() {
    let w = World::new();
    w.ready();
    let before = w.read("catasto.ctmt");

    let error = w.fails(&["add", "nope.tmt", "--upstream", "shared", "no/such.tmt"]);
    assert!(error.contains("could not add nope.tmt"), "{error}");

    assert_eq!(w.read("catasto.ctmt"), before);
    assert!(!w.exists("nope.tmt"));
    assert!(!w.exists("catasto.lock") || !w.read("catasto.lock").contains("nope"));
}

#[test]
fn an_unknown_upstream_names_the_ones_there_are() {
    let w = World::new();
    w.ready();
    let error = w.fails(&["add", "a.tmt", "--upstream", "other", "a.tmt"]);
    assert!(
        error.contains("unknown upstream `other` (declared: shared)"),
        "{error}"
    );
}

#[test]
fn add_needs_to_know_where_from() {
    let w = World::new();
    w.ready();
    let error = w.fails(&["add", "a.tmt", "a.tmt"]);
    assert!(error.contains("--upstream"), "{error}");
    let error = w.fails(&[
        "add",
        "a.tmt",
        "--upstream",
        "shared",
        "--git",
        w.url(),
        "a.tmt",
    ]);
    assert!(error.contains("cannot be used with"), "{error}");
}

#[test]
fn add_defaults_path_to_dest_for_upstream_and_git_and_nested_dest() {
    let w = World::new();
    w.ready();

    // --upstream without PATH defaults to dest
    w.ok(&["add", "a.tmt", "--upstream", "shared"]);
    assert_eq!(w.read("a.tmt"), "a one");
    let manifest = w.read("catasto.ctmt");
    assert!(manifest.contains("\"a.tmt\":"), "{manifest}");
    assert!(manifest.contains("path: \"a.tmt\""), "{manifest}");
    assert!(manifest.contains("upstream: shared"), "{manifest}");
    let listed = w.ok(&["list"]);
    assert!(listed.contains("shared:a.tmt"), "{listed}");

    // nested dest (including leading ./) defaults to same nested path
    w.ok(&["add", "./sub/b.tmt", "--upstream", "shared"]);
    assert_eq!(w.read("sub/b.tmt"), "b one");
    let manifest = w.read("catasto.ctmt");
    assert!(manifest.contains("\"sub/b.tmt\":"), "{manifest}");
    assert!(manifest.contains("path: \"sub/b.tmt\""), "{manifest}");
    let listed = w.ok(&["list"]);
    assert!(listed.contains("shared:sub/b.tmt"), "{listed}");

    // --git without PATH defaults to dest
    let w_git = World::new();
    w_git.ok(&["init"]);
    w_git.ok(&["add", "a.tmt", "--git", w_git.url(), "--rev", "main"]);
    assert_eq!(w_git.read("a.tmt"), "a one");
    let manifest_git = w_git.read("catasto.ctmt");
    assert!(manifest_git.contains("path: \"a.tmt\""), "{manifest_git}");
}

#[test]
fn add_with_local_requires_path_and_leaves_manifest_and_lock_untouched() {
    let w = World::new();
    w.ready();
    w.ok(&["add", "a.tmt", "--upstream", "shared", "a.tmt"]);

    let manifest_before = w.read("catasto.ctmt");
    let lock_before = w.read("catasto.lock");

    let error = w.fails(&["add", "local.tmt", "--local"]);
    assert!(
        error.contains("--local needs the path of the file to take it from"),
        "{error}"
    );
    assert_eq!(w.read("catasto.ctmt"), manifest_before);
    assert_eq!(w.read("catasto.lock"), lock_before);
    assert!(!w.exists("local.tmt"));
}

#[test]
fn list_says_json_and_how_a_file_stands() {
    let w = World::new();
    w.ready();
    w.ok(&["add", "a.tmt", "--upstream", "shared", "a.tmt"]);
    std::fs::write(w.vault.join("a.tmt"), "edited").unwrap();

    let json: serde_json::Value = serde_json::from_str(&w.ok(&["list", "--json"])).unwrap();
    assert_eq!(json[0]["path"], "a.tmt");
    assert_eq!(json[0]["from"], "shared:a.tmt");
    assert_eq!(json[0]["state"], "modified");
    assert_eq!(json[0]["commit"].as_str().unwrap().len(), 40);
}

#[test]
fn check_says_the_command_that_puts_a_file_right() {
    let w = World::new();
    w.ready();
    w.ok(&["add", "a.tmt", "--upstream", "shared", "a.tmt"]);
    std::fs::write(w.vault.join("a.tmt"), "edited").unwrap();
    let error = w.fails(&["check"]);
    assert!(error.contains("a.tmt: modified"), "{error}");
    assert!(error.contains("`catasto sync` restores it"), "{error}");
    assert_eq!(error.lines().count(), 1, "one violation, one line: {error}");

    std::fs::remove_file(w.vault.join("a.tmt")).unwrap();
    let error = w.fails(&["check"]);
    assert!(
        error.contains("missing") && error.contains("run `catasto sync`"),
        "{error}"
    );
}

#[test]
fn sync_only_moves_the_file_it_is_told_to() {
    let w = World::new();
    w.ready();
    w.ok(&["add", "a.tmt", "--upstream", "shared", "a.tmt"]);
    w.ok(&["add", "b.tmt", "--upstream", "shared", "sub/b.tmt"]);
    w.commit("a.tmt", "a two");
    w.commit("sub/b.tmt", "b two");

    let synced = w.ok(&["sync", "--update", "--only", "a.tmt"]);
    assert_eq!(synced, "synced 1 of 2 file(s)\n");
    assert_eq!(w.read("a.tmt"), "a two");
    assert_eq!(w.read("b.tmt"), "b one");
    assert!(w.run(&["check"]).status.success());

    let error = w.fails(&["sync", "--only", "z.tmt"]);
    assert!(error.contains("z.tmt is not declared"), "{error}");
}

#[test]
fn remove_keeps_the_file_unless_told_and_upstream_remove_waits_for_it() {
    let w = World::new();
    w.ready();
    w.ok(&["add", "a.tmt", "--upstream", "shared", "a.tmt"]);

    let error = w.fails(&["upstream", "remove", "shared"]);
    assert!(error.contains("used by a.tmt"), "{error}");

    assert!(w.ok(&["remove", "a.tmt"]).contains("still on disk"));
    assert!(w.exists("a.tmt"));
    assert!(w.run(&["check"]).status.success());
    w.ok(&["upstream", "remove", "shared"]);
    assert_eq!(w.ok(&["upstream", "list"]), "");

    w.ok(&["upstream", "add", "shared", w.url()]);
    w.ok(&["add", "c.tmt", "--upstream", "shared", "a.tmt"]);
    assert!(w.ok(&["remove", "c.tmt", "--delete"]).contains("deleted"));
    assert!(!w.exists("c.tmt"));
}

#[test]
fn the_workspace_can_be_given_with_c_or_as_a_trailing_path() {
    let w = World::new();
    w.ready();
    let elsewhere = w.vault.parent().unwrap();
    let vault = w.vault.to_str().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_catasto"))
            .args(args)
            .current_dir(elsewhere)
            .env("CATASTO_CACHE_DIR", &w.cache)
            .output()
            .unwrap()
    };

    assert!(run(&["-C", vault, "list"]).status.success());
    assert!(run(&["check", vault]).status.success());
    assert!(run(&["-C", vault, "check", vault]).status.success());
    let output = run(&["-C", vault, "check", "/"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("given twice"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_manifest_spelled_in_another_case_is_named_not_ignored() {
    let w = World::new();
    std::fs::write(w.vault.join("Catasto.ctmt"), "").unwrap();
    for args in [&["init"][..], &["check"], &["list"], &["sync"]] {
        let error = w.fails(args);
        assert!(
            error.contains("found Catasto.ctmt") && error.contains("rename it"),
            "{args:?}: {error}"
        );
    }
}

#[test]
fn init_does_not_overwrite_and_without_a_manifest_the_editing_commands_say_what_to_run() {
    let w = World::new();
    let error = w.fails(&["add", "a.tmt", "--local", "x"]);
    assert!(error.contains("catasto init"), "{error}");
    w.ok(&["init"]);
    std::fs::write(w.vault.join("catasto.ctmt"), "// mine\n").unwrap();
    let error = w.fails(&["init"]);
    assert!(error.contains("already exists"), "{error}");
    assert_eq!(w.read("catasto.ctmt"), "// mine\n");
}

#[test]
fn help_lists_the_commands() {
    let w = World::new();
    let help = w.ok(&["--help"]);
    for command in [
        "init", "add", "remove", "upstream", "list", "check", "lock", "sync",
    ] {
        assert!(help.contains(command), "{command}: {help}");
    }
}

#[test]
fn add_with_local_and_keep_symlink_is_rejected() {
    let w = World::new();
    w.ready();
    std::fs::write(w.vault.join("local.txt"), "content").unwrap();
    let error = w.fails(&["add", "link.txt", "local.txt", "--local", "--keep-symlink"]);
    assert!(
        error.contains("--local has no --keep-symlink option")
            || error.contains("a local source has no such option"),
        "{error}"
    );
}

#[test]
#[cfg(unix)]
fn add_with_keep_symlink_e2e() {
    let w = World::new();
    w.ready();
    w.commit_symlink("link.tmt", "a.tmt");

    let out = w.ok(&["add", "link.tmt", "--upstream", "shared", "--keep-symlink"]);
    assert!(out.contains("added link.tmt"));

    let link_path = w.vault.join("link.tmt");
    let meta = std::fs::symlink_metadata(&link_path).unwrap();
    assert!(meta.is_symlink());
    assert_eq!(
        std::fs::read_link(&link_path).unwrap().to_str().unwrap(),
        "a.tmt"
    );

    let manifest = w.read("catasto.ctmt");
    assert!(manifest.contains("symlink: keep"));

    let check = w.ok(&["check"]);
    assert!(check.is_empty() || check.contains("ok"));
}
