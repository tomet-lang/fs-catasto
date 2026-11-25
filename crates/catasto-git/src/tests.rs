use std::path::Path;

use super::testing::TestRepo;
use super::*;

fn cache() -> (tempfile::TempDir, GitCache) {
    let dir = tempfile::tempdir().unwrap();
    let cache = GitCache::new(dir.path());
    (dir, cache)
}

#[test]
fn resolves_a_branch_a_tag_and_a_commit() {
    let repo = TestRepo::new();
    let first = repo.commit("dir/w.tmt", "one");
    repo.tag("v1");
    let second = repo.commit("dir/w.tmt", "two");
    let (_dir, cache) = cache();
    let path = Path::new("dir/w.tmt");

    let by_branch = cache
        .fetch(&repo.url(), "main", Some(path), None, SymlinkMode::Follow)
        .unwrap();
    let by_tag = cache
        .fetch(&repo.url(), "v1", Some(path), None, SymlinkMode::Follow)
        .unwrap();
    let by_commit = cache
        .fetch(&repo.url(), &first, Some(path), None, SymlinkMode::Follow)
        .unwrap();

    assert_eq!(
        (by_branch.commit.as_str(), by_branch.content),
        (second.as_str(), Content::File(b"two".to_vec()))
    );
    assert_eq!(
        (by_tag.commit.as_str(), by_tag.content),
        (first.as_str(), Content::File(b"one".to_vec()))
    );
    assert_eq!(by_commit.commit, first);
}

#[test]
fn a_pinned_commit_ignores_where_the_branch_went() {
    let repo = TestRepo::new();
    let first = repo.commit("w.tmt", "one");
    repo.commit("w.tmt", "two");
    let (_dir, cache) = cache();

    let fetched = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("w.tmt")),
            Some(&first),
            SymlinkMode::Follow,
        )
        .unwrap();

    assert_eq!(fetched.commit, first);
    assert_eq!(fetched.content, Content::File(b"one".to_vec()));
}

#[test]
fn a_cached_pinned_commit_needs_no_remote() {
    let repo = TestRepo::new();
    let first = repo.commit("w.tmt", "one");
    let (_dir, cache) = cache();
    cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("w.tmt")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap();
    let url = repo.url();
    drop(repo);

    let fetched = cache
        .fetch(
            &url,
            "main",
            Some(Path::new("w.tmt")),
            Some(&first),
            SymlinkMode::Follow,
        )
        .unwrap();

    assert_eq!(fetched.content, Content::File(b"one".to_vec()));
}

#[test]
fn a_pinned_commit_missing_from_the_remote_is_an_error() {
    let repo = TestRepo::new();
    repo.commit("w.tmt", "one");
    let (_dir, cache) = cache();

    let err = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("w.tmt")),
            Some(&"0".repeat(40)),
            SymlinkMode::Follow,
        )
        .unwrap_err();

    assert!(err.to_string().contains("is not in"), "{err}");
}

#[test]
fn a_malformed_pinned_commit_is_rejected_before_running_git() {
    let repo = TestRepo::new();
    repo.commit("w.tmt", "one");
    let (_dir, cache) = cache();

    for bad in ["--upload-pack=x", "abc", "main"] {
        let result = cache.fetch(
            &repo.url(),
            "main",
            Some(Path::new("w.tmt")),
            Some(bad),
            SymlinkMode::Follow,
        );
        assert!(result.is_err(), "accepted {bad:?}");
    }
}

