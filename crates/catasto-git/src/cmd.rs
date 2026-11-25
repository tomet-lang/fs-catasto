use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};

/// Kind of a git-visible entry in a working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VisibleEntryKind {
    /// A regular file (`100644` or `100755`).
    File,
    /// A symbolic link (`120000`).
    Symlink,
    /// A git submodule or nested git repository (`160000` or trailing `/` in `--others`).
    Submodule,
}

/// One git-visible entry in a working tree, relative to the queried directory.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VisibleEntry {
    /// Path relative to the queried directory, separated by `/`.
    pub path: String,
    /// Kind of entry.
    pub kind: VisibleEntryKind,
}

pub(crate) fn run(git_dir: Option<&Path>, args: &[&str]) -> Result<Output> {
    let mut command = Command::new("git");
    if let Some(dir) = git_dir {
        // Run inside the repository: git otherwise hints at files of the
        // caller's working directory when a path is missing.
        command.current_dir(dir).args(["--git-dir", "."]);
    }
    let output = command
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        // Belt and braces next to `classify`: no other transport, whatever a
        // redirect or a config file asks for.
        .env("GIT_ALLOW_PROTOCOL", "https:file")
        .stdin(Stdio::null())
        .output()
        .context("cannot run git (is it installed?)")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(output)
}

pub(crate) fn init_bare(repo: &Path) -> Result<()> {
    if repo.join("HEAD").exists() {
        return Ok(());
    }
    std::fs::create_dir_all(repo).with_context(|| format!("cannot create {}", repo.display()))?;
    let repo_str = repo.to_str().context("cache path is not valid UTF-8")?;
    run(None, &["init", "--bare", "--quiet", repo_str])?;
    Ok(())
}

pub(crate) fn fetch_remote(repo: &Path, url: &str) -> Result<()> {
    run(
        Some(repo),
        &[
            "fetch",
            "--quiet",
            "--force",
            "--tags",
            "--",
            url,
            "+refs/heads/*:refs/heads/*",
        ],
    )
    .with_context(|| format!("cannot fetch {url}"))?;
    Ok(())
}

