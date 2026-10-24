//! Unified diff between what is on disk and what `sync` would write.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use catasto_git::Content;
use similar::TextDiff;

use crate::manifest::Source;
use crate::sync::{SyncMode, plan_sync};

/// Computes the difference between the files on disk and what `sync` (mode `Locked`)
/// would write. Returns unified diff text, empty when nothing differs.
pub fn diff(root: &Path, cache_dir: Option<&Path>, only: &[PathBuf]) -> Result<String> {
    let plan = plan_sync(root, SyncMode::Locked, cache_dir, only)?;
    let mut output = String::new();

    for file in &plan.files {
        let dest_path = &file.destination;
        let source_note = match &file.source {
            Source::Git { .. } => {
                let short = file
                    .commit
                    .as_deref()
                    .map(|c| &c[..c.len().min(7)])
                    .unwrap_or("unknown");
                format!("git {short}")
            }
            Source::Path(p) => format!("local {}", p.display()),
        };

        match std::fs::symlink_metadata(dest_path) {
            Ok(meta) => {
                if meta.is_symlink() {
                    let disk_target = std::fs::read_link(dest_path)
                        .with_context(|| format!("cannot read link {}", dest_path.display()))?;
                    match &file.content {
                        Content::Link(target) => {
                            if disk_target.to_str() != Some(target) {
                                output.push_str(&format!(
                                    "link {}: {} -> {target}\n",
                                    file.path.display(),
                                    disk_target.display()
                                ));
                            }
                        }
                        Content::File(_) => {
                            output.push_str(&format!(
                                "{}: symlink becomes regular file\n",
                                file.path.display()
                            ));
                        }
                    }
                } else {
                    match &file.content {
                        Content::Link(target) => {
                            output.push_str(&format!(
                                "{}: regular file becomes symlink to {target}\n",
                                file.path.display()
                            ));
                        }
                        Content::File(source_bytes) => {
                            let disk_bytes = std::fs::read(dest_path).with_context(|| {
                                format!("cannot read {}", dest_path.display())
                            })?;
                            if &disk_bytes != source_bytes {
                                let disk_str = std::str::from_utf8(&disk_bytes);
                                let source_str = std::str::from_utf8(source_bytes);
                                match (disk_str, source_str) {
                                    (Ok(d), Ok(s)) => {
                                        let diff = TextDiff::from_lines(d, s)
                                            .unified_diff()
                                            .context_radius(3)
                                            .header(
                                                &format!("a/{}", file.path.display()),
                                                &format!(
                                                    "b/{}\t{}",
                                                    file.path.display(),
                                                    source_note
                                                ),
                                            )
                                            .to_string();
                                        output.push_str(&diff);
                                    }
                                    _ => {
                                        output.push_str(&format!(
                                            "Binary files differ: {}\n",
                                            file.path.display()
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => match &file.content {
                Content::File(source_bytes) => match std::str::from_utf8(source_bytes) {
                    Ok(s) => {
                        let diff = TextDiff::from_lines("", s)
                            .unified_diff()
                            .context_radius(3)
                            .header(
                                "/dev/null",
                                &format!("b/{}\t{}", file.path.display(), source_note),
                            )
                            .to_string();
                        output.push_str(&diff);
                    }
                    Err(_) => {
                        output.push_str(&format!(
                            "Binary files differ: {}\n",
                            file.path.display()
                        ));
                    }
                },
                Content::Link(target) => {
                    output.push_str(&format!(
                        "link {}: /dev/null -> {target}\n",
                        file.path.display()
                    ));
                }
            },
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("cannot inspect {}", dest_path.display()));
            }
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests;