#[test]
fn an_unknown_rev_is_an_error() {
    let repo = TestRepo::new();
    repo.commit("w.tmt", "one");
    let (_dir, cache) = cache();

    let err = cache
        .fetch(
            &repo.url(),
            "nope",
            Some(Path::new("w.tmt")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap_err();

    assert!(err.to_string().contains("\"nope\" not found"), "{err}");
}

#[test]
fn an_unknown_path_is_an_error() {
    let repo = TestRepo::new();
    repo.commit("w.tmt", "one");
    let (_dir, cache) = cache();

    let err = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("nope.tmt")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap_err();

    assert!(err.to_string().contains("nope.tmt is not in"), "{err}");
}

#[test]
fn a_moved_branch_is_picked_up_on_the_next_fetch() {
    let repo = TestRepo::new();
    repo.commit("w.tmt", "one");
    repo.branch("stable");
    let (_dir, cache) = cache();
    let path = Path::new("w.tmt");
    cache
        .fetch(&repo.url(), "stable", Some(path), None, SymlinkMode::Follow)
        .unwrap();
    repo.git(&["checkout", "--quiet", "stable"]);
    let moved = repo.commit("w.tmt", "two");

    let fetched = cache
        .fetch(&repo.url(), "stable", Some(path), None, SymlinkMode::Follow)
        .unwrap();

    assert_eq!(fetched.commit, moved);
}

#[test]
fn accepts_https_urls_and_local_paths() {
    for url in [
        "https://github.com/tomet-lang/twrit.git",
        "/abs/path/repo",
        "../repo",
        "repo",
        "dir/with:colon",
        "./host:path",
    ] {
        assert!(classify(url).is_ok(), "rejected {url}");
    }
}

#[test]
fn rejects_ssh_and_every_other_transport() {
    for url in [
        "git@github.com:tomet-lang/twrit.git",
        "github-alias:tomet-lang/twrit.git",
        "ssh://git@github.com/tomet-lang/twrit.git",
        "git://example.com/repo.git",
        "http://example.com/repo.git",
        "file:///abs/path/repo",
        "ext::sh -c id",
        "https://",
        "https:///x",
        "-oProxyCommand=x",
        "",
    ] {
        assert!(classify(url).is_err(), "accepted {url}");
    }
}

#[test]
fn local_paths_resolve_against_the_workspace_root() {
    let root = Path::new("/ws/vault");

    assert_eq!(
        resolve_url(root, "../remote").unwrap(),
        "/ws/vault/../remote"
    );
    assert_eq!(resolve_url(root, "remote").unwrap(), "/ws/vault/remote");
    assert_eq!(resolve_url(root, "/abs/repo").unwrap(), "/abs/repo");
    assert_eq!(
        resolve_url(root, "https://example.com/r.git").unwrap(),
        "https://example.com/r.git"
    );
}

#[test]
fn git_itself_refuses_other_transports() {
    let (_dir, cache) = cache();

    let err = cache
        .fetch(
            "ssh://localhost/repo.git",
            "main",
            Some(Path::new("w.tmt")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap_err();

    assert!(format!("{err:#}").contains("not allowed"), "{err:#}");
}

#[test]
fn the_cache_keeps_one_clone_per_url() {
    let a = TestRepo::new();
    a.commit("w.tmt", "a");
    let b = TestRepo::new();
    b.commit("w.tmt", "b");
    let (_dir, cache) = cache();
    let path = Path::new("w.tmt");

    assert_eq!(
        cache
            .fetch(&a.url(), "main", Some(path), None, SymlinkMode::Follow)
            .unwrap()
            .content,
        Content::File(b"a".to_vec())
    );
    assert_eq!(
        cache
            .fetch(&b.url(), "main", Some(path), None, SymlinkMode::Follow)
            .unwrap()
            .content,
        Content::File(b"b".to_vec())
    );
}

#[test]
fn fetches_a_directory_tree() {
    let repo = TestRepo::new();
    repo.commit("dir/file.txt", "content");
    repo.commit("dir/nested/deep.txt", "deep");
    repo.commit("other.txt", "outside");
    let (_dir, cache) = cache();

    let fetched = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("dir")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap();

    assert!(!fetched.executable);
    assert_eq!(
        fetched.content,
        Content::Dir(vec![
            DirEntry {
                rel_path: PathBuf::from("file.txt"),
                content: DirEntryContent::File {
                    bytes: b"content".to_vec(),
                    executable: false,
                },
            },
            DirEntry {
                rel_path: PathBuf::from("nested/deep.txt"),
                content: DirEntryContent::File {
                    bytes: b"deep".to_vec(),
                    executable: false,
                },
            },
        ])
    );
}

#[test]
#[cfg(unix)]
fn follow_one_hop_and_several_hops_inside_repo() {
    let repo = TestRepo::new();
    repo.commit("target/real.txt", "hello");
    repo.commit_symlink("link1", "target/real.txt");
    repo.commit_symlink("nested/link2", "../link1");
    repo.commit_symlink("nested/deep/link3", "../link2");

    let (_dir, cache) = cache();

    let f1 = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("link1")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap();
    assert_eq!(f1.content, Content::File(b"hello".to_vec()));

    let f3 = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("nested/deep/link3")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap();
    assert_eq!(f3.content, Content::File(b"hello".to_vec()));
}

#[test]
#[cfg(unix)]
fn follow_eight_hops_ok_nine_hops_error() {
    let repo = TestRepo::new();
    repo.commit("real.txt", "found");
    // link1 -> real.txt (1 hop)
    repo.commit_symlink("l1", "real.txt");
    for i in 2..=10 {
        repo.commit_symlink(&format!("l{i}"), &format!("l{}", i - 1));
    }

    let (_dir, cache) = cache();

    // l8 -> l7 -> l6 -> l5 -> l4 -> l3 -> l2 -> l1 -> real.txt (8 hops to reach real.txt)
    let f8 = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("l8")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap();
    assert_eq!(f8.content, Content::File(b"found".to_vec()));

    // l9 -> 8 links before reaching real.txt requires 9 hops -> error
    let err9 = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("l9")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap_err();
    assert!(
        err9.to_string()
            .contains("too many levels of symbolic links"),
        "{err9}"
    );
    assert!(err9.to_string().contains("link chain:"), "{err9}");
}

#[test]
#[cfg(unix)]
fn follow_loop_is_an_error() {
    let repo = TestRepo::new();
    repo.commit_symlink("a", "b");
    repo.commit_symlink("b", "a");

    let (_dir, cache) = cache();
    let err = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("a")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap_err();
    assert!(err.to_string().contains("symlink loop detected"), "{err}");
    assert!(err.to_string().contains("link chain: a -> b -> a"), "{err}");
}

#[test]
#[cfg(unix)]
fn follow_bad_target_errors() {
    let repo = TestRepo::new();
    repo.commit("real.txt", "ok");
    repo.commit_symlink("abs", "/etc/passwd");
    repo.commit_symlink("escape", "../../secret");
    repo.commit_symlink("dangling", "missing.txt");
    repo.commit_symlink("to_dir", "dir");
    repo.commit("dir/inside.txt", "inside");

    let (_dir, cache) = cache();

    let err_abs = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("abs")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap_err();
    assert!(
        err_abs.to_string().contains("symlink target is absolute"),
        "{err_abs}"
    );

    let err_escape = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("escape")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap_err();
    assert!(
        err_escape
            .to_string()
            .contains("climbs out of the repository"),
        "{err_escape}"
    );

    let err_dangling = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("dangling")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap_err();
    assert!(
        err_dangling
            .to_string()
            .contains("target `missing.txt` does not exist"),
        "{err_dangling}"
    );

    let fetched_dir = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("to_dir")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap();
    assert_eq!(
        fetched_dir.content,
        Content::Dir(vec![DirEntry {
            rel_path: PathBuf::from("inside.txt"),
            content: DirEntryContent::File {
                bytes: b"inside".to_vec(),
                executable: false,
            },
        }])
    );
}

#[test]
#[cfg(unix)]
fn keep_mode_checks() {
    let repo = TestRepo::new();
    repo.commit("real.txt", "hello");
    repo.commit_symlink("link.txt", "target.txt");
    repo.commit_symlink("bad_abs", "/abs");

    let (_dir, cache) = cache();

    // Plain link ok, even if target is dangling
    let fetched = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("link.txt")),
            None,
            SymlinkMode::Keep,
        )
        .unwrap();
    assert_eq!(fetched.content, Content::Link("target.txt".to_owned()));

    // Not a link is an error
    let err_not_link = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("real.txt")),
            None,
            SymlinkMode::Keep,
        )
        .unwrap_err();
    assert!(
        err_not_link
            .to_string()
            .contains("`symlink: keep`, but real.txt is not a symlink"),
        "{err_not_link}"
    );

    // Bad target text is an error even with keep
    let err_bad = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("bad_abs")),
            None,
            SymlinkMode::Keep,
        )
        .unwrap_err();
    assert!(
        err_bad.to_string().contains("symlink target is absolute"),
        "{err_bad}"
    );
}

