//! The commands that change what a workspace declares: `init`, `add`,
//! `remove`, `upstream`, and `list` to read it back.
//!
//! Each edit goes through [`crate::edit`], which keeps the file as the user
//! wrote it, and is parsed again before it is written; a manifest that would
//! not load is never saved. The file is replaced by renaming a temporary
//! beside it, so a crash leaves the old one or the new one.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::check::{ViolationKind, check};
use crate::edit::{self, SourceText, UpstreamText};
use crate::lockfile::{LOCK_FILE, Lock};
use crate::manifest::{
    MANIFEST_FILE, Manifest, Source, Symlink, UpstreamInfo, reject_misnamed, upstreams_of,
};
use crate::sync::{SyncMode, sync_only};

/// What `catasto init` writes: comments only, which is a valid empty manifest.
const SKELETON: &str = "\
// The files catasto manages in this workspace. Each key is a path here;
// `source` is where its canonical content comes from.
//
// Name a place once and take files from it:
//
//   catasto upstream add shared https://example.com/shared.git
//   catasto add docs/terms.tmt --upstream shared terms.tmt
//
// which leaves, below this comment,
//
//   ::upstreams{
//     shared: { git: \"https://example.com/shared.git\", rev: main }
//   }
//
//   \"docs/terms.tmt\": {
//     source: { upstream: shared, path: \"terms.tmt\" }
//   }
";

/// Creates `catasto.ctmt` in `root`. It never overwrites one.
pub fn init(root: &Path) -> Result<PathBuf> {
    if !root.is_dir() {
        bail!("not a directory: {}", root.display());
    }
    let file = root.join(MANIFEST_FILE);
    if file.exists() {
        bail!("{} already exists", file.display());
    }
    reject_misnamed(root, MANIFEST_FILE)?;
    std::fs::write(&file, SKELETON).with_context(|| format!("cannot write {}", file.display()))?;
    Ok(file)
}

/// Declares `dest` with `source`, fetches it and locks it. The other files and
/// what the lock says about them are left alone. If the fetch fails, so does
/// the declaration: the manifest is put back.
pub fn add(root: &Path, dest: &str, source: &SourceText, cache_dir: Option<&Path>) -> Result<()> {
    let dest = clean_dest(dest);
    if let SourceText::Upstream { name, .. } = source {
        let known = upstream_list(root)?;
        if !known.iter().any(|upstream| &upstream.name == name) {
            let names: Vec<_> = known
                .iter()
                .map(|upstream| upstream.name.as_str())
                .collect();
            let have = if names.is_empty() {
                "none is declared".to_owned()
            } else {
                format!("declared: {}", names.join(", "))
            };
            bail!("unknown upstream `{name}` ({have}); name one with `catasto upstream add`");
        }
    }
    let before = edit_manifest(root, |text| edit::add_entry(text, &dest, source))?;
    let result = sync_only(root, SyncMode::Locked, cache_dir, &[PathBuf::from(&dest)]);
    if let Err(error) = result {
        write_atomic(&root.join(MANIFEST_FILE), &before)?;
        return Err(error.context(format!("could not add {dest}")));
    }
    Ok(())
}

/// Undeclares `dest` and forgets what the lock says about it. The file stays on
/// disk unless `delete_file`.
pub fn remove(root: &Path, dest: &str, delete_file: bool) -> Result<()> {
    let dest = clean_dest(dest);
    edit_manifest(root, |text| edit::remove_entry(text, &dest))?;

    let lock = Lock::load(root)?;
    let kept: Vec<_> = lock
        .locked
        .into_iter()
        .filter(|locked| locked.path != Path::new(&dest))
        .collect();
    let file = root.join(LOCK_FILE);
    std::fs::write(&file, Lock::new(kept).render()?)
        .with_context(|| format!("cannot write {}", file.display()))?;

    if delete_file {
        match std::fs::remove_file(root.join(&dest)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("cannot delete {dest}")),
        }
    }
    Ok(())
}

/// Names a place in `::upstreams`.
pub fn upstream_add(root: &Path, name: &str, upstream: &UpstreamText) -> Result<()> {
    edit_manifest(root, |text| edit::add_upstream(text, name, upstream))?;
    Ok(())
}

