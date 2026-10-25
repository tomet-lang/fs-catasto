use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result, bail};

use crate::cmd::{repo_path, run};
use crate::{Content, SymlinkMode};

/// `(repository, commit id, path)` of one `ls-tree` lookup.
type CacheKey = (PathBuf, String, String);

static LS_TREE_CACHE: LazyLock<Mutex<HashMap<CacheKey, Option<TreeEntry>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
pub(crate) struct TreeEntry {
    pub(crate) mode: String,
    pub(crate) kind: String,
    pub(crate) oid: String,
    pub(crate) path: String,
}

pub(crate) fn parse_ls_tree_output(stdout: &[u8]) -> Result<Vec<TreeEntry>> {
    let mut entries = Vec::new();
    for item in stdout.split(|&b| b == 0) {
        if item.is_empty() {
            continue;
        }
        let item_str =
            std::str::from_utf8(item).context("git ls-tree output is not valid UTF-8")?;
        let (meta, path) = item_str
            .split_once('\t')
            .context("invalid git ls-tree output: missing tab")?;
        let mut meta_parts = meta.split_whitespace();
        let mode = meta_parts
            .next()
            .context("missing mode in ls-tree")?
            .to_owned();
        let kind = meta_parts
            .next()
            .context("missing kind in ls-tree")?
            .to_owned();
        let oid = meta_parts
            .next()
            .context("missing oid in ls-tree")?
            .to_owned();
        entries.push(TreeEntry {
            mode,
            kind,
            oid,
            path: path.to_owned(),
        });
    }
    Ok(entries)
}

/// Looks up one path in a commit's tree, memoised per process.
///
/// `commit` is always a full commit id (see `GitCache::read`), which names an
/// immutable tree, so a cached entry can never go stale. The lock is not held
/// while git runs.
pub(crate) fn ls_tree_one(
    repo: &Path,
    commit: &str,
    path_in_repo: &str,
) -> Result<Option<TreeEntry>> {
    let key = (
        repo.to_path_buf(),
        commit.to_owned(),
        path_in_repo.to_owned(),
    );
    if let Some(cached) = LS_TREE_CACHE.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return Ok(cached);
    }
    let output = run(Some(repo), &["ls-tree", "-z", commit, "--", path_in_repo])?;
    let entries = parse_ls_tree_output(&output.stdout)?;
    let entry = entries.into_iter().find(|e| e.path == path_in_repo);
    if let Ok(mut cache) = LS_TREE_CACHE.lock() {
        cache.insert(key, entry.clone());
    }
    Ok(entry)
}

pub(crate) fn check_parent_components_not_symlinks(
    repo: &Path,
    commit: &str,
    manifest_path: &Path,
    path_in_repo: &str,
) -> Result<()> {
    let parts: Vec<&str> = path_in_repo.split('/').filter(|s| !s.is_empty()).collect();
    let mut current = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            break;
        }
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(part);

        if let Some(entry) = ls_tree_one(repo, commit, &current)? {
            if entry.mode == "120000" {
                bail!(
                    "directory component `{current}` of {} is a symlink",
                    manifest_path.display()
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn resolve_symlink_target(
    link_in_repo: &str,
    target: &str,
    manifest_path: &Path,
    chain: &[String],
) -> Result<String> {
    let chain_info = format!("link chain: {}", chain.join(" -> "));
    if target.is_empty() {
        bail!(
            "cannot fetch {}: symlink target is empty ({chain_info})",
            manifest_path.display()
        );
    }
    if target.contains('\0') {
        bail!(
            "cannot fetch {}: symlink target contains NUL byte ({chain_info})",
            manifest_path.display()
        );
    }
    if target.starts_with('/') || target.starts_with('\\') {
        bail!(
            "cannot fetch {}: symlink target is absolute: {target} ({chain_info})",
            manifest_path.display()
        );
    }

    let parent = match link_in_repo.rfind('/') {
        Some(idx) => &link_in_repo[..idx],
        None => "",
    };

    let mut parts: Vec<&str> = if parent.is_empty() {
        Vec::new()
    } else {
        parent.split('/').filter(|s| !s.is_empty()).collect()
    };

    for comp in target.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if parts.is_empty() {
                    bail!(
                        "cannot fetch {}: symlink target climbs out of the repository: {target} ({chain_info})",
                        manifest_path.display()
                    );
                }
                parts.pop();
            }
            c => parts.push(c),
        }
    }

    let resolved = parts.join("/");
    if resolved.is_empty() {
        bail!(
            "cannot fetch {}: symlink target points to the repository root ({chain_info})",
            manifest_path.display()
        );
    }
    Ok(resolved)
}

