use super::*;
use crate::AlreadyDeclared;
use catasto_git::testing::TestRepo;

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
        repo.commit("a.tmt", "a one");
        repo.commit("sub/b.tmt", "b one");
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

    fn via(&self, path: &str) -> SourceText {
        SourceText::Upstream {
            name: "t".into(),
            path: path.into(),
            keep_symlink: false,
        }
    }

    fn add(&self, dest: &str, path: &str) -> Result<AddOutcome> {
        add(&self.root, dest, &self.via(path), false, Some(&self.cache))
    }

    fn add_force(&self, dest: &str, path: &str) -> Result<AddOutcome> {
        add(&self.root, dest, &self.via(path), true, Some(&self.cache))
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.root.join(path)).unwrap()
    }

    fn manifest(&self) -> String {
        self.read(MANIFEST_FILE)
    }

    fn ready(&self) {
        init(&self.root).unwrap();
        upstream_add(&self.root, "t", &self.upstream()).unwrap();
    }
}

#[test]
fn init_writes_a_manifest_that_loads_and_declares_nothing() {
    let fx = Fixture::new();
    let file = init(&fx.root).unwrap();
    assert_eq!(file, fx.root.join("catasto.ctmt"));
    assert!(Manifest::load(&fx.root).unwrap().managed.is_empty());
    assert!(check(&fx.root).unwrap().is_ok());
    assert!(fx.manifest().contains("catasto add"));
}

#[test]
fn init_never_overwrites() {
    let fx = Fixture::new();
    init(&fx.root).unwrap();
    std::fs::write(fx.root.join("catasto.ctmt"), "// mine\n").unwrap();
    let err = init(&fx.root).unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
    assert_eq!(fx.manifest(), "// mine\n");
}

#[test]
fn a_manifest_spelled_in_another_case_is_named_in_the_error() {
    let fx = Fixture::new();
    std::fs::write(fx.root.join("Catasto.ctmt"), "").unwrap();
    for err in [
        init(&fx.root).unwrap_err(),
        Manifest::load(&fx.root).unwrap_err(),
        check(&fx.root).unwrap_err(),
    ] {
        let text = err.to_string();
        assert!(
            text.contains("found Catasto.ctmt") && text.contains("rename it"),
            "{text}"
        );
    }
}

#[test]
fn commands_that_edit_need_a_manifest() {
    let fx = Fixture::new();
    let err = fx.add("a.tmt", "a.tmt").unwrap_err();
    assert!(err.to_string().contains("catasto init"), "{err}");
}

#[test]
fn an_upstream_then_a_file_from_it_is_declared_fetched_and_locked() {
    let fx = Fixture::new();
    fx.ready();
    fx.add("docs/a.tmt", "a.tmt").unwrap();

    assert_eq!(fx.read("docs/a.tmt"), "a one");
    let lock = Lock::load(&fx.root).unwrap();
    let locked = lock.get(Path::new("docs/a.tmt")).unwrap();
    assert_eq!(
        locked.commit.as_deref(),
        Some(fx.repo.git(&["rev-parse", "HEAD"]).as_str())
    );
    assert!(check(&fx.root).unwrap().is_ok());
    // The comments the file opens with are still there.
    assert!(fx.manifest().starts_with("// The files catasto manages"));
}

#[test]
fn a_second_file_leaves_the_first_where_it_was() {
    let fx = Fixture::new();
    fx.ready();
    fx.add("a.tmt", "a.tmt").unwrap();
    let first = Lock::load(&fx.root)
        .unwrap()
        .get(Path::new("a.tmt"))
        .cloned();

    fx.repo.commit("a.tmt", "a two");
    fx.add("b.tmt", "sub/b.tmt").unwrap();

    // `a` stays on the commit it was locked at; only `b` is read.
    assert_eq!(fx.read("a.tmt"), "a one");
    assert_eq!(
        Lock::load(&fx.root)
            .unwrap()
            .get(Path::new("a.tmt"))
            .cloned(),
        first
    );
    assert_eq!(fx.read("b.tmt"), "b one");
    assert!(check(&fx.root).unwrap().is_ok());
}

#[test]
fn a_file_from_a_repository_written_out_and_a_local_file_are_added_too() {
    let fx = Fixture::new();
    init(&fx.root).unwrap();
    add(
        &fx.root,
        "x.tmt",
        &SourceText::Git {
            git: fx.repo.url(),
            rev: "main".into(),
            path: "a.tmt".into(),
            keep_symlink: false,
        },
        false,
        Some(&fx.cache),
    )
    .unwrap();
    std::fs::create_dir_all(fx.root.join("../shared")).unwrap();
    std::fs::write(fx.root.join("../shared/l.tmt"), "local").unwrap();
    add(
        &fx.root,
        "l.tmt",
        &SourceText::Local {
            path: "../shared/l.tmt".into(),
        },
        false,
        Some(&fx.cache),
    )
    .unwrap();
    assert_eq!(fx.read("x.tmt"), "a one");
    assert_eq!(fx.read("l.tmt"), "local");
    assert!(check(&fx.root).unwrap().is_ok());
}