#[test]
#[cfg(unix)]
fn symlinked_directory_component_is_an_error() {
    let repo = TestRepo::new();
    repo.commit("realdir/file.txt", "deep content");
    repo.commit_symlink("symdir", "realdir");

    let (_dir, cache) = cache();

    let err = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("symdir/file.txt")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("directory component `symdir` of symdir/file.txt is a symlink"),
        "{err}"
    );
}

#[test]
#[cfg(unix)]
fn pinned_commit_still_resolves_link_at_pinned_commit() {
    let repo = TestRepo::new();
    repo.commit("v1.txt", "version 1");
    repo.commit("v2.txt", "version 2");
    let first = repo.commit_symlink("link", "v1.txt");
    repo.commit_symlink("link", "v2.txt");

    let (_dir, cache) = cache();

    let fetched = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("link")),
            Some(&first),
            SymlinkMode::Follow,
        )
        .unwrap();
    assert_eq!(fetched.commit, first);
    assert_eq!(fetched.content, Content::File(b"version 1".to_vec()));
}

#[test]
fn reports_mode_for_executable_and_normal_files() {
    let repo = TestRepo::new();
    repo.commit("normal.txt", "normal content");
    repo.commit_executable("script.sh", "echo hi");

    let (_dir, cache) = cache();

    let fetched_normal = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("normal.txt")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap();
    assert!(!fetched_normal.executable);
    assert_eq!(
        fetched_normal.content,
        Content::File(b"normal content".to_vec())
    );

    let fetched_exec = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("script.sh")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap();
    assert!(fetched_exec.executable);
    assert_eq!(fetched_exec.content, Content::File(b"echo hi".to_vec()));
}

