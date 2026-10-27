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

    fn commit_executable(&self, path: &str, content: &str) {
        let file = self.remote.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&file, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        self.git(&["add", path]);
        self.git(&["update-index", "--chmod=+x", path]);
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

#[test]
fn add_force_cli_exit_codes_and_messages() {
    let w = World::new();
    w.ready();

    // 1. First add: exit code 0, prints "added ..."
    let out = w.ok(&["add", "a.tmt", "--upstream", "shared", "a.tmt"]);
    assert!(out.starts_with("added a.tmt"), "{out}");

    // 2. Second add without --force: exit code 3, error on stderr
    let output = w.run(&["add", "a.tmt", "--upstream", "shared", "a.tmt"]);
    assert_eq!(output.status.code(), Some(3));
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("a.tmt is already declared (use --force to replace its source)"),
        "{err}"
    );

    // 3. Add with --force and exact same source: exit code 0, prints "a.tmt: declared as given"
    let out = w.ok(&["add", "a.tmt", "--upstream", "shared", "a.tmt", "--force"]);
    assert!(out.contains("a.tmt: declared as given"), "{out}");

    // 4. Add with --force and different source: exit code 0, prints "replaced a.tmt"
    let out = w.ok(&["add", "a.tmt", "--upstream", "shared", "sub/b.tmt", "--force"]);
    assert!(out.starts_with("replaced a.tmt"), "{out}");
    assert_eq!(w.read("a.tmt"), "b one");
}

#[test]
fn upstream_add_force_cli_exit_codes_and_messages() {
    let w = World::new();
    w.ready(); // adds upstream "shared"
    w.ok(&["add", "a.tmt", "--upstream", "shared", "a.tmt"]);

    // 1. Without --force on existing upstream: exit code 3, error on stderr
    let output = w.run(&["upstream", "add", "shared", w.url()]);
    assert_eq!(output.status.code(), Some(3));
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("upstream `shared` is already declared (use --force to replace it)"),
        "{err}"
    );

    // 2. With --force and exact same definition: exit code 0, prints "upstream shared: declared as given"
    let out = w.ok(&["upstream", "add", "shared", w.url(), "--force"]);
    assert!(out.contains("upstream shared: declared as given"), "{out}");

    // 3. With --force and different definition (rev v2) when 1 file uses it:
    // prints "replaced upstream shared" and "1 file(s) use it; run `catasto sync` to take them from the new place"
    w.commit("a.tmt", "a two on v2");
    w.git(&["tag", "v2"]);
    let out = w.ok(&[
        "upstream", "add", "shared", w.url(), "--rev", "v2", "--force",
    ]);
    assert!(out.contains("replaced upstream shared"), "{out}");
    assert!(
        out.contains("1 file(s) use it; run `catasto sync` to take them from the new place"),
        "{out}"
    );

    // 4. With --force and different definition when NO file uses it:
    // Add unused upstream "unused"
    w.ok(&["upstream", "add", "unused", w.url()]);
    let out = w.ok(&[
        "upstream", "add", "unused", w.url(), "--rev", "v2", "--force",
    ]);
    assert!(out.contains("replaced upstream unused"), "{out}");
    assert!(
        !out.contains("file(s) use it; run `catasto sync` to take them from the new place"),
        "{out}"
    );
}

