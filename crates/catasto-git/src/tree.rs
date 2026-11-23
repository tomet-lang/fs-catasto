use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result, bail};

use crate::cmd::{repo_path, run};
use crate::{Content, DirEntry, DirEntryContent, SymlinkMode};

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

fn is_file(entry: &TreeEntry) -> bool {
    entry.mode == "100644" || entry.mode == "100755"
}

fn is_tree(entry: &TreeEntry) -> bool {
    entry.mode == "040000" || entry.kind == "tree"
}

fn is_submodule(entry: &TreeEntry) -> bool {
    entry.mode == "160000" || entry.kind == "commit"
}

enum ResolvedTarget {
    File { bytes: Vec<u8>, executable: bool },
    Tree { repo_path: String },
}

fn follow_symlink(
    repo: &Path,
    commit: &str,
    display_path: &Path,
    initial_path: &str,
    initial_oid: &str,
) -> Result<ResolvedTarget> {
    let mut current_path = initial_path.to_owned();
    let mut current_oid = initial_oid.to_owned();
    let mut chain = vec![initial_path.to_owned()];
    let mut hops = 0;

    loop {
        hops += 1;
        if hops > 8 {
            bail!(
                "cannot fetch {}: too many levels of symbolic links (link chain: {})",
                display_path.display(),
                chain.join(" -> ")
            );
        }

        let text = cat_symlink_text(repo, &current_oid, display_path, &chain)?;
        let next_path = resolve_symlink_target(&current_path, &text, display_path, &chain)?;

        if chain.contains(&next_path) {
            chain.push(next_path);
            bail!(
                "cannot fetch {}: symlink loop detected (link chain: {})",
                display_path.display(),
                chain.join(" -> ")
            );
        }

        check_parent_components_not_symlinks(repo, commit, display_path, &next_path)?;

        let next_entry = match ls_tree_one(repo, commit, &next_path)? {
            Some(e) => e,
            None => {
                chain.push(next_path.clone());
                bail!(
                    "cannot fetch {}: symlink target `{next_path}` does not exist (link chain: {})",
                    display_path.display(),
                    chain.join(" -> ")
                );
            }
        };

        chain.push(next_path.clone());

        if is_file(&next_entry) {
            let bytes = cat_blob_bytes(repo, &next_entry.oid)?;
            let executable = next_entry.mode == "100755";
            return Ok(ResolvedTarget::File { bytes, executable });
        } else if is_tree(&next_entry) {
            return Ok(ResolvedTarget::Tree {
                repo_path: next_path,
            });
        } else if next_entry.mode == "120000" {
            current_path = next_path;
            current_oid = next_entry.oid;
        } else {
            let kind = if is_submodule(&next_entry) {
                "a git submodule"
            } else {
                "an unsupported git object"
            };
            bail!(
                "cannot fetch {}: symlink target `{next_path}` is {kind} (link chain: {})",
                display_path.display(),
                chain.join(" -> ")
            );
        }
    }
}

struct TreeReadContext<'a> {
    repo: &'a Path,
    commit: &'a str,
    display_path: &'a Path,
    symlink: SymlinkMode,
}

fn collect_tree_entries(
    ctx: &TreeReadContext<'_>,
    tree_repo_path: &str,
    rel_prefix: &str,
    active_trees: &mut Vec<String>,
    entries: &mut Vec<DirEntry>,
) -> Result<()> {
    let output = if tree_repo_path.is_empty() {
        run(Some(ctx.repo), &["ls-tree", "-r", "-z", ctx.commit])?
    } else {
        run(
            Some(ctx.repo),
            &["ls-tree", "-r", "-z", ctx.commit, "--", tree_repo_path],
        )?
    };
    let tree_entries = parse_ls_tree_output(&output.stdout)?;
    let prefix = if tree_repo_path.is_empty() {
        String::new()
    } else {
        format!("{tree_repo_path}/")
    };

    for entry in tree_entries {
        let rel_path_str = if prefix.is_empty() {
            entry.path.as_str()
        } else {
            match entry.path.strip_prefix(&prefix) {
                Some(rest) => rest,
                None => continue,
            }
        };
        let full_rel_str = if rel_prefix.is_empty() {
            rel_path_str.to_owned()
        } else {
            format!("{rel_prefix}/{rel_path_str}")
        };

        if is_file(&entry) {
            let bytes = cat_blob_bytes(ctx.repo, &entry.oid)?;
            let executable = entry.mode == "100755";
            entries.push(DirEntry {
                rel_path: PathBuf::from(&full_rel_str),
                content: DirEntryContent::File { bytes, executable },
            });
        } else if entry.mode == "120000" {
            match ctx.symlink {
                SymlinkMode::Keep => {
                    let chain = vec![entry.path.clone()];
                    let text = cat_symlink_text(ctx.repo, &entry.oid, ctx.display_path, &chain)?;
                    let _ = resolve_symlink_target(&entry.path, &text, ctx.display_path, &chain)?;
                    entries.push(DirEntry {
                        rel_path: PathBuf::from(&full_rel_str),
                        content: DirEntryContent::Link(text),
                    });
                }
                SymlinkMode::Follow => {
                    match follow_symlink(
                        ctx.repo,
                        ctx.commit,
                        ctx.display_path,
                        &entry.path,
                        &entry.oid,
                    )? {
                        ResolvedTarget::File { bytes, executable } => {
                            entries.push(DirEntry {
                                rel_path: PathBuf::from(&full_rel_str),
                                content: DirEntryContent::File { bytes, executable },
                            });
                        }
                        ResolvedTarget::Tree { repo_path } => {
                            if active_trees.contains(&repo_path) {
                                bail!(
                                    "cannot fetch {}: symlink loop detected (directory `{repo_path}`)",
                                    ctx.display_path.display()
                                );
                            }
                            active_trees.push(repo_path.clone());
                            let res = collect_tree_entries(
                                ctx,
                                &repo_path,
                                &full_rel_str,
                                active_trees,
                                entries,
                            );
                            active_trees.pop();
                            res?;
                        }
                    }
                }
            }
        } else if is_submodule(&entry) {
            bail!(
                "cannot fetch {}: {} is a git submodule",
                ctx.display_path.display(),
                entry.path
            );
        } else {
            bail!(
                "cannot fetch {}: {} is an unsupported git object",
                ctx.display_path.display(),
                entry.path
            );
        }
    }

    Ok(())
}

