use std::path::PathBuf;

use super::*;
use crate::check::check;
use catasto_git::testing::TestRepo;

const MANIFEST: &str = "\
\"vocab/writ.tmt\": { source: \"../vocabularies/writ.tmt\" }
";

fn sync(root: &Path) -> Result<Lock> {
    super::sync(root, SyncMode::Locked)
}

fn workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let source = tmp.path().join("vocabularies/writ.tmt");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(vault.join("catasto.ctmt"), MANIFEST).unwrap();
    std::fs::write(&source, "canonical").unwrap();
    (tmp, vault, source)
}

#[test]
fn fetches_a_file_that_does_not_exist_yet() {
    let (_tmp, vault, _) = workspace();

    sync(&vault).unwrap();

    assert_eq!(
        std::fs::read_to_string(vault.join("vocab/writ.tmt")).unwrap(),
        "canonical"
    );
    assert!(check(&vault).unwrap().is_ok());
}

#[test]
fn overwrites_a_drifted_file_and_clears_every_violation() {
    let (_tmp, vault, source) = workspace();
    sync(&vault).unwrap();
    std::fs::write(vault.join("vocab/writ.tmt"), "edited").unwrap();
    std::fs::write(&source, "canonical v2").unwrap();
    assert!(!check(&vault).unwrap().is_ok());

    sync(&vault).unwrap();

    assert_eq!(
        std::fs::read_to_string(vault.join("vocab/writ.tmt")).unwrap(),
        "canonical v2"
    );
    assert!(check(&vault).unwrap().is_ok());
}

#[test]
fn a_missing_source_leaves_the_workspace_untouched() {
    let (_tmp, vault, source) = workspace();
    std::fs::create_dir_all(vault.join("vocab")).unwrap();
    std::fs::write(vault.join("vocab/writ.tmt"), "local").unwrap();
    std::fs::remove_file(&source).unwrap();

    let err = sync(&vault).unwrap_err();

    assert!(
        err.to_string().contains("source of vocab/writ.tmt"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(vault.join("vocab/writ.tmt")).unwrap(),
        "local"
    );
    assert!(!vault.join("catasto.lock").exists());
}

/// A vault whose one file comes from the `main` branch of a test repository.
struct GitFixture {
    _tmp: tempfile::TempDir,
    vault: PathBuf,
    cache: PathBuf,
    repo: TestRepo,
}

impl GitFixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(&vault).unwrap();
        let repo = TestRepo::new();
        repo.commit("shared/w.tmt", "one");
        std::fs::write(
                vault.join("catasto.ctmt"),
                format!(
                    "\"vocab/w.tmt\": {{ source: {{ git: {:?}, rev: main, path: \"shared/w.tmt\" }} }}\n",
                    repo.url()
                ),
            )
            .unwrap();
        Self {
            _tmp: tmp,
            vault,
            cache,
            repo,
        }
    }

    fn sync(&self, mode: SyncMode) -> Lock {
        sync_with(&self.vault, mode, Some(&self.cache)).unwrap()
    }

    fn content(&self) -> String {
        std::fs::read_to_string(self.vault.join("vocab/w.tmt")).unwrap()
    }
}

#[test]
fn syncing_a_git_source_writes_the_file_and_locks_the_commit() {
    let fx = GitFixture::new();
    let head = fx.repo.git(&["rev-parse", "HEAD"]);

    let lock = fx.sync(SyncMode::Locked);

    assert_eq!(fx.content(), "one");
    assert_eq!(lock.locked[0].commit.as_deref(), Some(head.as_str()));
    assert!(check(&fx.vault).unwrap().is_ok());
}

#[test]
fn a_locked_sync_stays_on_the_pinned_commit_after_the_branch_moves() {
    let fx = GitFixture::new();
    let pinned = fx.sync(SyncMode::Locked).locked[0].commit.clone();
    fx.repo.commit("shared/w.tmt", "two");

    let lock = fx.sync(SyncMode::Locked);

    assert_eq!(fx.content(), "one");
    assert_eq!(lock.locked[0].commit, pinned);
}

#[test]
fn a_locked_sync_restores_a_modified_file_to_the_pinned_commit() {
    let fx = GitFixture::new();
    fx.sync(SyncMode::Locked);
    std::fs::write(fx.vault.join("vocab/w.tmt"), "edited").unwrap();
    fx.repo.commit("shared/w.tmt", "two");

    fx.sync(SyncMode::Locked);

    assert_eq!(fx.content(), "one");
    assert!(check(&fx.vault).unwrap().is_ok());
}

