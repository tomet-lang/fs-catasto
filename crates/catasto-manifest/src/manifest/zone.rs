use std::collections::HashSet;
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use hole_engine::Program;
use tomet_ast::{Declaration as AstDeclaration, Value};
use tomet_scalar::Scalar;

use super::is_inside_workspace;

/// A compiled hole pattern used in `::zone(...)` directory rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZonePattern {
    /// Original pattern text written in `catasto.ctmt`.
    pub source: String,
    /// Compiled `hole-engine` bytecode program (`CompileMode::Path`).
    pub program: Program,
}

impl ZonePattern {
    /// Compiles a path or entry pattern string in `CompileMode::Path`.
    pub fn compile(source: impl Into<String>) -> Result<Self> {
        let source = source.into();
        let pattern = tomet_hole::compile_path_str(&source)
            .map_err(|e| anyhow!("invalid hole pattern `{source}`: {e}"))?;
        let program = Program::compile(&pattern);
        Ok(Self { source, program })
    }

    /// Returns `true` if `input` matches the entire pattern.
    pub fn is_match(&self, input: &str) -> bool {
        self.program.is_full_match(input)
    }
}

/// Entry matching constraints (`require`, `allow`, `deny`) shared by a zone and its `files`/`dirs` scopes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryRules {
    /// Entry patterns that must each be matched by at least one non-ignored child.
    pub require: Vec<ZonePattern>,
    /// Allowlist of entry patterns; when `Some`, every non-ignored child must match at least one.
    pub allow: Option<Vec<ZonePattern>>,
    /// Denylist of entry patterns; no non-ignored child may match any pattern.
    pub deny: Vec<ZonePattern>,
}

impl EntryRules {
    /// Returns `true` when no `require`, `allow`, or `deny` rules are configured.
    pub fn is_empty(&self) -> bool {
        self.require.is_empty() && self.allow.is_none() && self.deny.is_empty()
    }

