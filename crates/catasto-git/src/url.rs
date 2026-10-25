use std::path::Path;

use anyhow::{Context, Result};

pub use catasto_manifest::manifest::url::{UrlKind, classify};

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