#[test]
fn update_moves_to_where_the_rev_points_now() {
    let fx = GitFixture::new();
    fx.sync(SyncMode::Locked);
    let moved = fx.repo.commit("shared/w.tmt", "two");

    let lock = fx.sync(SyncMode::Update);

    assert_eq!(fx.content(), "two");
    assert_eq!(lock.locked[0].commit.as_deref(), Some(moved.as_str()));
    assert!(check(&fx.vault).unwrap().is_ok());
}

#[test]
fn a_changed_rev_is_resolved_even_in_locked_mode() {
    let fx = GitFixture::new();
    fx.sync(SyncMode::Locked);
    fx.repo.tag("v1");
    let moved = fx.repo.commit("shared/w.tmt", "two");
    fx.repo.tag("v2");
    let manifest = std::fs::read_to_string(fx.vault.join("catasto.ctmt")).unwrap();
    std::fs::write(
        fx.vault.join("catasto.ctmt"),
        manifest.replace("rev: main", "rev: v2"),
    )
    .unwrap();

    let lock = fx.sync(SyncMode::Locked);

    assert_eq!(fx.content(), "two");
    assert_eq!(lock.locked[0].commit.as_deref(), Some(moved.as_str()));
}

#[test]
fn a_relative_git_path_is_resolved_against_the_workspace_root() {
    let fx = GitFixture::new();
    // Both directories are created in the system temp directory, so the
    // repository is `../../<name>` from `<tmp>/vault`.
    let repo_name = Path::new(&fx.repo.url())
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let manifest = std::fs::read_to_string(fx.vault.join("catasto.ctmt")).unwrap();
    std::fs::write(
        fx.vault.join("catasto.ctmt"),
        manifest.replace(&fx.repo.url(), &format!("../../{repo_name}")),
    )
    .unwrap();

    let lock = fx.sync(SyncMode::Locked);

    assert_eq!(fx.content(), "one");
    assert!(lock.locked[0].commit.is_some());
    assert!(check(&fx.vault).unwrap().is_ok());
}

/// A vault with entries from two repositories, written in an order that
/// alternates between them.
fn two_repositories() -> (tempfile::TempDir, PathBuf, PathBuf, TestRepo, TestRepo) {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let cache = tmp.path().join("cache");
    std::fs::create_dir_all(&vault).unwrap();
    let first = TestRepo::new();
    first.commit("a1.tmt", "first a1");
    first.commit("a2.tmt", "first a2");
    let second = TestRepo::new();
    second.commit("b1.tmt", "second b1");
    std::fs::write(
        vault.join("catasto.ctmt"),
        format!(
            "\"v/a1.tmt\": {{ source: {{ git: {first:?}, rev: main, path: \"a1.tmt\" }} }}\n\
                 \"v/b1.tmt\": {{ source: {{ git: {second:?}, rev: main, path: \"b1.tmt\" }} }}\n\
                 \"v/a2.tmt\": {{ source: {{ git: {first:?}, rev: main, path: \"a2.tmt\" }} }}\n",
            first = first.url(),
            second = second.url(),
        ),
    )
    .unwrap();
    (tmp, vault, cache, first, second)
}

#[test]
fn repositories_are_read_together_and_each_entry_gets_its_own_commit() {
    let (_tmp, vault, cache, first, second) = two_repositories();

    let lock = sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();

    for (file, content) in [
        ("v/a1.tmt", "first a1"),
        ("v/b1.tmt", "second b1"),
        ("v/a2.tmt", "first a2"),
    ] {
        assert_eq!(
            std::fs::read_to_string(vault.join(file)).unwrap(),
            content,
            "{file}"
        );
    }
    let commit = |file: &str| lock.get(Path::new(file)).unwrap().commit.clone().unwrap();
    assert_eq!(commit("v/a1.tmt"), first.git(&["rev-parse", "HEAD"]));
    assert_eq!(commit("v/a2.tmt"), first.git(&["rev-parse", "HEAD"]));
    assert_eq!(commit("v/b1.tmt"), second.git(&["rev-parse", "HEAD"]));
    assert!(check(&vault).unwrap().is_ok());
}

#[test]
fn several_repositories_stay_pinned_until_update() {
    let (_tmp, vault, cache, first, second) = two_repositories();
    sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();
    first.commit("a1.tmt", "moved a1");
    second.commit("b1.tmt", "moved b1");

    sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();
    assert_eq!(
        std::fs::read_to_string(vault.join("v/b1.tmt")).unwrap(),
        "second b1"
    );

    sync_with(&vault, SyncMode::Update, Some(&cache)).unwrap();
    assert_eq!(
        std::fs::read_to_string(vault.join("v/a1.tmt")).unwrap(),
        "moved a1"
    );
    assert_eq!(
        std::fs::read_to_string(vault.join("v/b1.tmt")).unwrap(),
        "moved b1"
    );
}

