use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use catasto_git::{VisibleEntry, VisibleEntryKind};

use super::{Violation, ViolationKind};
use crate::manifest::{EntryRules, Zone};

struct ChildEntry {
    name: String,
    rel_path: PathBuf,
    is_dir: bool,
}

#[derive(Default)]
struct DirNode {
    children: BTreeMap<String, NodeKind>,
}

enum NodeKind {
    /// A regular file or symbolic link (`is_dir: false`, not traversed).
    File,
    /// A git submodule or nested git repository (`is_dir: true`, not traversed).
    Submodule,
    /// A directory containing git-visible entries (`is_dir: true`, traversed).
    Dir(DirNode),
}

impl DirNode {
    fn from_visible_entries(entries: &[VisibleEntry]) -> Self {
        let mut root = Self::default();
        for entry in entries {
            let parts: Vec<&str> = entry.path.split('/').filter(|s| !s.is_empty()).collect();
            if parts.is_empty() {
                continue;
            }
            let mut cur = &mut root;
            for &dir_seg in &parts[..parts.len() - 1] {
                let node = cur
                    .children
                    .entry(dir_seg.to_owned())
                    .or_insert_with(|| NodeKind::Dir(Self::default()));
                if !matches!(node, NodeKind::Dir(_)) {
                    *node = NodeKind::Dir(Self::default());
                }
                let NodeKind::Dir(next) = node else {
                    unreachable!();
                };
                cur = next;
            }
            let leaf_name = parts[parts.len() - 1];
            let leaf_kind = match entry.kind {
                VisibleEntryKind::File | VisibleEntryKind::Symlink => NodeKind::File,
                VisibleEntryKind::Submodule => NodeKind::Submodule,
            };
            cur.children.entry(leaf_name.to_owned()).or_insert(leaf_kind);
        }
        root
    }
}

/// Evaluates all `::zone(...)` rules across the git-visible tree under `root`.
///
/// Only git-visible entries (tracked and not deleted, or untracked and not ignored)
/// and directories containing at least one such entry are evaluated. Directory symlinks
/// are treated as non-directory leaf entries, submodules are treated as directory entries
/// without descending into them, and subdirectories ignored by every active zone on their
/// parent are skipped.
pub(super) fn check_zones(root: &Path, zones: &[Zone]) -> Result<Vec<Violation>> {
    if zones.is_empty() {
        return Ok(Vec::new());
    }
    let entries = catasto_git::visible_entries(root)?;
    let tree = DirNode::from_visible_entries(&entries);
    let mut violations = Vec::new();
    let inherited = vec![false; zones.len()];
    walk_tree(&tree, ".", zones, &inherited, &mut violations);
    Ok(violations)
}

fn walk_tree(
    node: &DirNode,
    rel_dir: &str,
    zones: &[Zone],
    inherited: &[bool],
    violations: &mut Vec<Violation>,
) {
    let children: Vec<ChildEntry> = node
        .children
        .iter()
        .map(|(name, kind)| {
            let rel_path = if rel_dir == "." {
                PathBuf::from(name)
            } else {
                PathBuf::from(format!("{rel_dir}/{name}"))
            };
            let is_dir = matches!(kind, NodeKind::Dir(_) | NodeKind::Submodule);
            ChildEntry {
                name: name.clone(),
                rel_path,
                is_dir,
            }
        })
        .collect();

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

    for (name, kind) in &node.children {
        let NodeKind::Dir(child_node) = kind else {
            continue;
        };
        let ignored_by_all_active = has_active
            && zones
                .iter()
                .zip(&active)
                .filter(|&(_, &is_active)| is_active)
                .all(|(zone, _)| is_ignored(zone, name));
        if ignored_by_all_active {
            continue;
        }

        let child_inherited: Vec<bool> = zones
            .iter()
            .zip(&active)
            .map(|(zone, &is_active)| is_active && zone.recursive && !is_ignored(zone, name))
            .collect();
        let child_rel = if rel_dir == "." {
            name.clone()
        } else {
            format!("{rel_dir}/{name}")
        };
        walk_tree(
            child_node,
            &child_rel,
            zones,
            &child_inherited,
            violations,
        );
    }
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
        evaluate_entry_rules(
            rel_dir,
            &zone.target.source,
            &zone.files,
            &files,
            violations,
        );
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
