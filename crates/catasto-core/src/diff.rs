//! Unified diff between what is on disk and what `sync` would write.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use catasto_git::{Content, DirEntry, DirEntryContent};
use similar::TextDiff;

use crate::manifest::Source;
use crate::sync::{PlannedFile, SyncMode, plan_sync};

/// Computes the difference between the files on disk and what `sync` (mode `Locked`)
/// would write. Returns unified diff text, empty when nothing differs.
pub fn diff(root: &Path, cache_dir: Option<&Path>, only: &[PathBuf]) -> Result<String> {
    let plan = plan_sync(root, SyncMode::Locked, cache_dir, only)?;
    let mut output = String::new();

    for file in &plan.files {
        if let Some(hunk) = diff_file(file)? {
            output.push_str(&hunk);
        }
    }

    Ok(output)
}

/// Computes the diff between the on-disk file or directory and the planned content.
/// Returns `Ok(Some(diff_text))` if there is a difference, or `Ok(None)` if identical.
pub fn diff_file(file: &PlannedFile) -> Result<Option<String>> {
    let note = source_note(&file.source, file.commit.as_deref());
    if let Content::Dir(entries) = &file.content {
        return diff_dir(file, entries, &note);
    }

    let dest_path = &file.destination;
    match std::fs::symlink_metadata(dest_path) {
        Ok(meta) if meta.is_dir() => {
            let mut out = String::new();
            for (rel_path, abs_path, is_symlink) in collect_disk_entries(dest_path)? {
                out.push_str(&render_removed_entry(
                    &file.path.join(&rel_path),
                    &abs_path,
                    is_symlink,
                )?);
            }
            out.push_str(&diff_missing_content(&file.path, &file.content, &note)?);
            Ok(Some(out))
        }
        Ok(meta) => {
            if meta.is_symlink() {
                let disk_target = std::fs::read_link(dest_path)
                    .with_context(|| format!("cannot read link {}", dest_path.display()))?;
                match &file.content {
                    Content::Link(target) => {
                        if disk_target.to_str() != Some(target) {
                            Ok(Some(format!(
                                "link {}: {} -> {target}\n",
                                file.path.display(),
                                disk_target.display()
                            )))
                        } else {
                            Ok(None)
                        }
                    }
                    Content::File(_) => Ok(Some(format!(
                        "{}: symlink becomes regular file\n",
                        file.path.display()
                    ))),
                    Content::Dir(_) => unreachable!("handled above"),
                }
            } else {
                match &file.content {
                    Content::Link(target) => Ok(Some(format!(
                        "{}: regular file becomes symlink to {target}\n",
                        file.path.display()
                    ))),
                    Content::File(source_bytes) => {
                        let disk_bytes = std::fs::read(dest_path)
                            .with_context(|| format!("cannot read {}", dest_path.display()))?;
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
                                            &format!("b/{}\t{}", file.path.display(), note),
                                        )
                                        .to_string();
                                    Ok(Some(diff))
                                }
                                _ => Ok(Some(format!(
                                    "Binary files differ: {}\n",
                                    file.path.display()
                                ))),
                            }
                        } else {
                            Ok(None)
                        }
                    }
                    Content::Dir(_) => unreachable!("handled above"),
                }
            }
        }
        Err(e) if is_missing_or_not_dir(&e) => {
            Ok(Some(diff_missing_content(&file.path, &file.content, &note)?))
        }
        Err(e) => Err(e).with_context(|| format!("cannot inspect {}", dest_path.display())),
    }
}

fn source_note(source: &Source, commit: Option<&str>) -> String {
    match source {
        Source::Git { .. } => {
            let short = commit.map(|c| &c[..c.len().min(7)]).unwrap_or("unknown");
            format!("git {short}")
        }
        Source::Path(p) => format!("local {}", p.display()),
    }
}

fn is_missing_or_not_dir(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::NotFound || e.kind() == std::io::ErrorKind::NotADirectory
}

fn diff_missing_content(path: &Path, content: &Content, note: &str) -> Result<String> {
    match content {
        Content::File(source_bytes) => match std::str::from_utf8(source_bytes) {
            Ok(s) => Ok(TextDiff::from_lines("", s)
                .unified_diff()
                .context_radius(3)
                .header("/dev/null", &format!("b/{}\t{}", path.display(), note))
                .to_string()),
            Err(_) => Ok(format!("Binary files differ: {}\n", path.display())),
        },
        Content::Link(target) => Ok(format!("link {}: /dev/null -> {target}\n", path.display())),
        Content::Dir(_) => Ok(String::new()),
    }
}