#[test]
fn the_error_reported_is_the_first_in_manifest_order() {
    let (_tmp, vault, cache, first, second) = two_repositories();
    // Both repositories fail, the second one's entry is written first.
    std::fs::write(
            vault.join("catasto.ctmt"),
            format!(
                "\"v/b.tmt\": {{ source: {{ git: {second:?}, rev: main, path: \"missing-b.tmt\" }} }}\n\
                 \"v/a.tmt\": {{ source: {{ git: {first:?}, rev: main, path: \"missing-a.tmt\" }} }}\n",
                first = first.url(),
                second = second.url(),
            ),
        )
        .unwrap();

    for _ in 0..5 {
        let err = sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap_err();
        assert!(err.to_string().contains("source of v/b.tmt"), "{err}");
    }
    assert!(!vault.join("v").exists());
}

#[test]
fn a_git_failure_leaves_the_workspace_untouched() {
    let fx = GitFixture::new();
    let manifest = std::fs::read_to_string(fx.vault.join("catasto.ctmt")).unwrap();
    std::fs::write(
        fx.vault.join("catasto.ctmt"),
        manifest.replace("rev: main", "rev: nope"),
    )
    .unwrap();

    let err = super::sync_with(&fx.vault, SyncMode::Locked, Some(&fx.cache)).unwrap_err();

    assert!(
        format!("{err:#}").contains("source of vocab/w.tmt"),
        "{err:#}"
    );
    assert!(!fx.vault.join("vocab/w.tmt").exists());
    assert!(!fx.vault.join("catasto.lock").exists());
}

#[test]
#[cfg(unix)]
fn sync_symlink_follow_and_keep_and_switching() {
    let repo = TestRepo::new();
    repo.commit("target.txt", "real content");
    repo.commit_symlink("link.txt", "target.txt");

    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let cache = tmp.path().join("cache");
    std::fs::create_dir_all(&vault).unwrap();

    // 1. follow places the target's content as a regular file
    std::fs::write(
        vault.join("catasto.ctmt"),
        format!(
            "\"out.txt\": {{ source: {{ git: {:?}, rev: main, path: \"link.txt\" }} }}",
            repo.url()
        ),
    )
    .unwrap();
    sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();

    let out = vault.join("out.txt");
    let meta = std::fs::symlink_metadata(&out).unwrap();
    assert!(meta.is_file());
    assert!(!meta.is_symlink());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "real content");
    assert!(check(&vault).unwrap().is_ok());

    // 2. switch from follow to keep: sync replaces destination with symlink
    std::fs::write(
            vault.join("catasto.ctmt"),
            format!("\"out.txt\": {{ source: {{ git: {:?}, rev: main, path: \"link.txt\", symlink: keep }} }}", repo.url()),
        ).unwrap();
    sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();

    let meta = std::fs::symlink_metadata(&out).unwrap();
    assert!(meta.is_symlink());
    assert_eq!(
        std::fs::read_link(&out).unwrap().to_str().unwrap(),
        "target.txt"
    );
    assert!(check(&vault).unwrap().is_ok());

    // 3. switch from keep to follow: sync replaces symlink with regular file
    std::fs::write(
        vault.join("catasto.ctmt"),
        format!(
            "\"out.txt\": {{ source: {{ git: {:?}, rev: main, path: \"link.txt\" }} }}",
            repo.url()
        ),
    )
    .unwrap();
    sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();

    let meta = std::fs::symlink_metadata(&out).unwrap();
    assert!(meta.is_file());
    assert!(!meta.is_symlink());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "real content");
    assert!(check(&vault).unwrap().is_ok());
}

#[test]
#[cfg(unix)]
fn sync_follow_never_writes_through_destination_symlink() {
    let repo = TestRepo::new();
    repo.commit("src.txt", "new content from git");

    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let cache = tmp.path().join("cache");
    std::fs::create_dir_all(&vault).unwrap();

    // A secret file
    let victim = tmp.path().join("victim.txt");
    std::fs::write(&victim, "victim original content").unwrap();

    // Destination is currently a symlink pointing to victim
    let dest = vault.join("dest.txt");
    std::os::unix::fs::symlink(&victim, &dest).unwrap();

    std::fs::write(
        vault.join("catasto.ctmt"),
        format!(
            "\"dest.txt\": {{ source: {{ git: {:?}, rev: main, path: \"src.txt\" }} }}",
            repo.url()
        ),
    )
    .unwrap();

    // Sync in follow mode
    sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();

    // The destination should now be a regular file with new content
    let meta = std::fs::symlink_metadata(&dest).unwrap();
    assert!(meta.is_file());
    assert!(!meta.is_symlink());
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "new content from git"
    );

    // The victim file pointed to by the symlink MUST NOT be modified!
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "victim original content"
    );
}

