//! Edits to the text of `catasto.ctmt` that leave the rest of the file as the
//! user wrote it.
//!
//! The manifest is written by hand, with comments and its own layout, so a
//! command that changes it cannot parse it and print it back. These functions
//! read the text only far enough to know where things are -- the `::name`
//! declarations at the top and the `"path": value` entries below them -- and
//! then insert or cut a span. Everything else stays byte for byte.
//!
//! None of them judges the result. A caller parses the new text with
//! [`crate::Manifest::parse`] before it writes anything, so an edit that makes
//! the file invalid is refused and the file is untouched.

use anyhow::{Result, anyhow, bail};
use tomet_ast::Value;
use tomet_scalar::Scalar;

mod scanner;
#[cfg(test)]
mod tests;

use scanner::{Layout, cut_lines, is_name_char, line_start, scan, scan_entries, splice};

/// Where a managed file comes from, as the text of one entry writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceText {
    /// `catasto.upstream(name, path: "...")` or `catasto.upstream(name)`
    Upstream {
        name: String,
        path: Option<String>,
        keep_symlink: bool,
        submodule: bool,
    },
    /// `catasto.git("...", rev: ..., path: "...")`
    Git {
        git: String,
        rev: String,
        path: Option<String>,
        keep_symlink: bool,
        submodule: bool,
    },
    /// `catasto.local("relative/path")`
    Local { path: String },
}

impl SourceText {
    fn keep_symlink(&self) -> bool {
        match self {
            Self::Upstream { keep_symlink, .. } | Self::Git { keep_symlink, .. } => *keep_symlink,
            Self::Local { .. } => false,
        }
    }

    fn submodule(&self) -> bool {
        match self {
            Self::Upstream { submodule, .. } | Self::Git { submodule, .. } => *submodule,
            Self::Local { .. } => false,
        }
    }
}

/// What `::upstream(...)` says about one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamText {
    pub git: String,
    pub rev: String,
    pub path: Option<String>,
}

/// The outcome of declaring an entry in the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOutcome {
    /// A new entry was added.
    Added,
    /// An existing entry's source was replaced.
    Replaced,
    /// The entry already had the given source; the manifest was unchanged.
    Unchanged,
}

/// The kind of item that was already declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredKind {
    Entry,
    Upstream,
}

/// An entry or upstream is already declared in the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlreadyDeclared {
    pub dest: String,
    pub kind: DeclaredKind,
}

impl AlreadyDeclared {
    pub fn entry(dest: impl Into<String>) -> Self {
        Self {
            dest: dest.into(),
            kind: DeclaredKind::Entry,
        }
    }

    pub fn upstream(name: impl Into<String>) -> Self {
        Self {
            dest: name.into(),
            kind: DeclaredKind::Upstream,
        }
    }
}

impl std::fmt::Display for AlreadyDeclared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            DeclaredKind::Entry => write!(
                f,
                "{} is already declared (use --force to replace its source)",
                self.dest
            ),
            DeclaredKind::Upstream => write!(
                f,
                "upstream `{}` is already declared (use --force to replace it)",
                self.dest
            ),
        }
    }
}

impl std::error::Error for AlreadyDeclared {}

/// Declares `dest` with `source`, after the last entry, or replaces its source if `force`.
pub fn add_or_replace_entry(
    text: &str,
    dest: &str,
    source: &SourceText,
    force: bool,
) -> Result<(String, AddOutcome)> {
    let layout = scan(text)?;
    if let Some(existing) = layout.entries.iter().find(|entry| entry.key == dest) {
        if !force {
            return Err(anyhow::Error::new(AlreadyDeclared::entry(dest)));
        }
        let val_span = existing.value.clone();
        let existing_text = &text[val_span.clone()];
        let existing_exec = extract_existing_executable(existing_text);
        let multiline = existing_text.starts_with('(') && existing_text.contains('\n');
        let options = RenderOptions::merged(source, existing_exec);
        let new_value = format_entry_value(source, options, multiline);

        let replaced = if existing_text == new_value {
            if has_kind_catasto(text, &layout) {
                return Ok((text.to_owned(), AddOutcome::Unchanged));
            }
            text.to_owned()
        } else {
            splice(text, val_span, &new_value)
        };
        let ensured = ensure_kind_catasto(&replaced)?;
        let outcome = if ensured == text {
            AddOutcome::Unchanged
        } else {
            AddOutcome::Replaced
        };
        return Ok((ensured, outcome));
    }

    let text_with_kind = ensure_kind_catasto(text)?;
    let layout = scan(&text_with_kind)?;
    let mut out = text_with_kind.trim_end().to_owned();
    if !out.empty_or_ends_with_newline() {
        out.push('\n');
    }
    // A blank line between the header or the comments and the first entry; none
    // between entries, which is how the files in this ecosystem are written.
    if layout.entries.is_empty() && !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&entry_text(dest, source));
    Ok((out, AddOutcome::Added))
}

