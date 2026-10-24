//! `catasto-git` -- reading files out of git repositories through the system `git`.
//!
//! Each repository URL gets one bare clone in the cache directory. Nothing is
//! written into the workspace.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};

use catasto_digest::sha256_hex;

/// Environment variable that overrides the cache directory.
pub const CACHE_DIR_ENV: &str = "CATASTO_CACHE_DIR";

/// Where a repository lives, as far as catasto accepts it.
///
/// There is deliberately no ssh: `git@host:path`, `ssh://`, `git://` and plain
/// `http://` are rejected, so catasto never needs keys, agents or host config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlKind {
    /// An `https://` URL.
    Https,
    /// A path on the local filesystem, absolute or relative to the workspace.
    Local,
}

/// Classifies a `git` value from `catasto.ctmt`.
pub fn classify(url: &str) -> Result<UrlKind, String> {
    if url.is_empty() || url.starts_with('-') {
        return Err(format!("invalid git url: {url:?}"));
    }
    if let Some(rest) = url.strip_prefix("https://") {
        if rest.is_empty() || rest.starts_with('/') {
            return Err(format!("invalid git url: {url:?}"));
        }
        return Ok(UrlKind::Https);
    }
    if url.contains("://") {
        return Err(format!(
            "unsupported git url {url:?}: use an https:// url or a local path"
        ));
    }
    // git reads `host:path` and `user@host:path` as ssh when the colon comes
    // before any slash; `ext::command` is covered by the same rule.
    let colon = url.find(':');
    let slash = url.find('/');
    if colon.is_some_and(|c| slash.is_none_or(|s| c < s)) {
        return Err(format!(
            "unsupported git url {url:?}: ssh is not supported, use an https:// url or a local path"
        ));
    }
    Ok(UrlKind::Local)
}

/// The URL to hand to `git`: remote URLs as written, local paths made
/// absolute against the workspace `root`.
pub fn resolve_url(root: &Path, url: &str) -> Result<String> {
    match classify(url).map_err(anyhow::Error::msg)? {
        UrlKind::Https => Ok(url.to_owned()),
        UrlKind::Local => {
            let path = std::path::absolute(root.join(url))
                .with_context(|| format!("cannot resolve {url:?} against {}", root.display()))?;
            path.into_os_string()
                .into_string()
                .map_err(|_| anyhow::anyhow!("git path is not valid UTF-8: {url:?}"))
        }
    }
}

/// A file read from a repository, with the commit it was read at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// Full commit id the revision resolved to (or was pinned to).
    pub commit: String,
    /// Content of the file at that commit.
    pub content: Vec<u8>,
}

/// Bare clones of the repositories catasto reads from.
#[derive(Debug, Clone)]
pub struct GitCache {
    dir: PathBuf,
}

impl GitCache {
    /// A cache rooted at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The default cache directory: `$CATASTO_CACHE_DIR`, else
    /// `$XDG_CACHE_HOME/catasto`, else `~/.cache/catasto`.
    pub fn default_dir() -> Result<PathBuf> {
        if let Some(dir) = non_empty_env(CACHE_DIR_ENV) {
            return Ok(dir.into());
        }
        if let Some(dir) = non_empty_env("XDG_CACHE_HOME") {
            return Ok(PathBuf::from(dir).join("catasto"));
        }
        if let Some(home) = non_empty_env("HOME") {
            return Ok(PathBuf::from(home).join(".cache").join("catasto"));
        }
        bail!("cannot locate a cache directory: set {CACHE_DIR_ENV}")
    }

    /// Reads `path` from the repository at `url`.
    ///
    /// With `pinned`, that commit is used and the network is touched only if
    /// the commit is not cached yet. Without it, the repository is fetched and
    /// `rev` (a tag, a branch, or a commit already reachable from one) is
    /// resolved to a commit.
    pub fn fetch(
        &self,
        url: &str,
        rev: &str,
        path: &Path,
        pinned: Option<&str>,
    ) -> Result<Fetched> {
        let repo = self.repo_dir(url);
        init_bare(&repo)?;

        let commit = match pinned {
            Some(commit) => {
                check_commit_id(commit)?;
                if !has_commit(&repo, commit) {
                    fetch_remote(&repo, url)?;
                    if !has_commit(&repo, commit) {
                        bail!("locked commit {commit} is not in {url}");
                    }
                }
                commit.to_owned()
            }
            None => {
                fetch_remote(&repo, url)?;
                resolve(&repo, url, rev)?
            }
        };

        let spec = format!("{commit}:{}", repo_path(path)?);
        let blob = run(Some(&repo), &["cat-file", "blob", &spec])
            .with_context(|| format!("{} is not in {url} at {commit}", path.display()))?;
        Ok(Fetched {
            commit,
            content: blob.stdout,
        })
    }