#[test]
#[cfg(unix)]
fn sync_keeps_the_mode_of_an_existing_destination() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::new();
    repo.commit("run.sh", "v1");

    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let cache = tmp.path().join("cache");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::write(
        vault.join("catasto.ctmt"),
        format!(
            "\"run.sh\": {{ source: {{ git: {:?}, rev: main, path: \"run.sh\" }} }}",
            repo.url()
        ),
    )
    .unwrap();
    sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();

    let dest = vault.join("run.sh");
    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o750)).unwrap();
    std::fs::write(&dest, "edited").unwrap();
    sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();

    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "v1");
    let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o750);
}

#[test]
#[cfg(unix)]
fn sync_bad_link_source_leaves_workspace_untouched() {
    let repo = TestRepo::new();
    repo.commit_symlink("loop_a", "loop_b");
    repo.commit_symlink("loop_b", "loop_a");

    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let cache = tmp.path().join("cache");
    std::fs::create_dir_all(&vault).unwrap();

    std::fs::write(
        vault.join("catasto.ctmt"),
        format!(
            "\"out.txt\": {{ source: {{ git: {:?}, rev: main, path: \"loop_a\" }} }}",
            repo.url()
        ),
    )
    .unwrap();

    let err = sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap_err();
    assert!(
        format!("{err:#}").contains("symlink loop detected"),
        "{err:#}"
    );
    assert!(!vault.join("out.txt").exists());
    assert!(!vault.join("catasto.lock").exists());
}

#[test]
fn dry_run_with_moved_commit_reports_changes_and_leaves_workspace_untouched() {
    let repo = TestRepo::new();
    repo.commit("a.tmt", "a one\n");
    let c1 = repo.commit("b.tmt", "b one\n");

    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let cache = tmp.path().join("cache");
    std::fs::create_dir_all(&vault).unwrap();

    let manifest_text = format!(
        "::upstreams{{\n  common: {{ git: {:?}, rev: main }}\n}}\n\n\"a.tmt\": {{ source: {{ upstream: common, path: \"a.tmt\" }} }}\n\"b.tmt\": {{ source: {{ upstream: common, path: \"b.tmt\" }} }}\n",
        repo.url()
    );
    std::fs::write(vault.join("catasto.ctmt"), &manifest_text).unwrap();

    // Initial sync
    sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();

    // Move branch on repo by committing a.tmt
    let c2 = repo.commit("a.tmt", "a two\n");

    let manifest_before = std::fs::read_to_string(vault.join("catasto.ctmt")).unwrap();
    let lock_before = std::fs::read_to_string(vault.join("catasto.lock")).unwrap();
    let a_before = std::fs::read_to_string(vault.join("a.tmt")).unwrap();
    let b_before = std::fs::read_to_string(vault.join("b.tmt")).unwrap();

    // 1. Dry run in Update mode
    let report = sync_dry_run(&vault, SyncMode::Update, Some(&cache), &[]).unwrap();
    assert_eq!(report.total_files, 2);
    assert_eq!(report.changed_files, 1);
    assert_eq!(report.moved_sources.len(), 1);
    assert_eq!(report.moved_sources[0].label, "common");
    assert_eq!(report.moved_sources[0].rev, "main");
    assert_eq!(report.moved_sources[0].from, &c1[..7]);
    assert_eq!(report.moved_sources[0].to, &c2[..7]);

    let output = &report.output;
    assert!(
        output.starts_with(&format!("common main: {} -> {}\n", &c1[..7], &c2[..7])),
        "{output}"
    );
    assert!(output.contains("--- a/a.tmt"), "{output}");
    assert!(output.contains("+++ b/a.tmt\tgit "), "{output}");
    assert!(output.contains("-a one"), "{output}");
    assert!(output.contains("+a two"), "{output}");
    assert!(!output.contains("b.tmt"), "{output}");
    assert!(output.ends_with("would change 1 of 2 file(s)\n"), "{output}");

    // Assert workspace files and lock are byte-for-byte untouched!
    assert_eq!(
        std::fs::read_to_string(vault.join("catasto.ctmt")).unwrap(),
        manifest_before
    );
    assert_eq!(
        std::fs::read_to_string(vault.join("catasto.lock")).unwrap(),
        lock_before
    );
    assert_eq!(
        std::fs::read_to_string(vault.join("a.tmt")).unwrap(),
        a_before
    );
    assert_eq!(
        std::fs::read_to_string(vault.join("b.tmt")).unwrap(),
        b_before
    );

    // 2. Dry run with --only b.tmt (its upstream moved, but file content didn't change)
    let report_b =
        sync_dry_run(&vault, SyncMode::Update, Some(&cache), &[PathBuf::from("b.tmt")]).unwrap();
    assert_eq!(report_b.total_files, 1);
    assert_eq!(report_b.changed_files, 0);
    assert_eq!(
        report_b.output,
        format!("common main: {} -> {}\nnothing would change\n", &c1[..7], &c2[..7])
    );

    // 3. Real sync --update makes exactly the changes shown
    sync_with(&vault, SyncMode::Update, Some(&cache)).unwrap();
    assert_eq!(std::fs::read_to_string(vault.join("a.tmt")).unwrap(), "a two\n");
    assert_eq!(std::fs::read_to_string(vault.join("b.tmt")).unwrap(), "b one\n");

    // 4. Following dry-run now reports nothing would change
    let report_after = sync_dry_run(&vault, SyncMode::Update, Some(&cache), &[]).unwrap();
    assert_eq!(report_after.output, "nothing would change\n");
}

