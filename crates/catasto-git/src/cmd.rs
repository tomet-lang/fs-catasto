use std::path::Path;
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};

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

fn run_worktree(dir: &Path, args: &[&str]) -> Result<Output> {
    let mut command = Command::new("git");
    command.current_dir(dir);
    let output = command
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
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

/// Clones `url` into `dest` (optionally checking out `branch`) and returns the HEAD commit id.
pub fn clone_repo(url: &str, dest: &Path, branch: Option<&str>) -> Result<String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create directory {}", parent.display()))?;
    }
    let dest_str = dest.to_str().context("dest path is not valid UTF-8")?;
    let mut args = vec!["clone", "--quiet"];
    if let Some(b) = branch {
        if b.is_empty() || b.starts_with('-') {
            bail!("invalid branch name: {b:?}");
        }
        args.extend(["--branch", b]);
    }
    args.extend(["--", url, dest_str]);
    run(None, &args)?;
    head_commit(dest)
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

/// Fast-forwards the checked-out branch in `dest` to `origin/<branch>`.
pub fn fast_forward_branch(dest: &Path, branch: &str) -> Result<String> {
    if branch.is_empty() || branch.starts_with('-') {
        bail!("invalid branch name: {branch:?}");
    }
    let remote_ref = format!("origin/{branch}");
    run_worktree(dest, &["merge", "--ff-only", "--quiet", &remote_ref])?;
    head_commit(dest)
}
