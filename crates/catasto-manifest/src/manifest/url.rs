//! Git URL classification and validation.

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