#[test]
#[cfg(unix)]
fn reports_mode_for_followed_link_to_executable() {
    let repo = TestRepo::new();
    repo.commit_executable("target.sh", "#!/bin/sh\n");
    repo.commit_symlink("link.sh", "target.sh");

    let (_dir, cache) = cache();

    let fetched_follow = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("link.sh")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap();
    assert!(fetched_follow.executable);
    assert_eq!(
        fetched_follow.content,
        Content::File(b"#!/bin/sh\n".to_vec())
    );

    let fetched_keep = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("link.sh")),
            None,
            SymlinkMode::Keep,
        )
        .unwrap();
    assert!(!fetched_keep.executable);
    assert_eq!(fetched_keep.content, Content::Link("target.sh".to_owned()));
}

#[test]
fn worktree_helpers_clone_inspect_fetch_and_checkout() {
    let repo = TestRepo::new();
    let c1 = repo.commit("a.txt", "v1");

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("cloned");

    let head = clone_submodule(&repo.url(), &dest, "main", None, None).unwrap();
    assert_eq!(head, c1);
    assert_eq!(head_commit(&dest).unwrap(), c1);
    assert_eq!(remote_url(&dest).unwrap(), repo.url());
    assert_eq!(current_branch(&dest).unwrap().as_deref(), Some("main"));

    let c2 = repo.commit("a.txt", "v2");
    fetch_worktree(&dest).unwrap();
    let updated = checkout_rev(&dest, &repo.url(), "main").unwrap();
    assert_eq!(updated, c2);
    assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "v2");

    checkout_commit(&dest, &c1).unwrap();
    assert_eq!(head_commit(&dest).unwrap(), c1);
    assert_eq!(current_branch(&dest).unwrap(), None);
}

