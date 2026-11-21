//! `.gitmodules` generation and verification for repository entries.

use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use catasto_manifest::{Lock, Manifest};

pub const GITMODULES_FILE: &str = ".gitmodules";

/// What kind of drift `.gitmodules` has compared to `catasto.ctmt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitmodulesDrift {
    Missing,
    Modified,
    Orphaned,
}

/// Renders `.gitmodules` text from the manifest, or `None` if no entry has `submodule: true`.
pub fn render_gitmodules(manifest: &Manifest) -> Result<Option<String>> {
    let mut submodule_repos: Vec<_> = manifest.repos().filter(|(_, r)| r.submodule).collect();
    if submodule_repos.is_empty() {
        return Ok(None);
    }

    submodule_repos.sort_by_key(|(path, _)| *path);

    let mut out = String::new();
    for (path, repo) in submodule_repos {
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow!("path is not valid UTF-8: {}", path.display()))?;
        out.push_str(&format!("[submodule \"{path_str}\"]\n"));
        out.push_str(&format!("\tpath = {path_str}\n"));
        out.push_str(&format!("\turl = {}\n", repo.git));
        if let Some(ref branch) = repo.branch {
            out.push_str(&format!("\tbranch = {branch}\n"));
        }
        out.push_str("\tignore = dirty\n");
    }
    Ok(Some(out))
}

/// Synchronizes `.gitmodules` on disk with `manifest`.
///
/// Writes the file if `submodule: true` entries exist; removes it if previously tracked submodules are gone.
/// Leaves an unrelated `.gitmodules` untouched when neither `manifest` nor `lock` declares any submodule.
pub fn sync_gitmodules(root: &Path, manifest: &Manifest, lock: &Lock) -> Result<()> {
    let has_manifest_submodules = manifest.repos().any(|(_, r)| r.submodule);
    let had_lock_submodules = lock.repos.iter().any(|r| r.submodule);

    if !has_manifest_submodules && !had_lock_submodules {
        return Ok(());
    }

    let file = root.join(GITMODULES_FILE);
    match render_gitmodules(manifest)? {
        Some(text) => {
            std::fs::write(&file, text)
                .with_context(|| format!("cannot write {}", file.display()))?;
        }
        None => match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("cannot remove {}", file.display())),
        },
    }
    Ok(())
}

/// Checks whether `.gitmodules` has drifted from `manifest`.
pub fn check_gitmodules(
    root: &Path,
    manifest: &Manifest,
    lock: &Lock,
) -> Result<Option<GitmodulesDrift>> {
    let has_manifest_submodules = manifest.repos().any(|(_, r)| r.submodule);
    let had_lock_submodules = lock.repos.iter().any(|r| r.submodule);

    if !has_manifest_submodules && !had_lock_submodules {
        return Ok(None);
    }

    let file = root.join(GITMODULES_FILE);
    let expected = render_gitmodules(manifest)?;

    match (expected, file.exists()) {
        (Some(expected_text), true) => {
            let actual_text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            if actual_text != expected_text {
                Ok(Some(GitmodulesDrift::Modified))
            } else {
                Ok(None)
            }
        }
        (Some(_), false) => Ok(Some(GitmodulesDrift::Missing)),
        (None, true) => Ok(Some(GitmodulesDrift::Orphaned)),
        (None, false) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use catasto_manifest::LockedRepo;

    const USE: &str = "::kind(catasto, version: \"1.0.0\")\n\n";

    #[test]
    fn renders_gitmodules_for_submodule_repos() {
        let manifest = Manifest::parse(&format!(
            "{USE}\
             \"tools/sisho\": catasto.repo(\"https://github.com/tomet-lang/sisho.git\", branch: main, submodule: true)\n\
             \"sisho-desk\": catasto.repo(\"https://github.com/tomet-lang/sisho-desk.git\", submodule: false)\n"
        ))
        .unwrap();

        let rendered = render_gitmodules(&manifest).unwrap().unwrap();
        assert_eq!(
            rendered,
            "[submodule \"tools/sisho\"]\n\tpath = tools/sisho\n\turl = https://github.com/tomet-lang/sisho.git\n\tbranch = main\n\tignore = dirty\n"
        );
    }

    #[test]
    fn renders_none_when_no_submodules() {
        let manifest = Manifest::parse(&format!(
            "{USE}\"tools/sisho\": catasto.repo(\"https://github.com/tomet-lang/sisho.git\")\n"
        ))
        .unwrap();

        assert_eq!(render_gitmodules(&manifest).unwrap(), None);
    }

    #[test]
    fn sync_and_check_gitmodules_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest::parse(&format!(
            "{USE}\"sisho\": catasto.repo(\"https://github.com/tomet-lang/sisho.git\", submodule: true)\n"
        ))
        .unwrap();
        let lock = Lock::default();

        assert_eq!(
            check_gitmodules(dir.path(), &manifest, &lock).unwrap(),
            Some(GitmodulesDrift::Missing)
        );

        sync_gitmodules(dir.path(), &manifest, &lock).unwrap();
        assert_eq!(
            check_gitmodules(dir.path(), &manifest, &lock).unwrap(),
            None
        );

        // Tamper with .gitmodules
        std::fs::write(dir.path().join(GITMODULES_FILE), "tampered").unwrap();
        assert_eq!(
            check_gitmodules(dir.path(), &manifest, &lock).unwrap(),
            Some(GitmodulesDrift::Modified)
        );

        // Removing the submodule from manifest while lock still had it -> Orphaned, then sync removes it
        let empty_manifest = Manifest::default();
        let lock_with_sub = Lock::new(
            Vec::new(),
            vec![LockedRepo {
                path: "sisho".into(),
                git: "https://github.com/tomet-lang/sisho.git".into(),
                branch: None,
                submodule: true,
                commit: None,
            }],
        );
        assert_eq!(
            check_gitmodules(dir.path(), &empty_manifest, &lock_with_sub).unwrap(),
            Some(GitmodulesDrift::Orphaned)
        );
        sync_gitmodules(dir.path(), &empty_manifest, &lock_with_sub).unwrap();
        assert!(!dir.path().join(GITMODULES_FILE).exists());
    }

    #[test]
    fn ignores_unrelated_gitmodules_when_no_submodules_configured() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest::parse(&format!(
            "{USE}\"sisho\": catasto.repo(\"https://github.com/tomet-lang/sisho.git\")\n"
        ))
        .unwrap();
        let lock = Lock::default();

        std::fs::write(dir.path().join(GITMODULES_FILE), "unrelated").unwrap();
        assert_eq!(
            check_gitmodules(dir.path(), &manifest, &lock).unwrap(),
            None
        );

        sync_gitmodules(dir.path(), &manifest, &lock).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(GITMODULES_FILE)).unwrap(),
            "unrelated"
        );
    }
}