fn render_removed_entry(rel_display: &Path, disk_path: &Path, is_symlink: bool) -> Result<String> {
    if is_symlink {
        let disk_target = std::fs::read_link(disk_path)
            .with_context(|| format!("cannot read link {}", disk_path.display()))?;
        return Ok(format!(
            "link {}: {} -> /dev/null\n",
            rel_display.display(),
            disk_target.display()
        ));
    }
    let disk_bytes = std::fs::read(disk_path)
        .with_context(|| format!("cannot read {}", disk_path.display()))?;
    match std::str::from_utf8(&disk_bytes) {
        Ok(d) => Ok(TextDiff::from_lines(d, "")
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{}", rel_display.display()), "/dev/null")
            .to_string()),
        Err(_) => Ok(format!("Binary file removed: {}\n", rel_display.display())),
    }
}

fn diff_dir(file: &PlannedFile, entries: &[DirEntry], note: &str) -> Result<Option<String>> {
    let mut out = String::new();
    match std::fs::symlink_metadata(&file.destination) {
        Ok(meta) if meta.is_dir() => {
            for entry in entries {
                let child = PlannedFile {
                    path: file.path.join(&entry.rel_path),
                    destination: file.destination.join(&entry.rel_path),
                    content: match &entry.content {
                        DirEntryContent::File { bytes, .. } => Content::File(bytes.clone()),
                        DirEntryContent::Link(target) => Content::Link(target.clone()),
                    },
                    commit: file.commit.clone(),
                    source: file.source.clone(),
                    upstream: file.upstream.clone(),
                    executable: match &entry.content {
                        DirEntryContent::File { executable, .. } => {
                            file.declared_executable.unwrap_or(*executable)
                        }
                        DirEntryContent::Link(_) => false,
                    },
                    declared_executable: None,
                };
                if let Some(hunk) = diff_file(&child)? {
                    out.push_str(&hunk);
                }
            }

            let expected: HashSet<&Path> = entries.iter().map(|e| e.rel_path.as_path()).collect();
            for (rel_path, abs_path, is_symlink) in collect_disk_entries(&file.destination)? {
                if !expected.contains(rel_path.as_path()) {
                    out.push_str(&render_removed_entry(
                        &file.path.join(&rel_path),
                        &abs_path,
                        is_symlink,
                    )?);
                }
            }
        }
        Ok(meta) => {
            out.push_str(&render_removed_entry(
                &file.path,
                &file.destination,
                meta.is_symlink(),
            )?);
            for entry in entries {
                let child_content = match &entry.content {
                    DirEntryContent::File { bytes, .. } => Content::File(bytes.clone()),
                    DirEntryContent::Link(target) => Content::Link(target.clone()),
                };
                out.push_str(&diff_missing_content(
                    &file.path.join(&entry.rel_path),
                    &child_content,
                    note,
                )?);
            }
        }
        Err(e) if is_missing_or_not_dir(&e) => {
            for entry in entries {
                let child_content = match &entry.content {
                    DirEntryContent::File { bytes, .. } => Content::File(bytes.clone()),
                    DirEntryContent::Link(target) => Content::Link(target.clone()),
                };
                out.push_str(&diff_missing_content(
                    &file.path.join(&entry.rel_path),
                    &child_content,
                    note,
                )?);
            }
        }
        Err(e) => {
            return Err(e).with_context(|| format!("cannot inspect {}", file.destination.display()));
        }
    }

    if out.is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

/// Collects `(rel_path, abs_path, is_symlink)` for all non-directory entries under `dir`, sorted by `rel_path`.
fn collect_disk_entries(dir: &Path) -> Result<Vec<(PathBuf, PathBuf, bool)>> {
    fn walk(
        dir: &Path,
        prefix: &Path,
        out: &mut Vec<(PathBuf, PathBuf, bool)>,
    ) -> Result<()> {
        let read_dir = std::fs::read_dir(dir)
            .with_context(|| format!("cannot read directory {}", dir.display()))?;
        for item in read_dir {
            let item = item.with_context(|| format!("cannot read entry in {}", dir.display()))?;
            let abs = item.path();
            let rel = if prefix.as_os_str().is_empty() {
                PathBuf::from(item.file_name())
            } else {
                prefix.join(item.file_name())
            };
            let meta = std::fs::symlink_metadata(&abs)
                .with_context(|| format!("cannot inspect {}", abs.display()))?;
            if meta.is_symlink() {
                out.push((rel, abs, true));
            } else if meta.is_dir() {
                walk(&abs, &rel, out)?;
            } else {
                out.push((rel, abs, false));
            }
        }
        Ok(())
    }

    let mut out = Vec::new();
    walk(dir, Path::new(""), &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

#[cfg(test)]
mod tests;