/// Declares `dest` with `source`, after the last entry.
pub fn add_entry(text: &str, dest: &str, source: &SourceText) -> Result<String> {
    add_or_replace_entry(text, dest, source, false).map(|(text, _)| text)
}

trait EmptyOrNewline {
    fn empty_or_ends_with_newline(&self) -> bool;
}

impl EmptyOrNewline for str {
    fn empty_or_ends_with_newline(&self) -> bool {
        self.is_empty() || self.ends_with('\n')
    }
}

/// Undeclares `dest`: its entry and the comment lines directly above it.
pub fn remove_entry(text: &str, dest: &str) -> Result<String> {
    let layout = scan(text)?;
    let entry = layout
        .entries
        .iter()
        .find(|entry| entry.key == dest)
        .ok_or_else(|| anyhow!("{dest} is not declared"))?;
    Ok(cut_lines(text, &entry.span))
}

/// Names a place in `::upstream(...)`, creating the declaration when there is none.
pub fn add_upstream(text: &str, name: &str, upstream: &UpstreamText) -> Result<String> {
    add_or_replace_upstream(text, name, upstream, false).map(|(text, _)| text)
}

/// Names a place in `::upstream(...)`, or replaces its definition if `force`.
pub fn add_or_replace_upstream(
    text: &str,
    name: &str,
    upstream: &UpstreamText,
    force: bool,
) -> Result<(String, AddOutcome)> {
    if name.is_empty() || !name.chars().all(is_name_char) {
        bail!("an upstream name is letters, digits, `_` and `-`: {name:?}");
    }

    let initial_layout = scan(text)?;
    if let Some(decl) = initial_layout
        .declarations
        .iter()
        .find(|d| d.name == "upstream")
    {
        let Some(args) = decl.args.clone() else {
            bail!("`::upstream` has no parentheses to add to");
        };
        let inner = scan_entries(text, args.start + 1..args.end - 1)?;
        let close_line = line_start(text, args.end - 1);
        let is_multiline =
            text[close_line..args.end - 1].trim().is_empty() && close_line > args.start;
        let line = format!("  {name}: {}\n", upstream_text(upstream));

        if let Some(existing) = inner.iter().find(|entry| entry.key == name) {
            if !force {
                return Err(anyhow::Error::new(AlreadyDeclared::upstream(name)));
            }
            let replaced = if text[existing.value.clone()].trim() == upstream_text(upstream) {
                if has_kind_catasto(text, &initial_layout) {
                    return Ok((text.to_owned(), AddOutcome::Unchanged));
                }
                text.to_owned()
            } else if is_multiline {
                let start = line_start(text, existing.value.start);
                let end = text[existing.value.end..]
                    .find('\n')
                    .map_or(existing.value.end, |at| existing.value.end + at + 1);
                splice(text, start..end, &line)
            } else {
                let mut block = String::from("::upstream(\n");
                for entry in &inner {
                    if entry.key == name {
                        block.push_str(&line);
                    } else {
                        block.push_str(&format!(
                            "  {}: {}\n",
                            key_text(&entry.key),
                            text[entry.value.clone()].trim()
                        ));
                    }
                }
                block.push(')');
                splice(text, decl.span.clone(), &block)
            };
            let ensured = ensure_kind_catasto(&replaced)?;
            let outcome = if ensured == text {
                AddOutcome::Unchanged
            } else {
                AddOutcome::Replaced
            };
            return Ok((ensured, outcome));
        }

        let added = if is_multiline {
            splice(text, close_line..close_line, &line)
        } else {
            let mut block = String::from("::upstream(\n");
            for entry in &inner {
                block.push_str(&format!(
                    "  {}: {}\n",
                    key_text(&entry.key),
                    text[entry.value.clone()].trim()
                ));
            }
            block.push_str(&line);
            block.push(')');
            splice(text, decl.span.clone(), &block)
        };
        let ensured = ensure_kind_catasto(&added)?;
        return Ok((ensured, AddOutcome::Added));
    }

    let text_with_kind = ensure_kind_catasto(text)?;
    let layout = scan(&text_with_kind)?;
    let line = format!("  {name}: {}\n", upstream_text(upstream));

    // Place `::upstream(...)` directly below `::kind(catasto, ...)`.
    let kind_decl = layout
        .declarations
        .iter()
        .find(|d| d.name == "kind")
        .ok_or_else(|| anyhow!("missing `::kind(catasto)` declaration"))?;
    let after_kind = text_with_kind[kind_decl.span.end..]
        .find('\n')
        .map_or(text_with_kind.len(), |at| kind_decl.span.end + at + 1);
    let prefix = if text_with_kind[..after_kind].ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let block = format!("{prefix}::upstream(\n{line})\n");
    Ok((
        splice(&text_with_kind, after_kind..after_kind, &block),
        AddOutcome::Added,
    ))
}