#[test]
fn a_failed_add_leaves_the_manifest_as_it_was() {
    let fx = Fixture::new();
    fx.ready();
    let before = fx.manifest();
    let err = fx.add("nope.tmt", "no/such/file.tmt").unwrap_err();
    assert!(
        err.to_string().contains("could not add nope.tmt"),
        "{err:#}"
    );
    assert_eq!(fx.manifest(), before);
    assert!(!fx.root.join("nope.tmt").exists());
    assert!(Lock::load(&fx.root).unwrap().locked.is_empty());
}

#[test]
fn an_upstream_that_is_not_named_is_refused_with_the_names_there_are() {
    let fx = Fixture::new();
    fx.ready();
    let before = fx.manifest();
    let err = add(
        &fx.root,
        "a.tmt",
        &SourceText::Upstream {
            name: "nope".into(),
            path: "a.tmt".into(),
            keep_symlink: false,
        },
        false,
        Some(&fx.cache),
    )
    .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("unknown upstream `nope` (declared: t)"),
        "{text}"
    );
    assert!(text.contains("catasto upstream add"), "{text}");
    assert_eq!(fx.manifest(), before);
}

#[test]
fn adding_a_declared_path_again_is_refused() {
    let fx = Fixture::new();
    fx.ready();
    fx.add("a.tmt", "a.tmt").unwrap();
    let before = fx.manifest();
    let err = fx.add("a.tmt", "a.tmt").unwrap_err();
    let already = err.downcast_ref::<AlreadyDeclared>().expect("AlreadyDeclared");
    assert_eq!(already.dest, "a.tmt");
    assert!(
        err.to_string().contains("a.tmt is already declared (use --force to replace its source)"),
        "{err}"
    );
    assert_eq!(fx.manifest(), before);
}

#[test]
fn a_path_outside_the_workspace_is_refused_before_anything_is_written() {
    let fx = Fixture::new();
    fx.ready();
    let before = fx.manifest();
    let err = fx.add("../escape.tmt", "a.tmt").unwrap_err();
    assert!(
        format!("{err:#}").contains("inside the workspace"),
        "{err:#}"
    );
    assert_eq!(fx.manifest(), before);
}

#[test]
fn remove_forgets_the_entry_and_the_lock_and_keeps_the_file_unless_told() {
    let fx = Fixture::new();
    fx.ready();
    fx.add("a.tmt", "a.tmt").unwrap();
    fx.add("b.tmt", "sub/b.tmt").unwrap();

    remove(&fx.root, "a.tmt", false).unwrap();
    assert!(fx.root.join("a.tmt").exists());
    assert!(!fx.manifest().contains("\"a.tmt\""));
    let lock = Lock::load(&fx.root).unwrap();
    assert!(lock.get(Path::new("a.tmt")).is_none());
    assert!(lock.get(Path::new("b.tmt")).is_some());
    assert!(check(&fx.root).unwrap().is_ok());

    remove(&fx.root, "b.tmt", true).unwrap();
    assert!(!fx.root.join("b.tmt").exists());
    assert!(Lock::load(&fx.root).unwrap().locked.is_empty());
}

#[test]
fn removing_what_is_not_declared_says_so() {
    let fx = Fixture::new();
    fx.ready();
    let err = remove(&fx.root, "nope.tmt", false).unwrap_err();
    assert!(err.to_string().contains("not declared"), "{err}");
}

#[test]
fn an_upstream_in_use_cannot_be_removed_until_nothing_uses_it() {
    let fx = Fixture::new();
    fx.ready();
    fx.add("a.tmt", "a.tmt").unwrap();
    let err = upstream_remove(&fx.root, "t").unwrap_err();
    assert!(err.to_string().contains("used by a.tmt"), "{err}");

    remove(&fx.root, "a.tmt", false).unwrap();
    upstream_remove(&fx.root, "t").unwrap();
    assert!(upstream_list(&fx.root).unwrap().is_empty());
}

#[test]
fn the_upstreams_are_listed_in_order() {
    let fx = Fixture::new();
    fx.ready();
    upstream_add(
        &fx.root,
        "second",
        &UpstreamText {
            git: "https://example.com/s.git".into(),
            rev: "v1".into(),
            root: Some("dir".into()),
        },
    )
    .unwrap();
    let names: Vec<_> = upstream_list(&fx.root)
        .unwrap()
        .into_iter()
        .map(|u| (u.name, u.root))
        .collect();
    assert_eq!(
        names,
        [
            ("t".to_owned(), None),
            ("second".to_owned(), Some("dir".into()))
        ]
    );
}

#[test]
fn list_shows_where_each_file_comes_from_and_how_it_stands() {
    let fx = Fixture::new();
    fx.ready();
    fx.add("a.tmt", "a.tmt").unwrap();
    fx.add("b.tmt", "sub/b.tmt").unwrap();
    std::fs::write(fx.root.join("b.tmt"), "edited").unwrap();

    let rows = list(&fx.root).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].path, Path::new("a.tmt"));
    assert_eq!(rows[0].from, "t:a.tmt");
    assert_eq!(rows[0].state, None);
    assert!(rows[0].commit.is_some());
    assert_eq!(rows[1].from, "t:sub/b.tmt");
    assert_eq!(rows[1].state, Some(ViolationKind::Modified));
}