#[test]
#[cfg(unix)]
fn fetches_directory_and_matches_sha256_dir() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::new();
    repo.commit("dir/a.txt", "hello");
    repo.commit_executable("dir/sub/run.sh", "#!/bin/sh\necho hi\n");
    repo.commit_symlink("dir/sub/link", "../a.txt");
    repo.commit("outside.txt", "outside");

    let (_dir, cache) = cache();

    let fetched_keep = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("dir")),
            None,
            SymlinkMode::Keep,
        )
        .unwrap();
    let keep_entries = fetched_keep.content.dir_entries().unwrap();
    assert_eq!(
        keep_entries,
        &[
            DirEntry {
                rel_path: PathBuf::from("a.txt"),
                content: DirEntryContent::File {
                    bytes: b"hello".to_vec(),
                    executable: false,
                },
            },
            DirEntry {
                rel_path: PathBuf::from("sub/link"),
                content: DirEntryContent::Link("../a.txt".to_owned()),
            },
            DirEntry {
                rel_path: PathBuf::from("sub/run.sh"),
                content: DirEntryContent::File {
                    bytes: b"#!/bin/sh\necho hi\n".to_vec(),
                    executable: true,
                },
            },
        ]
    );

    // Materialize the same tree on disk and verify Content::sha256 matches sha256_dir.
    let disk = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(disk.path().join("sub")).unwrap();
    std::fs::write(disk.path().join("a.txt"), "hello").unwrap();
    let run_path = disk.path().join("sub/run.sh");
    std::fs::write(&run_path, "#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(&run_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("../a.txt", disk.path().join("sub/link")).unwrap();

    assert_eq!(
        fetched_keep.content.sha256(None),
        catasto_digest::sha256_dir(disk.path()).unwrap()
    );
    assert_ne!(
        fetched_keep.content.sha256(Some(false)),
        fetched_keep.content.sha256(None)
    );

    // With Follow mode, dir/sub/link resolves to a.txt's content.
    let fetched_follow = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("dir")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap();
    assert_eq!(
        fetched_follow.content.dir_entries().unwrap(),
        &[
            DirEntry {
                rel_path: PathBuf::from("a.txt"),
                content: DirEntryContent::File {
                    bytes: b"hello".to_vec(),
                    executable: false,
                },
            },
            DirEntry {
                rel_path: PathBuf::from("sub/link"),
                content: DirEntryContent::File {
                    bytes: b"hello".to_vec(),
                    executable: false,
                },
            },
            DirEntry {
                rel_path: PathBuf::from("sub/run.sh"),
                content: DirEntryContent::File {
                    bytes: b"#!/bin/sh\necho hi\n".to_vec(),
                    executable: true,
                },
            },
        ]
    );
}

#[test]
#[cfg(unix)]
fn fetches_repository_root_when_path_is_none() {
    let repo = TestRepo::new();
    repo.commit("root.txt", "root");
    repo.commit_executable("bin/tool", "#!/bin/sh\n");
    repo.commit_symlink("alias.txt", "root.txt");

    let (_dir, cache) = cache();

    let fetched = cache
        .fetch(&repo.url(), "main", None, None, SymlinkMode::Follow)
        .unwrap();
    assert!(!fetched.executable);
    assert_eq!(
        fetched.content.dir_entries().unwrap(),
        &[
            DirEntry {
                rel_path: PathBuf::from("alias.txt"),
                content: DirEntryContent::File {
                    bytes: b"root".to_vec(),
                    executable: false,
                },
            },
            DirEntry {
                rel_path: PathBuf::from("bin/tool"),
                content: DirEntryContent::File {
                    bytes: b"#!/bin/sh\n".to_vec(),
                    executable: true,
                },
            },
            DirEntry {
                rel_path: PathBuf::from("root.txt"),
                content: DirEntryContent::File {
                    bytes: b"root".to_vec(),
                    executable: false,
                },
            },
        ]
    );

    let fetched_keep = cache
        .fetch(&repo.url(), "main", None, None, SymlinkMode::Keep)
        .unwrap();
    assert_eq!(
        fetched_keep.content.dir_entries().unwrap()[0],
        DirEntry {
            rel_path: PathBuf::from("alias.txt"),
            content: DirEntryContent::Link("root.txt".into()),
        }
    );
}

