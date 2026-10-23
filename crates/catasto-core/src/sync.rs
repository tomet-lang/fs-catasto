//! Re-fetching managed files from their sources.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::lockfile::{Lock, lock};
use crate::manifest::Manifest;

/// Copies every managed file from its source, then rewrites `catasto.lock`.
///
/// Every source is read before anything is written, so a missing source leaves
/// the workspace untouched.
pub fn sync(root: &Path) -> Result<Lock> {
    let manifest = Manifest::load(root)?;

    let mut fetched = Vec::with_capacity(manifest.managed.len());
    for managed in &manifest.managed {
        let source = managed.source.resolve(root);
        if !source.is_file() {
            bail!(
                "source of {} does not exist: {}",
                managed.path.display(),
                source.display()
            );
        }
        let content =
            std::fs::read(&source).with_context(|| format!("cannot read {}", source.display()))?;
        fetched.push((root.join(&managed.path), content));
    }

    for (target, content) in fetched {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        std::fs::write(&target, content)
            .with_context(|| format!("cannot write {}", target.display()))?;
    }

    lock(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::check;

    const MANIFEST: &str = "\
[[managed]]
path = \"vocab/writ.tmt\"
source = { path = \"../vocabularies/writ.tmt\" }
";

    fn workspace() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        let source = tmp.path().join("vocabularies/writ.tmt");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(vault.join("catasto.toml"), MANIFEST).unwrap();
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
}