/// Drops a place from `::upstream(...)`; the declaration goes with its last place.
pub fn remove_upstream(text: &str, name: &str) -> Result<String> {
    let layout = scan(text)?;
    for decl in layout.declarations.iter().filter(|d| d.name == "upstream") {
        let Some(args) = decl.args.clone() else {
            continue;
        };
        let inner = scan_entries(text, args.start + 1..args.end - 1)?;
        let Some(entry) = inner.iter().find(|entry| entry.key == name) else {
            continue;
        };
        if inner.len() == 1 {
            // The last one: remove the declaration, and any blank line after it
            // unless that blank line separates an earlier header declaration
            // from body entries.
            let mut end = decl.span.end;
            let rest = &text[end..];
            let after = rest.trim_start_matches([' ', '\t']);
            if after.starts_with('\n') {
                end += rest.len() - after.len() + 1;
            }
            let start = line_start(text, decl.span.start);
            let has_earlier_decl = layout
                .declarations
                .iter()
                .any(|other| other.span.end <= start);
            let has_following_content = !text[end..].trim().is_empty();
            if text[end..].starts_with('\n') && !(has_earlier_decl && has_following_content) {
                end += 1;
            }
            return Ok(splice(text, start..end, ""));
        }
        let close_line = line_start(text, args.end - 1);
        let is_multiline =
            text[close_line..args.end - 1].trim().is_empty() && close_line > args.start;
        if is_multiline {
            return Ok(cut_lines(text, &entry.span));
        }
        let mut block = String::from("::upstream(\n");
        for item in &inner {
            if item.key != name {
                block.push_str(&format!(
                    "  {}: {}\n",
                    key_text(&item.key),
                    text[item.value.clone()].trim()
                ));
            }
        }
        block.push(')');
        return Ok(splice(text, decl.span.clone(), &block));
    }
    bail!("upstream `{name}` is not declared");
}

/// Whether some entry of `text` names `upstream` via `catasto.upstream(...)`.
pub fn upstream_is_used(text: &str, upstream: &str) -> Result<Vec<String>> {
    let manifest = crate::Manifest::parse(text)?;
    Ok(manifest
        .managed
        .iter()
        .filter(|m| m.upstream().is_some_and(|u| u.name == upstream))
        .map(|m| m.path.to_string_lossy().into_owned())
        .collect())
}

fn has_kind_catasto(text: &str, layout: &Layout) -> bool {
    layout.declarations.iter().any(|decl| {
        if decl.name != "kind" {
            return false;
        }
        let Some(args) = &decl.args else {
            return false;
        };
        let Ok(Value::Tuple(tuple)) = tomet_ast_parser::parse_value(&text[args.clone()]) else {
            return false;
        };
        let kind = tuple.get("kind").or_else(|| tuple.nth_unnamed(0));
        matches!(kind, Some(Value::Scalar(Scalar::Text(ns))) if ns == "catasto")
    })
}

