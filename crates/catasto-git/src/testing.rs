//! Throwaway repositories for tests, behind the `testing` feature.

use std::path::Path;
use std::process::Command;

/// A throwaway repository on branch `main`.
pub struct TestRepo {
    dir: tempfile::TempDir,
}

impl Default for TestRepo {
    fn default() -> Self {
        Self::new()
    }
}

impl TestRepo {
    pub fn new() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        repo.git(&["init", "--quiet", "-b", "main"]);
        repo
    }

    /// The URL to hand to catasto: the repository's own path.
    pub fn url(&self) -> String {
        self.dir.path().to_str().unwrap().to_owned()
    }

    /// Writes `path`, commits it, and returns the commit id.
    pub fn commit(&self, path: &str, content: &str) -> String {
        let file = self.dir.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "-m", content]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// Creates a symlink at `path` pointing to `target`, commits it, and returns the commit id.
    #[cfg(unix)]
    pub fn commit_symlink(&self, path: &str, target: &str) -> String {
        let link = self.dir.path().join(path);
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(target, &link).unwrap();
        self.git(&["add", "--all"]);
        self.git(&[
            "commit",
            "--quiet",
            "-m",
            &format!("link {path} -> {target}"),
        ]);
        self.git(&["rev-parse", "HEAD"])
    }

    pub fn tag(&self, name: &str) {
        self.git(&["tag", name]);
    }

    pub fn branch(&self, name: &str) {
        self.git(&["branch", name]);
    }

    pub fn git(&self, args: &[&str]) -> String {
        run_git(self.dir.path(), args)
    }
}

pub fn run_git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=catasto-test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