#[test]
#[cfg(unix)]
fn directory_symlink_expansion_and_loop_detection() {
    let repo = TestRepo::new();
    repo.commit("shared/one.txt", "1");
    repo.commit_symlink("shared/two.txt", "one.txt");
    repo.commit_symlink("dir/linked_shared", "../shared");
    repo.commit("dir/local.txt", "loc");
    repo.commit_symlink("loop_dir/self_link", ".");
    repo.commit("bad_keep/ok.txt", "ok");
    repo.commit_symlink("bad_keep/abs", "/etc/passwd");

    let (_dir, cache) = cache();

    let fetched = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("dir")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap();
    assert_eq!(
        fetched.content.dir_entries().unwrap(),
        &[
            DirEntry {
                rel_path: PathBuf::from("linked_shared/one.txt"),
                content: DirEntryContent::File {
                    bytes: b"1".to_vec(),
                    executable: false,
                },
            },
            DirEntry {
                rel_path: PathBuf::from("linked_shared/two.txt"),
                content: DirEntryContent::File {
                    bytes: b"1".to_vec(),
                    executable: false,
                },
            },
            DirEntry {
                rel_path: PathBuf::from("local.txt"),
                content: DirEntryContent::File {
                    bytes: b"loc".to_vec(),
                    executable: false,
                },
            },
        ]
    );

    let err_loop = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("loop_dir")),
            None,
            SymlinkMode::Follow,
        )
        .unwrap_err();
    assert!(
        err_loop.to_string().contains("symlink loop detected"),
        "{err_loop}"
    );

    let err_bad_keep = cache
        .fetch(
            &repo.url(),
            "main",
            Some(Path::new("bad_keep")),
            None,
            SymlinkMode::Keep,
        )
        .unwrap_err();
    assert!(
        err_bad_keep
            .to_string()
            .contains("symlink target is absolute"),
        "{err_bad_keep}"
    );
}

#[test]
fn clone_submodule_without_subpath_supports_branch_tag_commit_and_pinned() {
    let repo = TestRepo::new();
    let c1 = repo.commit("a.txt", "v1");
    repo.tag("v1.0");
    let c2 = repo.commit("a.txt", "v2");

    let dir = tempfile::tempdir().unwrap();

    // Branch
    let d_branch = dir.path().join("branch");
    let head_branch = clone_submodule(&repo.url(), &d_branch, "main", None, None).unwrap();
    assert_eq!(head_branch, c2);
    assert_eq!(current_branch(&d_branch).unwrap().as_deref(), Some("main"));
    assert_eq!(
        std::fs::read_to_string(d_branch.join("a.txt")).unwrap(),
        "v2"
    );
    assert!(check_sparse_checkout(&d_branch, None).unwrap());
    assert!(!is_sparse_checkout(&d_branch).unwrap());

    // Tag
    let d_tag = dir.path().join("tag");
    let head_tag = clone_submodule(&repo.url(), &d_tag, "v1.0", None, None).unwrap();
    assert_eq!(head_tag, c1);
    assert_eq!(std::fs::read_to_string(d_tag.join("a.txt")).unwrap(), "v1");
    assert!(check_sparse_checkout(&d_tag, None).unwrap());

    // Commit hash as rev
    let d_commit = dir.path().join("commit");
    let head_commit_res = clone_submodule(&repo.url(), &d_commit, &c1, None, None).unwrap();
    assert_eq!(head_commit_res, c1);
    assert_eq!(
        std::fs::read_to_string(d_commit.join("a.txt")).unwrap(),
        "v1"
    );

    // Pinned commit overrides rev
    let d_pinned = dir.path().join("pinned");
    let head_pinned = clone_submodule(&repo.url(), &d_pinned, "main", Some(&c1), None).unwrap();
    assert_eq!(head_pinned, c1);
    assert_eq!(
        std::fs::read_to_string(d_pinned.join("a.txt")).unwrap(),
        "v1"
    );
}