fn read_dir_tree(
    repo: &Path,
    commit: &str,
    tree_repo_path: &str,
    display_path: &Path,
    symlink: SymlinkMode,
) -> Result<Vec<DirEntry>> {
    let ctx = TreeReadContext {
        repo,
        commit,
        display_path,
        symlink,
    };
    let mut entries = Vec::new();
    let mut active_trees = vec![tree_repo_path.to_owned()];
    collect_tree_entries(&ctx, tree_repo_path, "", &mut active_trees, &mut entries)?;
    entries.sort_by(|a, b| {
        let a_key = a
            .rel_path
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        let b_key = b
            .rel_path
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        a_key.as_bytes().cmp(b_key.as_bytes())
    });
    Ok(entries)
}

/// Reads the file, symlink, or directory at `path` (or the root tree when `None`) within `repo` at `commit`.
pub(crate) fn read_path(
    repo: &Path,
    commit: &str,
    path: Option<&Path>,
    url: &str,
    symlink: SymlinkMode,
) -> Result<(Content, bool)> {
    let Some(p) = path else {
        if symlink == SymlinkMode::Keep {
            bail!("`symlink: keep` requires `path`");
        }
        let entries = read_dir_tree(repo, commit, "", Path::new("."), symlink)?;
        return Ok((Content::Dir(entries), false));
    };

    let initial_repo_path = repo_path(p)?;
    check_parent_components_not_symlinks(repo, commit, p, &initial_repo_path)?;

    let initial_entry = ls_tree_one(repo, commit, &initial_repo_path)?
        .with_context(|| format!("{} is not in {url} at {commit}", p.display()))?;

    if is_tree(&initial_entry) {
        let entries = read_dir_tree(repo, commit, &initial_repo_path, p, symlink)?;
        return Ok((Content::Dir(entries), false));
    }

    match symlink {
        SymlinkMode::Keep => {
            if initial_entry.mode != "120000" {
                bail!(
                    "`symlink: keep`, but {} is not a symlink in {url} at {commit}",
                    p.display()
                );
            }
            let chain = vec![initial_repo_path.clone()];
            let text = cat_symlink_text(repo, &initial_entry.oid, p, &chain)?;
            let _ = resolve_symlink_target(&initial_repo_path, &text, p, &chain)?;
            Ok((Content::Link(text), false))
        }
        SymlinkMode::Follow => {
            if is_file(&initial_entry) {
                let bytes = cat_blob_bytes(repo, &initial_entry.oid)?;
                let executable = initial_entry.mode == "100755";
                return Ok((Content::File(bytes), executable));
            }
            if initial_entry.mode != "120000" {
                let kind = if is_submodule(&initial_entry) {
                    "a git submodule"
                } else {
                    "an unsupported git object"
                };
                bail!(
                    "cannot fetch {}: {initial_repo_path} is {kind}, not a file",
                    p.display()
                );
            }

            match follow_symlink(repo, commit, p, &initial_repo_path, &initial_entry.oid)? {
                ResolvedTarget::File { bytes, executable } => {
                    Ok((Content::File(bytes), executable))
                }
                ResolvedTarget::Tree { repo_path } => {
                    let entries = read_dir_tree(repo, commit, &repo_path, p, symlink)?;
                    Ok((Content::Dir(entries), false))
                }
            }
        }
    }
}