pub(crate) fn resolve(repo: &Path, url: &str, rev: &str) -> Result<String> {
    if rev.is_empty() || rev.starts_with('-') {
        bail!("invalid rev: {rev:?}");
    }
    let output = run(
        Some(repo),
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
    .with_context(|| format!("revision {rev:?} not found in {url}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(crate) fn has_commit(repo: &Path, commit: &str) -> bool {
    run(
        Some(repo),
        &["cat-file", "-e", &format!("{commit}^{{commit}}")],
    )
    .is_ok()
}

/// Commit ids come from a hand-editable lockfile and end up as `git` arguments.
pub(crate) fn check_commit_id(commit: &str) -> Result<()> {
    let hex = commit.bytes().all(|b| b.is_ascii_hexdigit());
    if !hex || !matches!(commit.len(), 40 | 64) {
        bail!("not a full commit id: {commit:?}");
    }
    Ok(())
}

pub(crate) fn repo_path(path: &Path) -> Result<String> {
    let parts = path
        .components()
        .map(|c| {
            c.as_os_str()
                .to_str()
                .with_context(|| format!("path is not valid UTF-8: {}", path.display()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(parts.join("/"))
}

fn run_worktree_output(dir: &Path, args: &[&str]) -> Result<Output> {
    let mut command = Command::new("git");
    command.current_dir(dir);
    command
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_ALLOW_PROTOCOL", "https:file")
        .stdin(Stdio::null())
        .output()
        .context("cannot run git (is it installed?)")
}

fn run_worktree(dir: &Path, args: &[&str]) -> Result<Output> {
    let output = run_worktree_output(dir, args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(output)
}

fn worktree_has_path(dest: &Path, commit: &str, path_in_repo: &str) -> Result<bool> {
    let output = run_worktree(dest, &["ls-tree", "-z", commit, "--", path_in_repo])?;
    let entries = crate::tree::parse_ls_tree_output(&output.stdout)?;
    Ok(entries.into_iter().any(|e| e.path == path_in_repo))
}

/// Returns the full HEAD commit id of the repository at `dest`.
pub fn head_commit(dest: &Path) -> Result<String> {
    let output = run_worktree(dest, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Returns the `origin` remote URL of the repository at `dest`.
pub fn remote_url(dest: &Path) -> Result<String> {
    let output = run_worktree(dest, &["remote", "get-url", "origin"])?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Returns the currently checked-out branch name at `dest`, or `None` if detached.
pub fn current_branch(dest: &Path) -> Result<Option<String>> {
    let output = run_worktree(dest, &["branch", "--show-current"])?;
    let b = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if b.is_empty() { Ok(None) } else { Ok(Some(b)) }
}

/// Fetches `origin` (including tags) in the repository worktree at `dest`.
pub fn fetch_worktree(dest: &Path) -> Result<()> {
    run_worktree(dest, &["fetch", "--quiet", "--force", "--tags", "origin"])?;
    Ok(())
}

/// Checks out a specific commit id in the repository worktree at `dest`.
pub fn checkout_commit(dest: &Path, commit: &str) -> Result<()> {
    check_commit_id(commit)?;
    run_worktree(dest, &["checkout", "--quiet", commit])?;
    Ok(())
}

/// Returns whether sparse-checkout is enabled in the worktree at `dest`.
pub fn is_sparse_checkout(dest: &Path) -> Result<bool> {
    let output = run_worktree_output(dest, &["config", "--bool", "core.sparseCheckout"])?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim() == "true");
    }
    if output.status.code() == Some(1) && output.stderr.is_empty() {
        return Ok(false);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "git config --bool core.sparseCheckout failed: {}",
        stderr.trim()
    );
}

/// Lists the configured sparse-checkout patterns in `dest`, or an empty list if disabled.
pub fn sparse_checkout_list(dest: &Path) -> Result<Vec<String>> {
    if !is_sparse_checkout(dest)? {
        return Ok(Vec::new());
    }
    let output = run_worktree(dest, &["sparse-checkout", "list"])?;
    let list = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(list)
}

/// Configures sparse-checkout in `dest` to only include `subpath`, or disables it when `None`.
pub fn apply_sparse_checkout(dest: &Path, subpath: Option<&Path>) -> Result<()> {
    match subpath {
        Some(p) => {
            let sp = repo_path(p)?;
            if sp.is_empty() {
                bail!("sparse-checkout subpath cannot be empty");
            }
            let pattern = format!("/{sp}");
            run_worktree(dest, &["sparse-checkout", "set", "--no-cone", &pattern])?;
        }
        None => {
            if is_sparse_checkout(dest)? {
                run_worktree(dest, &["sparse-checkout", "disable"])?;
            }
        }
    }
    Ok(())
}

/// Verifies that the sparse-checkout configuration in `dest` matches `expected_subpath`.
pub fn check_sparse_checkout(dest: &Path, expected_subpath: Option<&Path>) -> Result<bool> {
    match expected_subpath {
        None => Ok(!is_sparse_checkout(dest)?),
        Some(p) => {
            let sp = repo_path(p)?;
            if !is_sparse_checkout(dest)? {
                return Ok(false);
            }
            let list = sparse_checkout_list(dest)?;
            let expected_slash = format!("/{sp}");
            let matches_pattern = list.len() == 1 && (list[0] == expected_slash || list[0] == sp);
            Ok(matches_pattern && dest.join(p).exists())
        }
    }
}

/// Checks out `rev` (a branch, tag, or commit hash) in the worktree at `dest` and returns HEAD.
pub fn checkout_rev(dest: &Path, url: &str, rev: &str) -> Result<String> {
    if rev.is_empty() || rev.starts_with('-') {
        bail!("invalid rev: {rev:?}");
    }
    let remote_ref = format!("refs/remotes/origin/{rev}");
    let show_ref = run_worktree_output(dest, &["show-ref", "--verify", "--quiet", &remote_ref])?;
    if show_ref.status.success() {
        run_worktree(dest, &["checkout", "--quiet", "-B", rev, &remote_ref])?;
    } else if show_ref.status.code() == Some(1) {
        let rev_spec = format!("{rev}^{{commit}}");
        let parsed = run_worktree_output(dest, &["rev-parse", "--verify", "--quiet", &rev_spec])?;
        if !parsed.status.success() {
            bail!("cannot resolve rev `{rev}` in {url}");
        }
        let commit = String::from_utf8_lossy(&parsed.stdout).trim().to_owned();
        run_worktree(dest, &["checkout", "--quiet", &commit])?;
    } else {
        let stderr = String::from_utf8_lossy(&show_ref.stderr);
        bail!("git show-ref failed: {}", stderr.trim());
    }
    head_commit(dest)
}

/// Clones `url` into `dest` as a submodule worktree at `rev` (or `pinned` commit) with optional
/// sparse-checkout `subpath`, returning the checked-out HEAD commit id.
pub fn clone_submodule(
    url: &str,
    dest: &Path,
    rev: &str,
    pinned: Option<&str>,
    subpath: Option<&Path>,
) -> Result<String> {
    if let Some(commit) = pinned {
        check_commit_id(commit)?;
    } else if rev.is_empty() || rev.starts_with('-') {
        bail!("invalid rev: {rev:?}");
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create directory {}", parent.display()))?;
    }
    let dest_str = dest.to_str().context("dest path is not valid UTF-8")?;
    run(
        None,
        &["clone", "--quiet", "--no-checkout", "--", url, dest_str],
    )?;

    let setup = || -> Result<String> {
        if subpath.is_some() {
            apply_sparse_checkout(dest, subpath)?;
        }
        if let Some(commit) = pinned {
            checkout_commit(dest, commit)?;
        } else {
            checkout_rev(dest, url, rev)?;
        }
        let head = head_commit(dest)?;
        if let Some(p) = subpath {
            let sp = repo_path(p)?;
            if !worktree_has_path(dest, &head, &sp)? {
                bail!("{} is not in {url} at {head}", p.display());
            }
        }
        Ok(head)
    };

    match setup() {
        Ok(head) => Ok(head),
        Err(err) => {
            let _ = std::fs::remove_dir_all(dest);
            Err(err)
        }
    }
}

/// Fetches `origin` in `dest`, updates sparse-checkout to `subpath`, checks out `rev`, and
/// returns the new HEAD commit id.
pub fn update_submodule(
    dest: &Path,
    url: &str,
    rev: &str,
    subpath: Option<&Path>,
) -> Result<String> {
    fetch_worktree(dest)?;
    apply_sparse_checkout(dest, subpath)?;
    let head = checkout_rev(dest, url, rev)?;
    if let Some(p) = subpath {
        let sp = repo_path(p)?;
        if !worktree_has_path(dest, &head, &sp)? {
            bail!("{} is not in {url} at {head}", p.display());
        }
    }
    Ok(head)
}

/// Lists all git-visible entries under `dir` (tracked entries still present in the working tree,
/// plus untracked and non-ignored entries), sorted by path.
pub fn visible_entries(dir: &Path) -> Result<Vec<VisibleEntry>> {
    let deleted_output = run_worktree(dir, &["ls-files", "-z", "--deleted"])?;
    let mut deleted = HashSet::new();
    for item in deleted_output.stdout.split(|&b| b == 0) {
        if item.is_empty() {
            continue;
        }
        let path = std::str::from_utf8(item).context("git ls-files output is not valid UTF-8")?;
        deleted.insert(path);
    }

    let stage_output = run_worktree(dir, &["ls-files", "-z", "--stage"])?;
    let mut entries = BTreeMap::new();
    for item in stage_output.stdout.split(|&b| b == 0) {
        if item.is_empty() {
            continue;
        }
        let item_str =
            std::str::from_utf8(item).context("git ls-files output is not valid UTF-8")?;
        let (meta, path) = item_str
            .split_once('\t')
            .context("invalid git ls-files --stage output: missing tab")?;
        if deleted.contains(path) {
            continue;
        }
        let mode = meta
            .split_whitespace()
            .next()
            .context("missing mode in git ls-files --stage")?;
        let kind = match mode {
            "160000" => VisibleEntryKind::Submodule,
            "120000" => VisibleEntryKind::Symlink,
            _ => VisibleEntryKind::File,
        };
        entries.insert(path.to_owned(), kind);
    }

    let others_output =
        run_worktree(dir, &["ls-files", "-z", "--others", "--exclude-standard"])?;
    for item in others_output.stdout.split(|&b| b == 0) {
        if item.is_empty() {
            continue;
        }
        let raw_path =
            std::str::from_utf8(item).context("git ls-files output is not valid UTF-8")?;
        if let Some(sub_path) = raw_path.strip_suffix('/') {
            if !sub_path.is_empty() {
                entries.insert(sub_path.to_owned(), VisibleEntryKind::Submodule);
            }
        } else {
            let is_symlink = std::fs::symlink_metadata(dir.join(raw_path))
                .is_ok_and(|meta| meta.is_symlink());
            let kind = if is_symlink {
                VisibleEntryKind::Symlink
            } else {
                VisibleEntryKind::File
            };
            entries.insert(raw_path.to_owned(), kind);
        }
    }

    Ok(entries
        .into_iter()
        .map(|(path, kind)| VisibleEntry { path, kind })
        .collect())
}
