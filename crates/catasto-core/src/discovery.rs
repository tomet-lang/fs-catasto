//! Workspace discovery across directory trees.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::manifest::MANIFEST_FILE;

/// Directory names to skip when traversing trees for workspaces.
const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".jj",
    ".agents",
    "target",
    "node_modules",
    "result",
    "dist",
    ".cache",
    "tmp",
];

/// Returns true if a directory name matches a VCS or build artifact directory.
pub fn is_ignored_dir(name: &str) -> bool {
    IGNORED_DIRS.contains(&name)
}

/// Normalizes a path by stripping leading `./` curdir components for cleaner display.
pub fn normalize_path(path: PathBuf) -> PathBuf {
    if let Ok(stripped) = path.strip_prefix(".") {
        if stripped.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            stripped.to_path_buf()
        }
    } else {
        path
    }
}

/// Discovers every catasto workspace under `root`, `root` itself included.
///
/// A workspace is a directory that holds a `catasto.ctmt`. `root` counts when it holds one, and the search goes on
/// below it and below every workspace it finds: a manifest never hides another one. `--all` means "every
/// `catasto.ctmt` under the root", and a workspace that sits inside another is still its own (its manifest and lock
/// are read from its own directory).
///
/// The search skips VCS directories and build artifacts (`.git`, `.jj`, `.agents`, `target`, `node_modules`,
/// `result`, `dist`, `.cache`, `tmp`) and does not follow symlinks.
///
/// The result is sorted, so `root` comes first and the rest follow in a fixed order.
pub fn discover_workspaces(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        bail!("path does not exist: {}", root.display());
    }

    let mut workspaces = Vec::new();

    if root.join(MANIFEST_FILE).is_file() {
        workspaces.push(normalize_path(root.to_path_buf()));
    }

    discover_workspaces_rec(root, &mut workspaces)?;
    workspaces.sort();
    Ok(workspaces)
}

fn discover_workspaces_rec(dir: &Path, workspaces: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read directory {}", dir.display()))?;

    // Sort entries by name for consistent recursive traversal.
    let mut entries: Vec<_> = entries
        .collect::<std::result::Result<Vec<_>, std::io::Error>>()
        .with_context(|| format!("failed to read entries in {}", dir.display()))?;
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to get file type for {}", entry.path().display()))?;

        // Do not recurse into symlinks to avoid directory traversal cycles.
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }

        let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) else {
            continue;
        };

        if is_ignored_dir(&name) {
            continue;
        }

        let path = entry.path();
        if path.join(MANIFEST_FILE).is_file() {
            workspaces.push(normalize_path(path.clone()));
        }
        discover_workspaces_rec(&path, workspaces)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn root_with_a_manifest_is_listed_first_and_does_not_hide_the_workspaces_below() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        fs::write(root.join(MANIFEST_FILE), "").unwrap();
        let ws_b = root.join("ws_b");
        let ws_a = root.join("ws_a");
        fs::create_dir_all(&ws_b).unwrap();
        fs::create_dir_all(&ws_a).unwrap();
        fs::write(ws_b.join(MANIFEST_FILE), "").unwrap();
        fs::write(ws_a.join(MANIFEST_FILE), "").unwrap();

        let found = discover_workspaces(root).unwrap();
        assert_eq!(found, vec![root.to_path_buf(), ws_a, ws_b]);
    }

    #[test]
    fn root_without_a_manifest_is_not_listed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let ws = root.join("ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join(MANIFEST_FILE), "").unwrap();

        let found = discover_workspaces(root).unwrap();
        assert_eq!(found, vec![ws]);
    }

    #[test]
    fn discovers_multiple_sub_workspaces_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let ws_b = root.join("ws_b");
        let ws_a = root.join("ws_a");
        let deep_ws = root.join("group").join("deep_ws");

        fs::create_dir_all(&ws_b).unwrap();
        fs::create_dir_all(&ws_a).unwrap();
        fs::create_dir_all(&deep_ws).unwrap();

        fs::write(ws_b.join(MANIFEST_FILE), "").unwrap();
        fs::write(ws_a.join(MANIFEST_FILE), "").unwrap();
        fs::write(deep_ws.join(MANIFEST_FILE), "").unwrap();

        let found = discover_workspaces(root).unwrap();
        let expected = vec![deep_ws, ws_a, ws_b];
        assert_eq!(found, expected);
    }

    #[test]
    fn ignores_vcs_and_build_artifact_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        for ignored in IGNORED_DIRS {
            let ignored_dir = root.join(ignored).join("fake_ws");
            fs::create_dir_all(&ignored_dir).unwrap();
            fs::write(ignored_dir.join(MANIFEST_FILE), "").unwrap();
        }

        let valid_ws = root.join("valid_ws");
        fs::create_dir_all(&valid_ws).unwrap();
        fs::write(valid_ws.join(MANIFEST_FILE), "").unwrap();

        let found = discover_workspaces(root).unwrap();
        assert_eq!(found, vec![valid_ws]);
    }

    #[test]
    fn finds_a_workspace_inside_another_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let ws = root.join("my_ws");
        let internal = ws.join("packages").join("pkg_a");
        fs::create_dir_all(&internal).unwrap();

        fs::write(ws.join(MANIFEST_FILE), "").unwrap();
        fs::write(internal.join(MANIFEST_FILE), "").unwrap();

        let found = discover_workspaces(root).unwrap();
        assert_eq!(found, vec![ws, internal]);
    }

    #[test]
    fn ignored_directories_are_skipped_inside_a_workspace_too() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let ws = root.join("my_ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join(MANIFEST_FILE), "").unwrap();
        for ignored in IGNORED_DIRS {
            let fake = ws.join(ignored).join("fake_ws");
            fs::create_dir_all(&fake).unwrap();
            fs::write(fake.join(MANIFEST_FILE), "").unwrap();
        }

        let found = discover_workspaces(root).unwrap();
        assert_eq!(found, vec![ws]);
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join(MANIFEST_FILE), "").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();

        let found = discover_workspaces(root).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn empty_root_returns_empty_vec() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let found = discover_workspaces(root).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn nonexistent_root_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let non_existent = tmp.path().join("does_not_exist");

        assert!(discover_workspaces(&non_existent).is_err());
    }

    #[test]
    fn normalizes_paths_with_dot_prefix() {
        assert_eq!(normalize_path(PathBuf::from(".")), PathBuf::from("."));
        assert_eq!(
            normalize_path(PathBuf::from("./repo-a")),
            PathBuf::from("repo-a")
        );
        assert_eq!(
            normalize_path(PathBuf::from("./a/b/c")),
            PathBuf::from("a/b/c")
        );
        assert_eq!(
            normalize_path(PathBuf::from("repo-a")),
            PathBuf::from("repo-a")
        );
    }
}
