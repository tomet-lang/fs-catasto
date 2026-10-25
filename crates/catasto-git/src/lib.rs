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

/// How symlinks in git repositories are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SymlinkMode {
    /// Resolve symlinks inside the repository and return the target file (default).
    #[default]
    Follow,
    /// Keep the symlink itself; the entry must be a symlink.
    Keep,
}

/// Content read from a git repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// Regular file content.
    File(Vec<u8>),
    /// Symlink target path string.
    Link(String),
}

impl Content {
    /// Returns the bytes of the file, or `None` if it is a symlink.
    pub fn file_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::File(bytes) => Some(bytes),
            Self::Link(_) => None,
        }
    }

    /// Returns the symlink target, or `None` if it is a file.
    pub fn link_target(&self) -> Option<&str> {
        match self {
            Self::Link(target) => Some(target),
            Self::File(_) => None,
        }
    }
}

/// A file or link read from a repository, with the commit it was read at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// Full commit id the revision resolved to (or was pinned to).
    pub commit: String,
    /// Content of the file or link at that commit.
    pub content: Content,
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
    ///
    /// With [`SymlinkMode::Follow`], any symlinks are resolved inside the
    /// repository up to 8 hops with loop detection, returning the final regular
    /// file content as [`Content::File`].
    /// With [`SymlinkMode::Keep`], the entry must be a symlink, and its target
    /// text is returned as [`Content::Link`] without following.
    ///
    /// In both modes, intermediate directory components must not be symlinks,
    /// and symlink targets are validated not to be empty, contain NUL bytes,
    /// be absolute, or escape the repository root.
    pub fn fetch(
        &self,
        url: &str,
        rev: &str,
        path: &Path,
        pinned: Option<&str>,
        symlink: SymlinkMode,
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

        let initial_repo_path = repo_path(path)?;
        check_parent_components_not_symlinks(&repo, &commit, path, &initial_repo_path)?;

        let initial_entry = ls_tree_one(&repo, &commit, &initial_repo_path)?
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
                let text = cat_symlink_text(&repo, &initial_entry.oid, path, &chain)?;
                let _ = resolve_symlink_target(&initial_repo_path, &text, path, &chain)?;
                Ok(Fetched {
                    commit,
                    content: Content::Link(text),
                })
            }
            SymlinkMode::Follow => {
                if initial_entry.mode == "100644" || initial_entry.mode == "100755" {
                    let bytes = cat_blob_bytes(&repo, &initial_entry.oid)?;
                    return Ok(Fetched {
                        commit,
                        content: Content::File(bytes),
                    });
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

                    let text = cat_symlink_text(&repo, &current_oid, path, &chain)?;
                    let next_path = resolve_symlink_target(&current_path, &text, path, &chain)?;

                    if chain.contains(&next_path) {
                        chain.push(next_path);
                        bail!(
                            "cannot fetch {}: symlink loop detected (link chain: {})",
                            path.display(),
                            chain.join(" -> ")
                        );
                    }

                    check_parent_components_not_symlinks(&repo, &commit, path, &next_path)?;

                    let next_entry = match ls_tree_one(&repo, &commit, &next_path)? {
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
                        let bytes = cat_blob_bytes(&repo, &next_entry.oid)?;
                        return Ok(Fetched {
                            commit,
                            content: Content::File(bytes),
                        });
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

#[derive(Debug, Clone)]
struct TreeEntry {
    mode: String,
    kind: String,
    oid: String,
    path: String,
}

fn parse_ls_tree_output(stdout: &[u8]) -> Result<Vec<TreeEntry>> {
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

fn ls_tree_one(repo: &Path, commit: &str, path_in_repo: &str) -> Result<Option<TreeEntry>> {
    let output = run(Some(repo), &["ls-tree", "-z", commit, "--", path_in_repo])?;
    let entries = parse_ls_tree_output(&output.stdout)?;
    Ok(entries.into_iter().find(|e| e.path == path_in_repo))
}

fn check_parent_components_not_symlinks(
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

fn resolve_symlink_target(
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

fn cat_blob_bytes(repo: &Path, oid: &str) -> Result<Vec<u8>> {
    let output = run(Some(repo), &["cat-file", "blob", oid])?;
    Ok(output.stdout)
}

fn cat_symlink_text(
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

        let by_branch = cache
            .fetch(&repo.url(), "main", path, None, SymlinkMode::Follow)
            .unwrap();
        let by_tag = cache
            .fetch(&repo.url(), "v1", path, None, SymlinkMode::Follow)
            .unwrap();
        let by_commit = cache
            .fetch(&repo.url(), &first, path, None, SymlinkMode::Follow)
            .unwrap();

        assert_eq!(
            (by_branch.commit.as_str(), by_branch.content),
            (second.as_str(), Content::File(b"two".to_vec()))
        );
        assert_eq!(
            (by_tag.commit.as_str(), by_tag.content),
            (first.as_str(), Content::File(b"one".to_vec()))
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
            .fetch(
                &repo.url(),
                "main",
                Path::new("w.tmt"),
                Some(&first),
                SymlinkMode::Follow,
            )
            .unwrap();

        assert_eq!(fetched.commit, first);
        assert_eq!(fetched.content, Content::File(b"one".to_vec()));
    }

    #[test]
    fn a_cached_pinned_commit_needs_no_remote() {
        let repo = TestRepo::new();
        let first = repo.commit("w.tmt", "one");
        let (_dir, cache) = cache();
        cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("w.tmt"),
                None,
                SymlinkMode::Follow,
            )
            .unwrap();
        let url = repo.url();
        drop(repo);

        let fetched = cache
            .fetch(
                &url,
                "main",
                Path::new("w.tmt"),
                Some(&first),
                SymlinkMode::Follow,
            )
            .unwrap();

        assert_eq!(fetched.content, Content::File(b"one".to_vec()));
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
                SymlinkMode::Follow,
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
            let result = cache.fetch(
                &repo.url(),
                "main",
                Path::new("w.tmt"),
                Some(bad),
                SymlinkMode::Follow,
            );
            assert!(result.is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn an_unknown_rev_is_an_error() {
        let repo = TestRepo::new();
        repo.commit("w.tmt", "one");
        let (_dir, cache) = cache();

        let err = cache
            .fetch(
                &repo.url(),
                "nope",
                Path::new("w.tmt"),
                None,
                SymlinkMode::Follow,
            )
            .unwrap_err();

        assert!(err.to_string().contains("\"nope\" not found"), "{err}");
    }

    #[test]
    fn an_unknown_path_is_an_error() {
        let repo = TestRepo::new();
        repo.commit("w.tmt", "one");
        let (_dir, cache) = cache();

        let err = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("nope.tmt"),
                None,
                SymlinkMode::Follow,
            )
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
        cache
            .fetch(&repo.url(), "stable", path, None, SymlinkMode::Follow)
            .unwrap();
        repo.git(&["checkout", "--quiet", "stable"]);
        let moved = repo.commit("w.tmt", "two");

        let fetched = cache
            .fetch(&repo.url(), "stable", path, None, SymlinkMode::Follow)
            .unwrap();

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
            .fetch(
                "ssh://localhost/repo.git",
                "main",
                Path::new("w.tmt"),
                None,
                SymlinkMode::Follow,
            )
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
            cache
                .fetch(&a.url(), "main", path, None, SymlinkMode::Follow)
                .unwrap()
                .content,
            Content::File(b"a".to_vec())
        );
        assert_eq!(
            cache
                .fetch(&b.url(), "main", path, None, SymlinkMode::Follow)
                .unwrap()
                .content,
            Content::File(b"b".to_vec())
        );
    }

    #[test]
    fn fetches_a_tree_as_error() {
        let repo = TestRepo::new();
        repo.commit("dir/file.txt", "content");
        let (_dir, cache) = cache();

        let err = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("dir"),
                None,
                SymlinkMode::Follow,
            )
            .unwrap_err();

        assert!(
            err.to_string().contains("is a directory, not a file"),
            "{err}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn follow_one_hop_and_several_hops_inside_repo() {
        let repo = TestRepo::new();
        repo.commit("target/real.txt", "hello");
        repo.commit_symlink("link1", "target/real.txt");
        repo.commit_symlink("nested/link2", "../link1");
        repo.commit_symlink("nested/deep/link3", "../link2");

        let (_dir, cache) = cache();

        let f1 = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("link1"),
                None,
                SymlinkMode::Follow,
            )
            .unwrap();
        assert_eq!(f1.content, Content::File(b"hello".to_vec()));

        let f3 = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("nested/deep/link3"),
                None,
                SymlinkMode::Follow,
            )
            .unwrap();
        assert_eq!(f3.content, Content::File(b"hello".to_vec()));
    }

    #[test]
    #[cfg(unix)]
    fn follow_eight_hops_ok_nine_hops_error() {
        let repo = TestRepo::new();
        repo.commit("real.txt", "found");
        // link1 -> real.txt (1 hop)
        repo.commit_symlink("l1", "real.txt");
        for i in 2..=10 {
            repo.commit_symlink(&format!("l{i}"), &format!("l{}", i - 1));
        }

        let (_dir, cache) = cache();

        // l8 -> l7 -> l6 -> l5 -> l4 -> l3 -> l2 -> l1 -> real.txt (8 hops to reach real.txt)
        let f8 = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("l8"),
                None,
                SymlinkMode::Follow,
            )
            .unwrap();
        assert_eq!(f8.content, Content::File(b"found".to_vec()));

        // l9 -> 8 links before reaching real.txt requires 9 hops -> error
        let err9 = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("l9"),
                None,
                SymlinkMode::Follow,
            )
            .unwrap_err();
        assert!(
            err9.to_string()
                .contains("too many levels of symbolic links"),
            "{err9}"
        );
        assert!(err9.to_string().contains("link chain:"), "{err9}");
    }

    #[test]
    #[cfg(unix)]
    fn follow_loop_is_an_error() {
        let repo = TestRepo::new();
        repo.commit_symlink("a", "b");
        repo.commit_symlink("b", "a");

        let (_dir, cache) = cache();
        let err = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("a"),
                None,
                SymlinkMode::Follow,
            )
            .unwrap_err();
        assert!(err.to_string().contains("symlink loop detected"), "{err}");
        assert!(err.to_string().contains("link chain: a -> b -> a"), "{err}");
    }

    #[test]
    #[cfg(unix)]
    fn follow_bad_target_errors() {
        let repo = TestRepo::new();
        repo.commit("real.txt", "ok");
        repo.commit_symlink("abs", "/etc/passwd");
        repo.commit_symlink("escape", "../../secret");
        repo.commit_symlink("dangling", "missing.txt");
        repo.commit_symlink("to_dir", "dir");
        repo.commit("dir/inside.txt", "inside");

        let (_dir, cache) = cache();

        let err_abs = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("abs"),
                None,
                SymlinkMode::Follow,
            )
            .unwrap_err();
        assert!(
            err_abs.to_string().contains("symlink target is absolute"),
            "{err_abs}"
        );

        let err_escape = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("escape"),
                None,
                SymlinkMode::Follow,
            )
            .unwrap_err();
        assert!(
            err_escape
                .to_string()
                .contains("climbs out of the repository"),
            "{err_escape}"
        );

        let err_dangling = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("dangling"),
                None,
                SymlinkMode::Follow,
            )
            .unwrap_err();
        assert!(
            err_dangling
                .to_string()
                .contains("target `missing.txt` does not exist"),
            "{err_dangling}"
        );

        let err_dir = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("to_dir"),
                None,
                SymlinkMode::Follow,
            )
            .unwrap_err();
        assert!(
            err_dir.to_string().contains("target `dir` is a directory"),
            "{err_dir}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn keep_mode_checks() {
        let repo = TestRepo::new();
        repo.commit("real.txt", "hello");
        repo.commit_symlink("link.txt", "target.txt");
        repo.commit_symlink("bad_abs", "/abs");

        let (_dir, cache) = cache();

        // Plain link ok, even if target is dangling
        let fetched = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("link.txt"),
                None,
                SymlinkMode::Keep,
            )
            .unwrap();
        assert_eq!(fetched.content, Content::Link("target.txt".to_owned()));

        // Not a link is an error
        let err_not_link = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("real.txt"),
                None,
                SymlinkMode::Keep,
            )
            .unwrap_err();
        assert!(
            err_not_link
                .to_string()
                .contains("`symlink: keep`, but real.txt is not a symlink"),
            "{err_not_link}"
        );

        // Bad target text is an error even with keep
        let err_bad = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("bad_abs"),
                None,
                SymlinkMode::Keep,
            )
            .unwrap_err();
        assert!(
            err_bad.to_string().contains("symlink target is absolute"),
            "{err_bad}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn symlinked_directory_component_is_an_error() {
        let repo = TestRepo::new();
        repo.commit("realdir/file.txt", "deep content");
        repo.commit_symlink("symdir", "realdir");

        let (_dir, cache) = cache();

        let err = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("symdir/file.txt"),
                None,
                SymlinkMode::Follow,
            )
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("directory component `symdir` of symdir/file.txt is a symlink"),
            "{err}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn pinned_commit_still_resolves_link_at_pinned_commit() {
        let repo = TestRepo::new();
        repo.commit("v1.txt", "version 1");
        repo.commit("v2.txt", "version 2");
        let first = repo.commit_symlink("link", "v1.txt");
        repo.commit_symlink("link", "v2.txt");

        let (_dir, cache) = cache();

        let fetched = cache
            .fetch(
                &repo.url(),
                "main",
                Path::new("link"),
                Some(&first),
                SymlinkMode::Follow,
            )
            .unwrap();
        assert_eq!(fetched.commit, first);
        assert_eq!(fetched.content, Content::File(b"version 1".to_vec()));
    }
}