/// Drops a place from `::upstreams`; refused while an entry still uses it.
pub fn upstream_remove(root: &Path, name: &str) -> Result<()> {
    edit_manifest(root, |text| {
        let users = edit::upstream_is_used(text, name)?;
        if !users.is_empty() {
            bail!("upstream `{name}` is used by {}", users.join(", "));
        }
        edit::remove_upstream(text, name)
    })?;
    Ok(())
}

/// The places `::upstreams` names.
pub fn upstream_list(root: &Path) -> Result<Vec<UpstreamInfo>> {
    upstreams_of(&read_manifest(root)?)
}

/// One declared file, as `catasto list` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Location inside the workspace.
    pub path: PathBuf,
    /// Where it comes from: `name:path` for an upstream, `git url@rev:path` for
    /// a repository written out, `local path` for a file.
    pub from: String,
    /// How it has drifted, if it has.
    pub state: Option<ViolationKind>,
    /// The commit it was locked at, for a git source.
    pub commit: Option<String>,
    /// The symlink mode if explicitly keep.
    pub symlink: Option<Symlink>,
}

/// Every declared file with its source and its state.
pub fn list(root: &Path) -> Result<Vec<Row>> {
    let manifest = Manifest::load(root)?;
    let lock = Lock::load(root)?;
    let report = check(root)?;
    let upstreams = match std::fs::read_to_string(root.join(MANIFEST_FILE)) {
        Ok(text) => upstreams_of(&text)?,
        Err(_) => Vec::new(),
    };

    Ok(manifest
        .managed
        .iter()
        .map(|managed| Row {
            path: managed.path.clone(),
            from: describe(&managed.source, &upstreams),
            state: report
                .violations
                .iter()
                .find(|violation| violation.path == managed.path)
                .map(|violation| violation.kind),
            commit: lock
                .get(&managed.path)
                .and_then(|locked| locked.commit.clone()),
            symlink: match managed.source {
                Source::Git {
                    symlink: Symlink::Keep,
                    ..
                } => Some(Symlink::Keep),
                _ => None,
            },
        })
        .collect())
}

fn describe(source: &Source, upstreams: &[UpstreamInfo]) -> String {
    match source {
        Source::Path(path) => format!("local {}", path.display()),
        Source::Git {
            git,
            rev,
            path,
            symlink,
        } => {
            let suffix = match symlink {
                Symlink::Follow => "",
                Symlink::Keep => " symlink: keep",
            };
            for upstream in upstreams.iter().filter(|u| &u.git == git && &u.rev == rev) {
                let inside = match &upstream.root {
                    Some(root) => path.strip_prefix(root).ok(),
                    None => Some(path.as_path()),
                };
                if let Some(inside) = inside {
                    return format!("{}:{}{suffix}", upstream.name, inside.display());
                }
            }
            format!("git {git}@{rev}:{}{suffix}", path.display())
        }
    }
}

// ---- the file ------------------------------------------------------------

/// A path as the manifest writes it: without a leading `./`.
fn clean_dest(dest: &str) -> String {
    dest.strip_prefix("./").unwrap_or(dest).to_owned()
}

fn read_manifest(root: &Path) -> Result<String> {
    let file = root.join(MANIFEST_FILE);
    match std::fs::read_to_string(&file) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            reject_misnamed(root, MANIFEST_FILE)?;
            bail!(
                "no {MANIFEST_FILE} in {}: run `catasto init`",
                root.display()
            )
        }
        Err(e) => Err(e).with_context(|| format!("cannot read {}", file.display())),
    }
}

/// Applies `change` to the manifest text, checks that the result loads, and
/// replaces the file. Returns the text it had before.
fn edit_manifest(root: &Path, change: impl FnOnce(&str) -> Result<String>) -> Result<String> {
    let before = read_manifest(root)?;
    let after = change(&before)?;
    Manifest::parse(&after).context("the edit would leave a manifest that does not load")?;
    write_atomic(&root.join(MANIFEST_FILE), &after)?;
    Ok(before)
}

fn write_atomic(file: &Path, text: &str) -> Result<()> {
    let temporary = file.with_extension("ctmt.tmp");
    std::fs::write(&temporary, text)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    std::fs::rename(&temporary, file).with_context(|| format!("cannot replace {}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
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
            }
        }

        fn add(&self, dest: &str, path: &str) -> Result<()> {
            add(&self.root, dest, &self.via(path), Some(&self.cache))
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
            },
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
            },
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
        assert!(err.to_string().contains("already declared"), "{err}");
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
}