#[test]
fn diff_command_cli_contract() {
    let w = World::new();
    w.ready(); // adds upstream "shared"
    w.ok(&["add", "a.tmt", "--upstream", "shared", "a.tmt"]);
    w.ok(&["add", "b.tmt", "--upstream", "shared", "sub/b.tmt"]);

    // 1. Clean tree: prints nothing and exits 0
    let out = w.run(&["diff"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stdout.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    // 2. Hand-edited file: prints expected hunk and exits 1
    let tree_manifest_before = w.read("catasto.ctmt");
    let tree_lock_before = w.read("catasto.lock");

    let a_path = w.vault.join("a.tmt");
    std::fs::write(&a_path, "a modified\n").unwrap();
    let b_path = w.vault.join("b.tmt");
    std::fs::write(&b_path, "b modified\n").unwrap();

    let out = w.run(&["diff"]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--- a/a.tmt"), "{stdout}");
    assert!(stdout.contains("+++ b/a.tmt\tgit "), "{stdout}");
    assert!(stdout.contains("-a modified"), "{stdout}");
    assert!(stdout.contains("+a one"), "{stdout}");
    assert!(stdout.contains("--- a/b.tmt"), "{stdout}");
    assert!(stdout.contains("+++ b/b.tmt\tgit "), "{stdout}");
    assert!(stdout.contains("-b modified"), "{stdout}");
    assert!(stdout.contains("+b one"), "{stdout}");

    // Working tree is untouched after diff!
    assert_eq!(w.read("a.tmt"), "a modified\n");
    assert_eq!(w.read("b.tmt"), "b modified\n");
    assert_eq!(w.read("catasto.ctmt"), tree_manifest_before);
    assert_eq!(w.read("catasto.lock"), tree_lock_before);

    // 3. --only limits it
    let out = w.run(&["diff", "--only", "a.tmt"]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("a.tmt"), "{stdout}");
    assert!(!stdout.contains("b.tmt"), "{stdout}");

    // 4. Binary file prints the one-line notice
    let bin_path = w.remote.join("bin.dat");
    std::fs::write(&bin_path, [0xFF, 0x00, 0xFE, 0x01]).unwrap();
    w.git(&["add", "bin.dat"]);
    w.git(&["commit", "-m", "add bin"]);
    w.ok(&["add", "bin.dat", "--upstream", "shared", "bin.dat"]);

    std::fs::write(w.vault.join("bin.dat"), [0xAA, 0xBB]).unwrap();
    let out = w.run(&["diff", "--only", "bin.dat"]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "Binary files differ: bin.dat\n");
}

#[test]
fn sync_dry_run_cli_contract() {
    let w = World::new();
    w.ready(); // adds upstream "shared"
    w.ok(&["add", "a.tmt", "--upstream", "shared", "a.tmt"]);
    w.ok(&["add", "b.tmt", "--upstream", "shared", "sub/b.tmt"]);

    // Capture initial state byte-for-byte
    let a_before = std::fs::read(w.vault.join("a.tmt")).unwrap();
    let b_before = std::fs::read(w.vault.join("b.tmt")).unwrap();
    let manifest_before = std::fs::read(w.vault.join("catasto.ctmt")).unwrap();
    let lock_before = std::fs::read(w.vault.join("catasto.lock")).unwrap();

    // Advance remote branch with a change to a.tmt
    w.commit("a.tmt", "a two\n");

    // 1. sync --update --dry-run
    // Prints moved commit line, diff hunk for a.tmt, would change 1 of 2 file(s), exits 0
    let out = w.run(&["sync", "--update", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("shared main: "), "{stdout}");
    assert!(stdout.contains("--- a/a.tmt"), "{stdout}");
    assert!(stdout.contains("+++ b/a.tmt\tgit "), "{stdout}");
    assert!(stdout.contains("-a one"), "{stdout}");
    assert!(stdout.contains("+a two"), "{stdout}");
    assert!(!stdout.contains("b.tmt"), "{stdout}");
    assert!(stdout.ends_with("would change 1 of 2 file(s)\n"), "{stdout}");

    // Manifest, lock and files are left byte for byte as before
    assert_eq!(std::fs::read(w.vault.join("a.tmt")).unwrap(), a_before);
    assert_eq!(std::fs::read(w.vault.join("b.tmt")).unwrap(), b_before);
    assert_eq!(std::fs::read(w.vault.join("catasto.ctmt")).unwrap(), manifest_before);
    assert_eq!(std::fs::read(w.vault.join("catasto.lock")).unwrap(), lock_before);

    // 2. --only limits it
    let out = w.run(&["sync", "--update", "--dry-run", "--only", "b.tmt"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("shared main: "), "{stdout}");
    assert!(!stdout.contains("--- a/"), "{stdout}");
    assert!(stdout.ends_with("nothing would change\n"), "{stdout}");

    // 3. sync --update makes exactly the changes shown
    let sync_out = w.run(&["sync", "--update"]);
    assert_eq!(sync_out.status.code(), Some(0));
    assert_eq!(w.read("a.tmt"), "a two\n");
    assert_eq!(w.read("b.tmt"), "b one");

    // 4. No moved commit prints "nothing would change"
    let out = w.run(&["sync", "--update", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "nothing would change\n");

    // 5. Unknown rev fails with exit 1 and the same message sync --update gives
    w.ok(&["upstream", "add", "shared", w.url(), "--rev", "no-such", "--force"]);
    let err_sync = w.fails(&["sync", "--update"]);
    let err_dry_run = w.fails(&["sync", "--update", "--dry-run"]);
    assert!(err_dry_run.contains("revision \"no-such\" not found"), "{err_dry_run}");
    assert_eq!(err_dry_run, err_sync);
}

#[test]
#[cfg(unix)]
fn executable_bit_cli_contract() {
    use std::os::unix::fs::PermissionsExt;

    let w = World::new();
    w.ready(); // adds upstream "shared"
    w.commit_executable("bin/tool.sh", "#!/bin/sh\necho ok\n");

    // 1. catasto add fetches the file and creates it executable
    let add_out = w.ok(&["add", "tool.sh", "--upstream", "shared", "bin/tool.sh"]);
    assert!(add_out.contains("added tool.sh"), "{add_out}");
    let tool_path = w.vault.join("tool.sh");
    let mode = std::fs::metadata(&tool_path).unwrap().permissions().mode();
    assert_ne!(mode & 0o111, 0);

    // Lockfile records "mode": "executable"
    let lock_text = w.read("catasto.lock");
    assert!(lock_text.contains("\"mode\": \"executable\""), "{lock_text}");

    // 2. catasto check is clean
    assert!(w.run(&["check"]).status.success());

    // 3. chmod -x tool.sh -> check detects drift and exits 1
    std::fs::set_permissions(&tool_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let check_out = w.run(&["check"]);
    assert_eq!(check_out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&check_out.stderr);
    assert!(stderr.contains("tool.sh: modified"), "{stderr}");

    // 4. catasto sync restores executable bit
    w.ok(&["sync"]);
    let mode = std::fs::metadata(&tool_path).unwrap().permissions().mode();
    assert_ne!(mode & 0o111, 0);
    assert!(w.run(&["check"]).status.success());
}


