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
        .fetch(&repo.url(), "main", path, None, SymlinkMode::Follow)
        .unwrap();
    let by_tag = cache
        .fetch(&repo.url(), "v1", path, None, SymlinkMode::Follow)
        .unwrap();
    let by_commit = cache
        .fetch(&repo.url(), &first, path, None, SymlinkMode::Follow)
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
            Path::new("w.tmt"),
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
            Path::new("w.tmt"),
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
            Path::new("w.tmt"),
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
            Path::new("w.tmt"),
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
            Path::new("w.tmt"),
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
            Path::new("w.tmt"),
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
            Path::new("nope.tmt"),
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
        .fetch(&repo.url(), "stable", path, None, SymlinkMode::Follow)
        .unwrap();
    repo.git(&["checkout", "--quiet", "stable"]);
    let moved = repo.commit("w.tmt", "two");

    let fetched = cache
        .fetch(&repo.url(), "stable", path, None, SymlinkMode::Follow)
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
            Path::new("w.tmt"),
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
            .fetch(&a.url(), "main", path, None, SymlinkMode::Follow)
            .unwrap()
            .content,
        Content::File(b"a".to_vec())
    );
    assert_eq!(
        cache
            .fetch(&b.url(), "main", path, None, SymlinkMode::Follow)
            .unwrap()
            .content,
        Content::File(b"b".to_vec())
    );
}

#[test]
fn fetches_a_tree_as_error() {
    let repo = TestRepo::new();
    repo.commit("dir/file.txt", "content");
    let (_dir, cache) = cache();

    let err = cache
        .fetch(
            &repo.url(),
            "main",
            Path::new("dir"),
            None,
            SymlinkMode::Follow,
        )
        .unwrap_err();

    assert!(
        err.to_string().contains("is a directory, not a file"),
        "{err}"
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
            Path::new("link1"),
            None,
            SymlinkMode::Follow,
        )
        .unwrap();
    assert_eq!(f1.content, Content::File(b"hello".to_vec()));

    let f3 = cache
        .fetch(
            &repo.url(),
            "main",
            Path::new("nested/deep/link3"),
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
            Path::new("l8"),
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
            Path::new("l9"),
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
            Path::new("a"),
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
            Path::new("abs"),
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
            Path::new("escape"),
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
            Path::new("dangling"),
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

    let err_dir = cache
        .fetch(
            &repo.url(),
            "main",
            Path::new("to_dir"),
            None,
            SymlinkMode::Follow,
        )
        .unwrap_err();
    assert!(
        err_dir.to_string().contains("target `dir` is a directory"),
        "{err_dir}"
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
            Path::new("link.txt"),
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
            Path::new("real.txt"),
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
            Path::new("bad_abs"),
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
            Path::new("symdir/file.txt"),
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
            Path::new("link"),
            Some(&first),
            SymlinkMode::Follow,
        )
        .unwrap();
    assert_eq!(fetched.commit, first);
    assert_eq!(fetched.content, Content::File(b"version 1".to_vec()));
}