#[test]
fn clone_submodule_with_sparse_checkout_and_checks() {
    let repo = TestRepo::new();
    repo.commit("root.txt", "root");
    repo.commit("pkg/a/one.txt", "a1");
    repo.commit("pkg/a/nested/two.txt", "a2");
    let head = repo.commit("pkg/b/other.txt", "b1");

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("sub");
    let subpath = Path::new("pkg/a");

    let cloned = clone_submodule(&repo.url(), &dest, "main", None, Some(subpath)).unwrap();
    assert_eq!(cloned, head);
    assert_eq!(
        std::fs::read_to_string(dest.join("pkg/a/one.txt")).unwrap(),
        "a1"
    );
    assert_eq!(
        std::fs::read_to_string(dest.join("pkg/a/nested/two.txt")).unwrap(),
        "a2"
    );
    assert!(!dest.join("root.txt").exists());
    assert!(!dest.join("pkg/b").exists());

    assert!(is_sparse_checkout(&dest).unwrap());
    assert_eq!(sparse_checkout_list(&dest).unwrap(), vec!["/pkg/a"]);
    assert!(check_sparse_checkout(&dest, Some(subpath)).unwrap());
    assert!(!check_sparse_checkout(&dest, None).unwrap());
    assert!(!check_sparse_checkout(&dest, Some(Path::new("pkg/b"))).unwrap());

    // Altering sparse-checkout causes check_sparse_checkout to return false.
    apply_sparse_checkout(&dest, Some(Path::new("pkg/b"))).unwrap();
    assert!(!check_sparse_checkout(&dest, Some(subpath)).unwrap());
    assert!(check_sparse_checkout(&dest, Some(Path::new("pkg/b"))).unwrap());

    // Disabling sparse-checkout populates root/siblings and causes subpath check to return false.
    apply_sparse_checkout(&dest, None).unwrap();
    assert!(!is_sparse_checkout(&dest).unwrap());
    assert!(check_sparse_checkout(&dest, None).unwrap());
    assert!(!check_sparse_checkout(&dest, Some(subpath)).unwrap());
    assert!(dest.join("root.txt").exists());
}

#[test]
fn clone_submodule_missing_subpath_errors_and_cleans_up() {
    let repo = TestRepo::new();
    repo.commit("pkg/a/one.txt", "a1");

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("failed_sub");

    let err = clone_submodule(
        &repo.url(),
        &dest,
        "main",
        None,
        Some(Path::new("does/not/exist")),
    )
    .unwrap_err();
    assert!(err.to_string().contains("is not in"), "{err}");
    assert!(!dest.exists());
}

#[test]
fn update_submodule_advances_rev_and_updates_sparse_checkout() {
    let repo = TestRepo::new();
    let c1 = repo.commit("pkg/a/file.txt", "v1");
    repo.commit("pkg/b/other.txt", "b1");

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("sub");
    clone_submodule(&repo.url(), &dest, "main", None, Some(Path::new("pkg/a"))).unwrap();

    let c2 = repo.commit("pkg/a/file.txt", "v2");
    let updated = update_submodule(&dest, &repo.url(), "main", Some(Path::new("pkg/a"))).unwrap();
    assert_eq!(updated, c2);
    assert_eq!(
        std::fs::read_to_string(dest.join("pkg/a/file.txt")).unwrap(),
        "v2"
    );
    assert!(!dest.join("pkg/b").exists());

    // Switch sparse-checkout subpath from pkg/a to pkg/b
    update_submodule(&dest, &repo.url(), "main", Some(Path::new("pkg/b"))).unwrap();
    assert!(check_sparse_checkout(&dest, Some(Path::new("pkg/b"))).unwrap());
    assert!(dest.join("pkg/b/other.txt").exists());
    assert!(!dest.join("pkg/a").exists());

    // Disable sparse-checkout and switch to an older commit c1
    let rolled_back = update_submodule(&dest, &repo.url(), &c1, None).unwrap();
    assert_eq!(rolled_back, c1);
    assert!(check_sparse_checkout(&dest, None).unwrap());
    assert_eq!(
        std::fs::read_to_string(dest.join("pkg/a/file.txt")).unwrap(),
        "v1"
    );
}