fn ensure_kind_catasto(text: &str) -> Result<String> {
    let layout = scan(text)?;
    if has_kind_catasto(text, &layout) {
        return Ok(text.to_owned());
    }
    let at = layout.first_item_start(text);
    let mut block = String::from("::kind(catasto, version: \"0.1.0\")\n");
    if at == text.len() {
        let gap = match text {
            "" => "",
            t if t.ends_with("\n\n") => "",
            t if t.ends_with('\n') => "\n",
            _ => "\n\n",
        };
        block = format!("{gap}{block}");
    } else if layout.declarations.is_empty() {
        block.push('\n');
    }
    Ok(splice(text, at..at, &block))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RenderOptions {
    executable: Option<bool>,
    keep_symlink: bool,
    submodule: bool,
}

impl RenderOptions {
    fn merged(source: &SourceText, existing_executable: Option<bool>) -> Self {
        let submodule = source.submodule();
        let keep_symlink = source.keep_symlink();
        let executable = if submodule { None } else { existing_executable };
        Self {
            executable,
            keep_symlink,
            submodule,
        }
    }
}

fn extract_existing_executable(existing_value_text: &str) -> Option<bool> {
    let Ok(Value::Tuple(tuple)) = tomet_ast_parser::parse_value(existing_value_text) else {
        return None;
    };
    match tuple.get("executable") {
        Some(Value::Scalar(Scalar::Bool(b))) => Some(*b),
        _ => None,
    }
}

fn source_call_text(source: &SourceText) -> String {
    match source {
        SourceText::Upstream { name, path, .. } => match path {
            Some(path) => format!(
                "catasto.upstream({}, path: {})",
                scalar(name),
                quoted(path)
            ),
            None => format!("catasto.upstream({})", scalar(name)),
        },
        SourceText::Git { git, rev, path, .. } => {
            let mut out = format!("catasto.git({}, rev: {}", quoted(git), scalar(rev));
            if let Some(path) = path {
                out.push_str(&format!(", path: {}", quoted(path)));
            }
            out.push(')');
            out
        }
        SourceText::Local { path } => format!("catasto.local({})", quoted(path)),
    }
}

fn format_entry_value(source: &SourceText, options: RenderOptions, multiline: bool) -> String {
    let call = source_call_text(source);
    let mut fields = Vec::new();
    if let Some(exec) = options.executable {
        fields.push(format!("executable: {exec}"));
    }
    if options.keep_symlink {
        fields.push("symlink: keep".to_owned());
    }
    if options.submodule {
        fields.push("submodule: true".to_owned());
    }
    if fields.is_empty() {
        return call;
    }
    if multiline {
        let mut items = Vec::with_capacity(1 + fields.len());
        items.push(format!("  {call}"));
        for field in fields {
            items.push(format!("  {field}"));
        }
        format!("(\n{}\n)", items.join(",\n"))
    } else {
        format!("({call}, {})", fields.join(", "))
    }
}

fn entry_text(dest: &str, source: &SourceText) -> String {
    let value = format_entry_value(source, RenderOptions::merged(source, None), false);
    format!("{}: {value}\n", quoted(dest))
}

fn upstream_text(upstream: &UpstreamText) -> String {
    let mut text = format!(
        "catasto.git({}, rev: {}",
        quoted(&upstream.git),
        scalar(&upstream.rev)
    );
    if let Some(path) = &upstream.path {
        text.push_str(&format!(", path: {}", scalar_path(path)));
    }
    text.push(')');
    text
}

/// A key, quoted unless it is a plain word.
fn key_text(key: &str) -> String {
    if !key.is_empty() && key.chars().all(is_name_char) {
        key.to_owned()
    } else {
        quoted(key)
    }
}

/// A value, bare when it reads back as the same string, quoted otherwise. A
/// number, a keyword or anything with punctuation is quoted.
fn scalar(value: &str) -> String {
    let word = value
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'));
    if word && !matches!(value, "null" | "none" | "auto" | "true" | "false") {
        value.to_owned()
    } else {
        quoted(value)
    }
}

/// Like [`scalar`], for a directory that may be several segments.
fn scalar_path(value: &str) -> String {
    scalar(value)
}

fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