pub(crate) fn cat_blob_bytes(repo: &Path, oid: &str) -> Result<Vec<u8>> {
    let output = run(Some(repo), &["cat-file", "blob", oid])?;
    Ok(output.stdout)
}

pub(crate) fn cat_symlink_text(
    repo: &Path,
    oid: &str,
    manifest_path: &Path,
    chain: &[String],
) -> Result<String> {
    let bytes = cat_blob_bytes(repo, oid)?;
    String::from_utf8(bytes).map_err(|_| {
        let chain_info = format!("link chain: {}", chain.join(" -> "));
        anyhow::anyhow!(
            "cannot fetch {}: symlink target is not valid UTF-8 ({chain_info})",
            manifest_path.display()
        )
    })
}

/// Reads the file or symlink at `path` within `repo` at `commit`.
pub(crate) fn read_path(
    repo: &Path,
    commit: &str,
    path: &Path,
    url: &str,
    symlink: SymlinkMode,
) -> Result<Content> {
    let initial_repo_path = repo_path(path)?;
    check_parent_components_not_symlinks(repo, commit, path, &initial_repo_path)?;

    let initial_entry = ls_tree_one(repo, commit, &initial_repo_path)?
        .with_context(|| format!("{} is not in {url} at {commit}", path.display()))?;

    match symlink {
        SymlinkMode::Keep => {
            if initial_entry.mode != "120000" {
                bail!(
                    "`symlink: keep`, but {} is not a symlink in {url} at {commit}",
                    path.display()
                );
            }
            let chain = vec![initial_repo_path.clone()];
            let text = cat_symlink_text(repo, &initial_entry.oid, path, &chain)?;
            let _ = resolve_symlink_target(&initial_repo_path, &text, path, &chain)?;
            Ok(Content::Link(text))
        }
        SymlinkMode::Follow => {
            if initial_entry.mode == "100644" || initial_entry.mode == "100755" {
                let bytes = cat_blob_bytes(repo, &initial_entry.oid)?;
                return Ok(Content::File(bytes));
            }
            if initial_entry.mode != "120000" {
                let kind = if initial_entry.mode == "040000" || initial_entry.kind == "tree" {
                    "a directory"
                } else if initial_entry.mode == "160000" || initial_entry.kind == "commit" {
                    "a git submodule"
                } else {
                    "an unsupported git object"
                };
                bail!(
                    "cannot fetch {}: {initial_repo_path} is {kind}, not a file",
                    path.display()
                );
            }

            let mut current_path = initial_repo_path.clone();
            let mut current_oid = initial_entry.oid;
            let mut chain = vec![initial_repo_path];
            let mut hops = 0;

            loop {
                hops += 1;
                if hops > 8 {
                    bail!(
                        "cannot fetch {}: too many levels of symbolic links (link chain: {})",
                        path.display(),
                        chain.join(" -> ")
                    );
                }

                let text = cat_symlink_text(repo, &current_oid, path, &chain)?;
                let next_path = resolve_symlink_target(&current_path, &text, path, &chain)?;

                if chain.contains(&next_path) {
                    chain.push(next_path);
                    bail!(
                        "cannot fetch {}: symlink loop detected (link chain: {})",
                        path.display(),
                        chain.join(" -> ")
                    );
                }

                check_parent_components_not_symlinks(repo, commit, path, &next_path)?;

                let next_entry = match ls_tree_one(repo, commit, &next_path)? {
                    Some(e) => e,
                    None => {
                        chain.push(next_path.clone());
                        bail!(
                            "cannot fetch {}: symlink target `{next_path}` does not exist (link chain: {})",
                            path.display(),
                            chain.join(" -> ")
                        );
                    }
                };

                chain.push(next_path.clone());

                if next_entry.mode == "100644" || next_entry.mode == "100755" {
                    let bytes = cat_blob_bytes(repo, &next_entry.oid)?;
                    return Ok(Content::File(bytes));
                } else if next_entry.mode == "120000" {
                    current_path = next_path;
                    current_oid = next_entry.oid;
                    continue;
                } else {
                    let kind = if next_entry.mode == "040000" || next_entry.kind == "tree" {
                        "a directory"
                    } else if next_entry.mode == "160000" || next_entry.kind == "commit" {
                        "a git submodule"
                    } else {
                        "an unsupported git object"
                    };
                    bail!(
                        "cannot fetch {}: symlink target `{next_path}` is {kind} (link chain: {})",
                        path.display(),
                        chain.join(" -> ")
                    );
                }
            }
        }
    }
}