#[test]
fn visible_entries_tracks_files_symlinks_submodules_and_excludes_ignored_and_deleted() {
    let sub_upstream = TestRepo::new();
    sub_upstream.commit("sub.txt", "sub");

    let repo = TestRepo::new();
    let root = PathBuf::from(repo.url());

    std::fs::write(root.join(".gitignore"), "*.swp\nignored_dir/\n").unwrap();
    repo.commit(".gitignore", "*.swp\nignored_dir/\n");
    repo.commit("tracked.txt", "keep");
    repo.commit("deleted.txt", "remove me");
    repo.commit("special/space and \"quote\".txt", "special");
    #[cfg(unix)]
    repo.commit_symlink("tracked_link", "tracked.txt");

    // Tracked submodule (160000)
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--quiet",
        &sub_upstream.url(),
        "deps/tracked_sub",
    ]);
    repo.git(&["commit", "--quiet", "-m", "add submodule"]);

    // Working-tree changes:
    // 1. Delete a tracked file without staging
    std::fs::remove_file(root.join("deleted.txt")).unwrap();
    // 2. Untracked non-ignored file
    std::fs::write(root.join("untracked.txt"), "new").unwrap();
    // 3. Untracked ignored files & directory
    std::fs::write(root.join("scratch.swp"), "ignored").unwrap();
    std::fs::create_dir_all(root.join("ignored_dir")).unwrap();
    std::fs::write(root.join("ignored_dir/inside.txt"), "ignored").unwrap();
    // 4. Empty directory
    std::fs::create_dir_all(root.join("empty_dir")).unwrap();
    // 5. Untracked submodule / nested git repo (shows with trailing `/` in `--others`)
    clone_submodule(
        &sub_upstream.url(),
        &root.join("deps/untracked_sub"),
        "main",
        None,
        None,
    )
    .unwrap();
    // 6. Untracked symlink
    #[cfg(unix)]
    std::os::unix::fs::symlink("tracked.txt", root.join("untracked_link")).unwrap();

    let entries = visible_entries(&root).unwrap();
    let expected = vec![
        VisibleEntry {
            path: ".gitignore".into(),
            kind: VisibleEntryKind::File,
        },
        VisibleEntry {
            path: ".gitmodules".into(),
            kind: VisibleEntryKind::File,
        },
        VisibleEntry {
            path: "deps/tracked_sub".into(),
            kind: VisibleEntryKind::Submodule,
        },
        VisibleEntry {
            path: "deps/untracked_sub".into(),
            kind: VisibleEntryKind::Submodule,
        },
        VisibleEntry {
            path: "special/space and \"quote\".txt".into(),
            kind: VisibleEntryKind::File,
        },
        #[cfg(unix)]
        VisibleEntry {
            path: "tracked_link".into(),
            kind: VisibleEntryKind::Symlink,
        },
        VisibleEntry {
            path: "tracked.txt".into(),
            kind: VisibleEntryKind::File,
        },
        #[cfg(unix)]
        VisibleEntry {
            path: "untracked_link".into(),
            kind: VisibleEntryKind::Symlink,
        },
        VisibleEntry {
            path: "untracked.txt".into(),
            kind: VisibleEntryKind::File,
        },
    ];
    let mut expected_sorted = expected;
    expected_sorted.sort();
    assert_eq!(entries, expected_sorted);

    // Non-git directory fails with an error
    let non_git = tempfile::tempdir().unwrap();
    assert!(visible_entries(non_git.path()).is_err());
}