    fn try_parse_field(
        &mut self,
        zone_target: &str,
        scope: Option<&str>,
        key: &str,
        val: &Value,
    ) -> Result<bool> {
        let field = match scope {
            Some(s) => format!("{s}.{key}"),
            None => key.to_owned(),
        };
        match key {
            "require" => self.require = parse_pattern_list(zone_target, &field, val)?,
            "allow" => self.allow = Some(parse_pattern_list(zone_target, &field, val)?),
            "deny" => self.deny = parse_pattern_list(zone_target, &field, val)?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// A directory schema rule declared in `::zone(...)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    /// Target directory path pattern (`"."` matches the workspace root).
    pub target: ZonePattern,
    /// Whether this zone recursively applies to non-ignored subdirectories.
    pub recursive: bool,
    /// Entry constraints applied to all non-ignored children (files and directories).
    pub entries: EntryRules,
    /// Entry constraints applied only to non-ignored non-directory entries.
    pub files: EntryRules,
    /// Entry constraints applied only to non-ignored directory entries.
    pub dirs: EntryRules,
    /// Patterns for child entries ignored during evaluation and recursive traversal.
    pub ignore: Vec<ZonePattern>,
}

/// Parses and validates all `::zone(...)` header declarations.
pub(super) fn parse_zone_decls(decls: &[&AstDeclaration]) -> Result<Vec<Zone>> {
    let mut zones = Vec::new();
    let mut seen_targets = HashSet::new();

    for decl in decls {
        if decl.data.value.is_some() || decl.content.is_some() {
            bail!(
                "`::zone` takes entries in parentheses, not `{{...}}`: `::zone(\"src\": (require: \"lib.rs\"))`"
            );
        }
        let Some(args) = &decl.data.args else {
            bail!("`::zone` needs named entries: `::zone(\"src\": (require: \"lib.rs\"))`");
        };
        if !args.has_no_unnamed() {
            bail!(
                "`::zone` entries must be named by target path: `::zone(\"src\": (require: \"lib.rs\"))`"
            );
        }

        for (raw_target, value) in args.named() {
            let target_str = normalize_and_validate_target(raw_target)?;
            if !seen_targets.insert(target_str.clone()) {
                bail!("zone `{target_str}` is declared twice");
            }
            let zone = parse_zone_entry(&target_str, value)?;
            zones.push(zone);
        }
    }

    Ok(zones)
}

fn normalize_and_validate_target(target: &str) -> Result<String> {
    if target.is_empty() {
        bail!("zone target path must not be empty; use `\".\"` for workspace root");
    }
    if target.starts_with('/') {
        bail!("zone target path must be relative and stay inside the workspace: `{target}`");
    }
    if target.contains("//") {
        bail!("zone target path must not contain empty segments (`//`): `{target}`");
    }
    if !is_inside_workspace(Path::new(target)) {
        bail!("zone target path must be relative and stay inside the workspace: `{target}`");
    }

    let trimmed = target.strip_suffix('/').unwrap_or(target);
    let normalized = trimmed.strip_prefix("./").unwrap_or(trimmed);
    let normalized = if normalized.is_empty() {
        "."
    } else {
        normalized
    };
    if normalized != "." && (normalized.contains("/./") || normalized.ends_with("/.")) {
        bail!("zone target path must not contain `.` segments: `{target}`");
    }
    Ok(normalized.to_owned())
}

fn parse_zone_entry(target_str: &str, value: &Value) -> Result<Zone> {
    let target =
        ZonePattern::compile(target_str).map_err(|e| anyhow!("zone `{target_str}`: {e}"))?;

    let Value::Tuple(spec) = value else {
        bail!(
            "zone `{target_str}`: expected `(require: ..., allow: ..., deny: ..., ignore: ..., recursive: ..., files: ..., dirs: ...)`"
        );
    };
    if !spec.has_no_unnamed() {
        bail!(
            "zone `{target_str}`: zone spec entries must be named (`require`, `allow`, `deny`, `ignore`, `recursive`, `files`, `dirs`)"
        );
    }

    let mut seen_fields = HashSet::new();
    let mut recursive = false;
    let mut entries = EntryRules::default();
    let mut files = EntryRules::default();
    let mut dirs = EntryRules::default();
    let mut ignore = Vec::new();

    for (key, val) in spec.named() {
        if !seen_fields.insert(key) {
            bail!("zone `{target_str}`: field `{key}` is declared twice");
        }
        if entries.try_parse_field(target_str, None, key, val)? {
            continue;
        }
        match key {
            "ignore" => ignore = parse_pattern_list(target_str, "ignore", val)?,
            "recursive" => recursive = parse_bool(target_str, "recursive", val)?,
            "files" => files = parse_entry_rules(target_str, "files", val)?,
            "dirs" => dirs = parse_entry_rules(target_str, "dirs", val)?,
            other => {
                bail!(
                    "zone `{target_str}`: unknown field `{other}`; expected `require`, `allow`, `deny`, `ignore`, `recursive`, `files`, or `dirs`"
                );
            }
        }
    }

    Ok(Zone {
        target,
        recursive,
        entries,
        files,
        dirs,
        ignore,
    })
}

fn parse_bool(zone_target: &str, field: &str, value: &Value) -> Result<bool> {
    let Value::Scalar(Scalar::Bool(b)) = value else {
        bail!("zone `{zone_target}`: `{field}` must be a boolean (`true` or `false`)");
    };
    Ok(*b)
}

fn parse_entry_rules(zone_target: &str, scope: &str, value: &Value) -> Result<EntryRules> {
    let Value::Tuple(tuple) = value else {
        bail!(
            "zone `{zone_target}`: `{scope}` must be a tuple `(require: ..., allow: ..., deny: ...)`"
        );
    };
    if !tuple.has_no_unnamed() {
        bail!("zone `{zone_target}`: `{scope}` entries must be named (`require`, `allow`, `deny`)");
    }

    let mut seen_fields = HashSet::new();
    let mut rules = EntryRules::default();
    for (key, val) in tuple.named() {
        if !seen_fields.insert(key) {
            bail!("zone `{zone_target}`: `{scope}` field `{key}` is declared twice");
        }
        if !rules.try_parse_field(zone_target, Some(scope), key, val)? {
            bail!(
                "zone `{zone_target}`: `{scope}` has unknown field `{key}`; expected `require`, `allow`, or `deny`"
            );
        }
    }
    Ok(rules)
}

fn parse_pattern_list(zone_target: &str, field: &str, value: &Value) -> Result<Vec<ZonePattern>> {
    match value {
        Value::Scalar(Scalar::Text(s)) => Ok(vec![compile_entry_pattern(zone_target, field, s)?]),
        Value::Tuple(tuple) => {
            let mut patterns = Vec::with_capacity(tuple.len());
            for entry in tuple.entries() {
                if entry.name().is_some() {
                    bail!(
                        "zone `{zone_target}`: `{field}` entries must be unnamed pattern strings"
                    );
                }
                let Value::Scalar(Scalar::Text(s)) = entry.value() else {
                    bail!("zone `{zone_target}`: `{field}` entries must be pattern strings");
                };
                patterns.push(compile_entry_pattern(zone_target, field, s)?);
            }
            Ok(patterns)
        }
        _ => bail!(
            "zone `{zone_target}`: `{field}` must be a pattern string or a tuple of pattern strings"
        ),
    }
}

fn compile_entry_pattern(zone_target: &str, field: &str, pattern: &str) -> Result<ZonePattern> {
    if pattern.is_empty() {
        bail!("zone `{zone_target}`: `{field}` pattern must not be empty");
    }
    ZonePattern::compile(pattern)
        .map_err(|e| anyhow!("zone `{zone_target}`: invalid `{field}` {e}"))
}