#[test]
#[cfg(unix)]
fn sync_executable_file_creates_mode_and_update_tracks_flipping() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::new();
    repo.commit("normal.txt", "normal file");
    repo.commit_executable("tool.sh", "#!/bin/sh\necho hi\n");

    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let cache = tmp.path().join("cache");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::write(
        vault.join("catasto.ctmt"),
        format!(
            "\"normal.txt\": {{ source: {{ git: {:?}, rev: main, path: \"normal.txt\" }} }}\n\
             \"tool.sh\": {{ source: {{ git: {:?}, rev: main, path: \"tool.sh\" }} }}\n",
            repo.url(),
            repo.url()
        ),
    )
    .unwrap();

    // 1. Initial sync creates normal file non-executable and tool.sh executable
    let lock1 = sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();
    let normal_mode = std::fs::metadata(vault.join("normal.txt"))
        .unwrap()
        .permissions()
        .mode();
    let tool_mode = std::fs::metadata(vault.join("tool.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(normal_mode & 0o111, 0);
    assert_ne!(tool_mode & 0o111, 0);

    // Verify lockfile records mode: executable for tool.sh, absent for normal.txt
    assert_eq!(lock1.get(Path::new("normal.txt")).unwrap().mode, None);
    assert_eq!(
        lock1.get(Path::new("tool.sh")).unwrap().mode,
        Some(LockMode::Executable)
    );
    let lock_text = std::fs::read_to_string(vault.join("catasto.lock")).unwrap();
    assert!(lock_text.contains("\"mode\": \"executable\""), "{lock_text}");

    // 2. Upstream flips: tool.sh becomes non-executable (chmod -x), normal.txt becomes executable (chmod +x)
    repo.commit_non_executable("tool.sh", "#!/bin/sh\nnot executable now\n");
    repo.commit_executable("normal.txt", "normal now executable");

    // Locked sync preserves previous state
    sync_with(&vault, SyncMode::Locked, Some(&cache)).unwrap();
    let tool_mode_locked = std::fs::metadata(vault.join("tool.sh"))
        .unwrap()
        .permissions()
        .mode();
    let normal_mode_locked = std::fs::metadata(vault.join("normal.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(tool_mode_locked & 0o111, 0);
    assert_eq!(normal_mode_locked & 0o111, 0);

    // Update sync picks up the flipped modes
    let lock2 = sync_with(&vault, SyncMode::Update, Some(&cache)).unwrap();
    let tool_mode_updated = std::fs::metadata(vault.join("tool.sh"))
        .unwrap()
        .permissions()
        .mode();
    let normal_mode_updated = std::fs::metadata(vault.join("normal.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(tool_mode_updated & 0o111, 0);
    assert_ne!(normal_mode_updated & 0o111, 0);
    assert_eq!(lock2.get(Path::new("tool.sh")).unwrap().mode, None);
    assert_eq!(
        lock2.get(Path::new("normal.txt")).unwrap().mode,
        Some(LockMode::Executable)
    );
}

