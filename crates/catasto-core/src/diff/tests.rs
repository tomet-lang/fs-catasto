use std::path::{Path, PathBuf};

use catasto_git::testing::TestRepo;

use crate::diff::diff;
use crate::manage::{add, init, upstream_add};
use crate::sync::{SyncMode, plan_sync};
use catasto_manifest::edit::{SourceText, UpstreamText};

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
    repo: TestRepo,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let repo = TestRepo::new();
        repo.commit("a.tmt", "a one\n");
        repo.commit("sub/b.tmt", "b one\n");
        Self {
            cache: tmp.path().join("cache"),
            _tmp: tmp,
            root,
            repo,
        }
    }

    fn upstream(&self) -> UpstreamText {
        UpstreamText {
            git: self.repo.url(),
            rev: "main".into(),
            root: None,
        }
    }

    fn ready(&self) {
        init(&self.root).unwrap();
        upstream_add(&self.root, "t", &self.upstream(), false).unwrap();
    }

    fn add_git(&self, dest: &str, path: &str) {
        add(
            &self.root,
            dest,
            &SourceText::Upstream {
                name: "t".into(),
                path: path.into(),
                keep_symlink: false,
            },
            false,
            Some(&self.cache),
        )
        .unwrap();
    }

    fn add_local(&self, dest: &str, local_file: &Path) {
        add(
            &self.root,
            dest,
            &SourceText::Local {
                path: local_file.to_str().unwrap().into(),
            },
            false,
            Some(&self.cache),
        )
        .unwrap();
    }
}

#[test]
fn equal_file_produces_no_diff() {
    let fx = Fixture::new();
    fx.ready();
    fx.add_git("a.tmt", "a.tmt");

    let out = diff(&fx.root, Some(&fx.cache), &[]).unwrap();
    assert_eq!(out, "");
}

#[test]
fn modified_file_shows_hunk_and_locked_commit() {
    let fx = Fixture::new();
    fx.ready();
    fx.add_git("a.tmt", "a.tmt");

    // Hand-edit a.tmt on disk
    std::fs::write(fx.root.join("a.tmt"), "a modified\n").unwrap();

    let plan = plan_sync(&fx.root, SyncMode::Locked, Some(&fx.cache), &[]).unwrap();
    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.files[0].path, Path::new("a.tmt"));

    let out = diff(&fx.root, Some(&fx.cache), &[]).unwrap();
    assert!(out.contains("--- a/a.tmt"), "{out}");
    assert!(out.contains("+++ b/a.tmt\tgit "), "{out}");
    assert!(out.contains("-a modified"), "{out}");
    assert!(out.contains("+a one"), "{out}");
}

#[test]
fn missing_file_shows_creation_from_dev_null() {
    let fx = Fixture::new();
    fx.ready();
    fx.add_git("a.tmt", "a.tmt");

    // Remove the file on disk
    std::fs::remove_file(fx.root.join("a.tmt")).unwrap();

    let out = diff(&fx.root, Some(&fx.cache), &[]).unwrap();
    assert!(out.contains("--- /dev/null"), "{out}");
    assert!(out.contains("+++ b/a.tmt\tgit "), "{out}");
    assert!(out.contains("+a one"), "{out}");
}

#[test]
fn stale_local_source_shows_upstream_changes() {
    let fx = Fixture::new();
    init(&fx.root).unwrap();

    let local_file = fx._tmp.path().join("local.tmt");
    std::fs::write(&local_file, "original\n").unwrap();
    fx.add_local("l.tmt", &local_file);

    // Now update the local source file outside the workspace
    std::fs::write(&local_file, "updated\n").unwrap();

    let out = diff(&fx.root, Some(&fx.cache), &[]).unwrap();
    assert!(out.contains("--- a/l.tmt"), "{out}");
    assert!(out.contains("+++ b/l.tmt\tlocal "), "{out}");
    assert!(out.contains("-original"), "{out}");
    assert!(out.contains("+updated"), "{out}");
}

#[test]
fn git_source_diff_uses_locked_commit() {
    let fx = Fixture::new();
    fx.ready();
    fx.add_git("a.tmt", "a.tmt");

    // Push new commit on repo main branch
    fx.repo.commit("a.tmt", "a two\n");

    // The lock still pins commit 1, and the file on disk matches commit 1.
    // diff in Locked mode should show no diff!
    let out = diff(&fx.root, Some(&fx.cache), &[]).unwrap();
    assert_eq!(out, "");

    // But if we hand edit the file on disk, diff compares against commit 1, not commit 2
    std::fs::write(fx.root.join("a.tmt"), "a modified\n").unwrap();
    let out = diff(&fx.root, Some(&fx.cache), &[]).unwrap();
    assert!(out.contains("+a one"), "{out}");
    assert!(!out.contains("+a two"), "{out}");
}

#[test]
fn binary_file_diff_prints_one_line_notice() {
    let fx = Fixture::new();
    fx.ready();
    // Commit a binary file to the repo
    let bin_content = [0xFF, 0x00, 0xFE, 0x01];
    let file = fx.repo.url();
    let repo_bin = PathBuf::from(&file).join("bin.dat");
    std::fs::write(&repo_bin, bin_content).unwrap();
    fx.repo.git(&["add", "bin.dat"]);
    fx.repo.git(&["commit", "-m", "bin"]);

    fx.add_git("bin.dat", "bin.dat");

    // Modify binary file on disk
    std::fs::write(fx.root.join("bin.dat"), [0xAA, 0xBB]).unwrap();

    let out = diff(&fx.root, Some(&fx.cache), &[]).unwrap();
    assert_eq!(out, "Binary files differ: bin.dat\n");
}

#[test]
#[cfg(unix)]
fn symlink_diff_formats() {
    let fx = Fixture::new();
    fx.ready();
    fx.repo.commit_symlink("link.tmt", "target1.tmt");

    add(
        &fx.root,
        "link.tmt",
        &SourceText::Upstream {
            name: "t".into(),
            path: "link.tmt".into(),
            keep_symlink: true,
        },
        false,
        Some(&fx.cache),
    )
    .unwrap();

    // 1. Equal link target -> no diff
    let out = diff(&fx.root, Some(&fx.cache), &[]).unwrap();
    assert_eq!(out, "");

    // 2. Change link target on disk
    std::fs::remove_file(fx.root.join("link.tmt")).unwrap();
    std::os::unix::fs::symlink("target2.tmt", fx.root.join("link.tmt")).unwrap();

    let out = diff(&fx.root, Some(&fx.cache), &[]).unwrap();
    assert_eq!(out, "link link.tmt: target2.tmt -> target1.tmt\n");

    // 3. Regular file on disk where symlink is expected
    std::fs::remove_file(fx.root.join("link.tmt")).unwrap();
    std::fs::write(fx.root.join("link.tmt"), "regular").unwrap();
    let out = diff(&fx.root, Some(&fx.cache), &[]).unwrap();
    assert_eq!(
        out,
        "link.tmt: regular file becomes symlink to target1.tmt\n"
    );
}
