//! The commands that change what a workspace declares: `init`, `add`,
//! `remove`, `upstream`, and `list` to read it back.
//!
//! Each edit goes through [`crate::edit`], which keeps the file as the user
//! wrote it, and is parsed again before it is written; a manifest that would
//! not load is never saved. The file is replaced by renaming a temporary
//! beside it, so a crash leaves the old one or the new one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::check::{ViolationKind, check};
pub use crate::edit::AddOutcome;
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

/// Declares `dest` with `source`, fetches it and locks it. If `force`, replaces
/// the source of an existing declaration. The other files and what the lock says
/// about them are left alone. If the fetch fails, so does the declaration: the
/// manifest is put back.
pub fn add(
    root: &Path,
    dest: &str,
    source: &SourceText,
    force: bool,
    cache_dir: Option<&Path>,
) -> Result<AddOutcome> {
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
    let mut outcome = AddOutcome::Added;
    let before = edit_manifest(root, |text| {
        let (new_text, out) = edit::add_or_replace_entry(text, &dest, source, force)?;
        outcome = out;
        Ok(new_text)
    })?;
    let result = sync_only(root, SyncMode::Locked, cache_dir, &[PathBuf::from(&dest)]);
    if let Err(error) = result {
        write_atomic(&root.join(MANIFEST_FILE), &before)?;
        return Err(error.context(format!("could not add {dest}")));
    }
    Ok(outcome)
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

/// Names a place in `::upstreams`, or replaces its definition if `force`.
pub fn upstream_add(
    root: &Path,
    name: &str,
    upstream: &UpstreamText,
    force: bool,
) -> Result<(AddOutcome, Vec<String>)> {
    let mut outcome = AddOutcome::Added;
    let mut used = Vec::new();
    edit_manifest(root, |text| {
        let (new_text, out) = edit::add_or_replace_upstream(text, name, upstream, force)?;
        if out == AddOutcome::Replaced {
            used = edit::upstream_is_used(&new_text, name)?;
        }
        outcome = out;
        Ok(new_text)
    })?;
    Ok((outcome, used))
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
    let manifest_file = root.join(MANIFEST_FILE);
    let (manifest, upstreams) = match std::fs::read_to_string(&manifest_file) {
        Ok(text) => (
            Manifest::parse(&text)
                .with_context(|| format!("invalid {}", manifest_file.display()))?,
            upstreams_of(&text)?,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            reject_misnamed(root, MANIFEST_FILE)?;
            (Manifest::default(), Vec::new())
        }
        Err(e) => {
            return Err(e).with_context(|| format!("cannot read {}", manifest_file.display()));
        }
    };

    let lock = Lock::load(root)?;
    let report = check(root)?;

    let mut violations_by_path = HashMap::new();
    for violation in &report.violations {
        violations_by_path
            .entry(&violation.path)
            .or_insert(violation.kind);
    }

    Ok(manifest
        .managed
        .iter()
        .map(|managed| Row {
            path: managed.path.clone(),
            from: describe(&managed.source, &upstreams),
            state: violations_by_path.get(&managed.path).copied(),
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
mod tests;