#[test]
fn syncing_one_file_moves_only_that_file() {
    let fx = Fixture::new();
    fx.ready();
    fx.add("a.tmt", "a.tmt").unwrap();
    fx.add("b.tmt", "sub/b.tmt").unwrap();
    fx.repo.commit("a.tmt", "a two");
    fx.repo.commit("sub/b.tmt", "b two");
    let b_before = Lock::load(&fx.root)
        .unwrap()
        .get(Path::new("b.tmt"))
        .cloned();

    sync_only(
        &fx.root,
        SyncMode::Update,
        Some(&fx.cache),
        &[PathBuf::from("a.tmt")],
    )
    .unwrap();

    assert_eq!(fx.read("a.tmt"), "a two");
    assert_eq!(fx.read("b.tmt"), "b one");
    assert_eq!(
        Lock::load(&fx.root)
            .unwrap()
            .get(Path::new("b.tmt"))
            .cloned(),
        b_before
    );
    assert!(check(&fx.root).unwrap().is_ok());
}

#[test]
fn syncing_a_file_that_is_not_declared_is_refused() {
    let fx = Fixture::new();
    fx.ready();
    let err = sync_only(
        &fx.root,
        SyncMode::Locked,
        Some(&fx.cache),
        &[PathBuf::from("x")],
    )
    .unwrap_err();
    assert!(err.to_string().contains("not declared"), "{err}");
}

#[test]
#[cfg(unix)]
fn add_with_keep_symlink_writes_fetches_and_locks_a_link() {
    let fx = Fixture::new();
    fx.ready();
    fx.repo.commit_symlink("link.tmt", "target.tmt");
    fx.repo.commit("target.tmt", "target content");

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

    let placed = fx.root.join("link.tmt");
    let meta = std::fs::symlink_metadata(&placed).unwrap();
    assert!(meta.is_symlink());
    assert_eq!(
        std::fs::read_link(&placed).unwrap().to_str().unwrap(),
        "target.tmt"
    );

    let lock = Lock::load(&fx.root).unwrap();
    let locked = lock.get(Path::new("link.tmt")).expect("locked");
    assert!(locked.commit.is_some());
    assert!(check(&fx.root).unwrap().is_ok());
}

#[test]
#[cfg(unix)]
fn add_with_keep_symlink_on_regular_file_fails_and_leaves_everything_as_it_was() {
    let fx = Fixture::new();
    fx.ready();
    let manifest_before = fx.manifest();
    let lock_before = std::fs::read_to_string(fx.root.join(LOCK_FILE)).ok();

    let err = add(
        &fx.root,
        "not_a_link.tmt",
        &SourceText::Upstream {
            name: "t".into(),
            path: "a.tmt".into(),
            keep_symlink: true,
        },
        false,
        Some(&fx.cache),
    )
    .unwrap_err();

    assert!(format!("{err:#}").contains("not a symlink"));
    assert_eq!(fx.manifest(), manifest_before);
    let lock_after = std::fs::read_to_string(fx.root.join(LOCK_FILE)).ok();
    assert_eq!(lock_after, lock_before);
    assert!(!fx.root.join("not_a_link.tmt").exists());
}

#[test]
fn add_force_outcomes_and_guarantees() {
    let fx = Fixture::new();
    fx.ready();

    // 1. Added
    let outcome = fx.add("docs/file.tmt", "a.tmt").unwrap();
    assert_eq!(outcome, AddOutcome::Added);
    assert_eq!(fx.read("docs/file.tmt"), "a one");

    // 2. Unchanged when replaced with the exact same source
    let manifest_before = fx.manifest();
    let outcome = fx.add_force("docs/file.tmt", "a.tmt").unwrap();
    assert_eq!(outcome, AddOutcome::Unchanged);
    assert_eq!(fx.manifest(), manifest_before);

    // 3. Replaced when given a different source
    let outcome = fx.add_force("docs/file.tmt", "sub/b.tmt").unwrap();
    assert_eq!(outcome, AddOutcome::Replaced);
    assert_eq!(fx.read("docs/file.tmt"), "b one");
    assert!(check(&fx.root).unwrap().is_ok());

    // 4. Failing fetch on a --force replace leaves manifest, lock and file as they were
    let manifest_before = fx.manifest();
    let lock_before = std::fs::read_to_string(fx.root.join(LOCK_FILE)).unwrap();
    let file_before = fx.read("docs/file.tmt");

    let err = fx.add_force("docs/file.tmt", "no/such/file.tmt").unwrap_err();
    assert!(format!("{err:#}").contains("could not add"));

    assert_eq!(fx.manifest(), manifest_before);
    assert_eq!(
        std::fs::read_to_string(fx.root.join(LOCK_FILE)).unwrap(),
        lock_before
    );
    assert_eq!(fx.read("docs/file.tmt"), file_before);
    assert!(check(&fx.root).unwrap().is_ok());
}
