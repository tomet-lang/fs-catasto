use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{Violation, ViolationKind};
use crate::manifest::{EntryRules, Zone};

struct ChildEntry {
    name: String,
    abs_path: PathBuf,
    rel_path: PathBuf,
    is_dir: bool,
    is_symlink: bool,
}

/// Evaluates all `::zone(...)` rules across directories under `root`.
///
/// Always skips `.git`, never follows directory symlinks, and skips recursing
/// into subdirectories ignored by every active zone on their parent.
pub(super) fn check_zones(root: &Path, zones: &[Zone]) -> Result<Vec<Violation>> {
    if zones.is_empty() {
        return Ok(Vec::new());
    }
    let mut violations = Vec::new();
    let inherited = vec![false; zones.len()];
    walk_dir(root, ".", zones, &inherited, &mut violations)?;
    Ok(violations)
}

fn walk_dir(
    abs_dir: &Path,
    rel_dir: &str,
    zones: &[Zone],
    inherited: &[bool],
    violations: &mut Vec<Violation>,
) -> Result<()> {
    let read = std::fs::read_dir(abs_dir)
        .with_context(|| format!("failed to read directory {}", abs_dir.display()))?;
    let mut raw_entries: Vec<_> = read
        .collect::<std::result::Result<Vec<_>, std::io::Error>>()
        .with_context(|| format!("failed to read entries in {}", abs_dir.display()))?;
    raw_entries.sort_by_key(|entry| entry.file_name());

    let mut children = Vec::with_capacity(raw_entries.len());
    for entry in raw_entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to get file type for {}", entry.path().display()))?;
        let rel_path = if rel_dir == "." {
            PathBuf::from(&name)
        } else {
            PathBuf::from(rel_dir).join(&name)
        };
        children.push(ChildEntry {
            name,
            abs_path: entry.path(),
            rel_path,
            is_dir: file_type.is_dir(),
            is_symlink: file_type.is_symlink(),
        });
    }

    let active: Vec<bool> = zones
        .iter()
        .zip(inherited)
        .map(|(zone, &was_inherited)| was_inherited || zone.target.is_match(rel_dir))
        .collect();
    let has_active = active.iter().any(|&is_active| is_active);

    for (zone, &is_active) in zones.iter().zip(&active) {
        if is_active {
            evaluate_zone(rel_dir, zone, &children, violations);
        }
    }

    for child in &children {
        if !child.is_dir || child.is_symlink {
            continue;
        }
        let ignored_by_all_active = has_active
            && zones
                .iter()
                .zip(&active)
                .filter(|&(_, &is_active)| is_active)
                .all(|(zone, _)| is_ignored(zone, &child.name));
        if ignored_by_all_active {
            continue;
        }

        let child_inherited: Vec<bool> = zones
            .iter()
            .zip(&active)
            .map(|(zone, &is_active)| {
                is_active && zone.recursive && !is_ignored(zone, &child.name)
            })
            .collect();
        let child_rel = if rel_dir == "." {
            child.name.clone()
        } else {
            format!("{rel_dir}/{}", child.name)
        };
        walk_dir(&child.abs_path, &child_rel, zones, &child_inherited, violations)?;
    }

    Ok(())
}

fn is_ignored(zone: &Zone, name: &str) -> bool {
    zone.ignore.iter().any(|pattern| pattern.is_match(name))
}

fn evaluate_zone(
    rel_dir: &str,
    zone: &Zone,
    children: &[ChildEntry],
    violations: &mut Vec<Violation>,
) {
    let non_ignored: Vec<&ChildEntry> = children
        .iter()
        .filter(|child| !is_ignored(zone, &child.name))
        .collect();

    if !zone.entries.is_empty() {
        evaluate_entry_rules(
            rel_dir,
            &zone.target.source,
            &zone.entries,
            &non_ignored,
            violations,
        );
    }

    if !zone.files.is_empty() {
        let files: Vec<&ChildEntry> = non_ignored
            .iter()
            .copied()
            .filter(|child| !child.is_dir)
            .collect();
        evaluate_entry_rules(rel_dir, &zone.target.source, &zone.files, &files, violations);
    }

    if !zone.dirs.is_empty() {
        let dirs: Vec<&ChildEntry> = non_ignored
            .iter()
            .copied()
            .filter(|child| child.is_dir)
            .collect();
        evaluate_entry_rules(rel_dir, &zone.target.source, &zone.dirs, &dirs, violations);
    }
}

fn evaluate_entry_rules(
    rel_dir: &str,
    zone_source: &str,
    rules: &EntryRules,
    entries: &[&ChildEntry],
    violations: &mut Vec<Violation>,
) {
    for req in &rules.require {
        if !entries.iter().any(|child| req.is_match(&child.name)) {
            violations.push(Violation {
                path: PathBuf::from(rel_dir),
                kind: ViolationKind::ZoneMissingRequired {
                    zone: zone_source.to_owned(),
                    pattern: req.source.clone(),
                },
            });
        }
    }

    if let Some(allow_patterns) = &rules.allow {
        for child in entries {
            if !allow_patterns.iter().any(|pat| pat.is_match(&child.name)) {
                violations.push(Violation {
                    path: child.rel_path.clone(),
                    kind: ViolationKind::ZoneDisallowed {
                        zone: zone_source.to_owned(),
                    },
                });
            }
        }
    }

    for child in entries {
        if let Some(deny_pat) = rules.deny.iter().find(|pat| pat.is_match(&child.name)) {
            violations.push(Violation {
                path: child.rel_path.clone(),
                kind: ViolationKind::ZoneDenied {
                    zone: zone_source.to_owned(),
                    pattern: deny_pat.source.clone(),
                },
            });
        }
    }
}