    fn repo_dir(&self, url: &str) -> PathBuf {
        let key = sha256_hex(url.as_bytes());
        self.dir.join("git").join(&key[..16])
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn init_bare(repo: &Path) -> Result<()> {
    if repo.join("HEAD").exists() {
        return Ok(());
    }
    std::fs::create_dir_all(repo).with_context(|| format!("cannot create {}", repo.display()))?;
    let repo = repo.to_str().context("cache path is not valid UTF-8")?;
    run(None, &["init", "--bare", "--quiet", repo])?;
    Ok(())
}

fn fetch_remote(repo: &Path, url: &str) -> Result<()> {
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

fn resolve(repo: &Path, url: &str, rev: &str) -> Result<String> {
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

fn has_commit(repo: &Path, commit: &str) -> bool {
    run(
        Some(repo),
        &["cat-file", "-e", &format!("{commit}^{{commit}}")],
    )
    .is_ok()
}

/// Commit ids come from a hand-editable lockfile and end up as `git` arguments.
fn check_commit_id(commit: &str) -> Result<()> {
    let hex = commit.bytes().all(|b| b.is_ascii_hexdigit());
    if !hex || !matches!(commit.len(), 40 | 64) {
        bail!("not a full commit id: {commit:?}");
    }
    Ok(())
}

fn repo_path(path: &Path) -> Result<String> {
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

fn run(git_dir: Option<&Path>, args: &[&str]) -> Result<Output> {
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

/// Throwaway repositories for tests, behind the `testing` feature.
#[cfg(any(test, feature = "testing"))]
pub mod testing {
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
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::TestRepo;
    use super::*;

    fn cache() -> (tempfile::TempDir, GitCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = GitCache::new(dir.path());
        (dir, cache)
    }

    #[test]
    fn resolves_a_branch_a_tag_and_a_commit() {
        let repo = TestRepo::new();
        let first = repo.commit("dir/w.tmt", "one");
        repo.tag("v1");
        let second = repo.commit("dir/w.tmt", "two");
        let (_dir, cache) = cache();
        let path = Path::new("dir/w.tmt");

        let by_branch = cache.fetch(&repo.url(), "main", path, None).unwrap();
        let by_tag = cache.fetch(&repo.url(), "v1", path, None).unwrap();
        let by_commit = cache.fetch(&repo.url(), &first, path, None).unwrap();

        assert_eq!(
            (by_branch.commit.as_str(), by_branch.content),
            (second.as_str(), b"two".to_vec())
        );
        assert_eq!(
            (by_tag.commit.as_str(), by_tag.content),
            (first.as_str(), b"one".to_vec())
        );
        assert_eq!(by_commit.commit, first);
    }

    #[test]
    fn a_pinned_commit_ignores_where_the_branch_went() {
        let repo = TestRepo::new();
        let first = repo.commit("w.tmt", "one");
        repo.commit("w.tmt", "two");
        let (_dir, cache) = cache();

        let fetched = cache
            .fetch(&repo.url(), "main", Path::new("w.tmt"), Some(&first))
            .unwrap();

        assert_eq!(fetched.commit, first);
        assert_eq!(fetched.content, b"one");
    }

    #[test]
    fn a_cached_pinned_commit_needs_no_remote() {
        let repo = TestRepo::new();
        let first = repo.commit("w.tmt", "one");
        let (_dir, cache) = cache();
        cache
            .fetch(&repo.url(), "main", Path::new("w.tmt"), None)
            .unwrap();
        let url = repo.url();
        drop(repo);

        let fetched = cache
            .fetch(&url, "main", Path::new("w.tmt"), Some(&first))
            .unwrap();

        assert_eq!(fetched.content, b"one");
    }

    #[test]
    fn a_pinned_commit_missing_from_the_remote_is_an_error() {
        let repo = TestRepo::new();
        repo.commit("w.tmt", "one");
        let (_dir, cache) = cache();

        let err = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("w.tmt"),
                Some(&"0".repeat(40)),
            )
            .unwrap_err();

        assert!(err.to_string().contains("is not in"), "{err}");
    }

    #[test]
    fn a_malformed_pinned_commit_is_rejected_before_running_git() {
        let repo = TestRepo::new();
        repo.commit("w.tmt", "one");
        let (_dir, cache) = cache();

        for bad in ["--upload-pack=x", "abc", "main"] {
            let result = cache.fetch(&repo.url(), "main", Path::new("w.tmt"), Some(bad));
            assert!(result.is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn an_unknown_rev_is_an_error() {
        let repo = TestRepo::new();
        repo.commit("w.tmt", "one");
        let (_dir, cache) = cache();

        let err = cache
            .fetch(&repo.url(), "nope", Path::new("w.tmt"), None)
            .unwrap_err();

        assert!(err.to_string().contains("\"nope\" not found"), "{err}");
    }

    #[test]
    fn an_unknown_path_is_an_error() {
        let repo = TestRepo::new();
        repo.commit("w.tmt", "one");
        let (_dir, cache) = cache();

        let err = cache
            .fetch(&repo.url(), "main", Path::new("nope.tmt"), None)
            .unwrap_err();

        assert!(err.to_string().contains("nope.tmt is not in"), "{err}");
    }

    #[test]
    fn a_moved_branch_is_picked_up_on_the_next_fetch() {
        let repo = TestRepo::new();
        repo.commit("w.tmt", "one");
        repo.branch("stable");
        let (_dir, cache) = cache();
        let path = Path::new("w.tmt");
        cache.fetch(&repo.url(), "stable", path, None).unwrap();
        repo.git(&["checkout", "--quiet", "stable"]);
        let moved = repo.commit("w.tmt", "two");

        let fetched = cache.fetch(&repo.url(), "stable", path, None).unwrap();

        assert_eq!(fetched.commit, moved);
    }

    #[test]
    fn accepts_https_urls_and_local_paths() {
        for url in [
            "https://github.com/tomet-lang/twrit.git",
            "/abs/path/repo",
            "../repo",
            "repo",
            "dir/with:colon",
            "./host:path",
        ] {
            assert!(classify(url).is_ok(), "rejected {url}");
        }
    }

    #[test]
    fn rejects_ssh_and_every_other_transport() {
        for url in [
            "git@github.com:tomet-lang/twrit.git",
            "github-alias:tomet-lang/twrit.git",
            "ssh://git@github.com/tomet-lang/twrit.git",
            "git://example.com/repo.git",
            "http://example.com/repo.git",
            "file:///abs/path/repo",
            "ext::sh -c id",
            "https://",
            "https:///x",
            "-oProxyCommand=x",
            "",
        ] {
            assert!(classify(url).is_err(), "accepted {url}");
        }
    }

    #[test]
    fn local_paths_resolve_against_the_workspace_root() {
        let root = Path::new("/ws/vault");

        assert_eq!(
            resolve_url(root, "../remote").unwrap(),
            "/ws/vault/../remote"
        );
        assert_eq!(resolve_url(root, "remote").unwrap(), "/ws/vault/remote");
        assert_eq!(resolve_url(root, "/abs/repo").unwrap(), "/abs/repo");
        assert_eq!(
            resolve_url(root, "https://example.com/r.git").unwrap(),
            "https://example.com/r.git"
        );
    }

    #[test]
    fn git_itself_refuses_other_transports() {
        let (_dir, cache) = cache();

        let err = cache
            .fetch("ssh://localhost/repo.git", "main", Path::new("w.tmt"), None)
            .unwrap_err();

        assert!(format!("{err:#}").contains("not allowed"), "{err:#}");
    }

    #[test]
    fn the_cache_keeps_one_clone_per_url() {
        let a = TestRepo::new();
        a.commit("w.tmt", "a");
        let b = TestRepo::new();
        b.commit("w.tmt", "b");
        let (_dir, cache) = cache();
        let path = Path::new("w.tmt");

        assert_eq!(
            cache.fetch(&a.url(), "main", path, None).unwrap().content,
            b"a"
        );
        assert_eq!(
            cache.fetch(&b.url(), "main", path, None).unwrap().content,
            b"b"
        );
    }
}
