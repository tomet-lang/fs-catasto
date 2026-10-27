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

mod scanner;
#[cfg(test)]
mod tests;

use scanner::{cut_lines, is_name_char, line_start, scan, scan_entries, splice};

/// Where a managed file comes from, as the text of one entry writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceText {
    /// `{ upstream: name, path: "..." }`
    Upstream {
        name: String,
        path: String,
        keep_symlink: bool,
    },
    /// `{ git: "...", rev: ..., path: "..." }`
    Git {
        git: String,
        rev: String,
        path: String,
        keep_symlink: bool,
    },
    /// `"relative/path"`
    Local { path: String },
}

/// What `::upstreams` says about one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamText {
    pub git: String,
    pub rev: String,
    pub root: Option<String>,
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

/// An entry is already declared in the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlreadyDeclared {
    pub dest: String,
}

impl std::fmt::Display for AlreadyDeclared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is already declared (use --force to replace its source)",
            self.dest
        )
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
            return Err(anyhow::Error::new(AlreadyDeclared {
                dest: dest.to_owned(),
            }));
        }
        let val_span = existing.value.clone();
        let value_str = &text[val_span.clone()];
        let open_brace = value_str
            .find('{')
            .ok_or_else(|| anyhow!("malformed entry for {dest}: missing `{{`"))?;
        let close_brace = value_str
            .rfind('}')
            .ok_or_else(|| anyhow!("malformed entry for {dest}: missing `}}`"))?;
        let inner_start = val_span.start + open_brace + 1;
        let inner_end = val_span.start + close_brace;
        let fields = scan_entries(text, inner_start..inner_end)?;
        let source_field = fields
            .iter()
            .find(|f| f.key == "source")
            .ok_or_else(|| anyhow!("entry {dest} has no `source` field to replace"))?;

        let new_source = source_value_text(source);
        if text[source_field.value.clone()] == new_source {
            return Ok((text.to_owned(), AddOutcome::Unchanged));
        }
        let new_text = splice(text, source_field.value.clone(), &new_source);
        return Ok((new_text, AddOutcome::Replaced));
    }

    let mut out = text.trim_end().to_owned();
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

/// Names a place in `::upstreams`, creating the declaration when there is none.
pub fn add_upstream(text: &str, name: &str, upstream: &UpstreamText) -> Result<String> {
    if name.is_empty() || !name.chars().all(is_name_char) {
        bail!("an upstream name is letters, digits, `_` and `-`: {name:?}");
    }
    let layout = scan(text)?;
    let line = format!("  {name}: {}\n", upstream_text(upstream));

    let Some(decl) = layout.declarations.iter().find(|d| d.name == "upstreams") else {
        let at = layout.first_item_start(text);
        let mut block = format!("::upstreams{{\n{line}}}\n");
        if at == text.len() {
            // Nothing but comments (or nothing) yet: the block goes after them.
            let gap = match text {
                "" => "",
                t if t.ends_with("\n\n") => "",
                t if t.ends_with('\n') => "\n",
                _ => "\n\n",
            };
            block = format!("{gap}{block}");
        } else {
            block.push('\n');
        }
        return Ok(splice(text, at..at, &block));
    };
    let Some(data) = decl.data.clone() else {
        bail!("`::upstreams` has no map to add to");
    };
    let inner = scan_entries(text, data.start + 1..data.end - 1)?;
    if inner.iter().any(|entry| entry.key == name) {
        bail!("upstream `{name}` is already declared");
    }

    // A closing brace on a line of its own: add a line above it and leave the rest.
    let close_line = line_start(text, data.end - 1);
    if text[close_line..data.end - 1].trim().is_empty() && close_line > data.start {
        return Ok(splice(text, close_line..close_line, &line));
    }

    // A block written on one line: write it out in full.
    let mut block = String::from("::upstreams{\n");
    for entry in &inner {
        block.push_str(&format!(
            "  {}: {}\n",
            key_text(&entry.key),
            text[entry.value.clone()].trim()
        ));
    }
    block.push_str(&line);
    block.push('}');
    Ok(splice(text, decl.span.clone(), &block))
}

/// Drops a place from `::upstreams`; the declaration goes with its last place.
pub fn remove_upstream(text: &str, name: &str) -> Result<String> {
    let layout = scan(text)?;
    let decl = layout
        .declarations
        .iter()
        .find(|d| d.name == "upstreams")
        .ok_or_else(|| anyhow!("upstream `{name}` is not declared"))?;
    let data = decl
        .data
        .clone()
        .ok_or_else(|| anyhow!("upstream `{name}` is not declared"))?;
    let inner = scan_entries(text, data.start + 1..data.end - 1)?;
    let Some(entry) = inner.iter().find(|entry| entry.key == name) else {
        bail!("upstream `{name}` is not declared");
    };
    if inner.len() == 1 {
        // The last one: the whole declaration, and the blank line after it.
        let mut end = decl.span.end;
        let rest = &text[end..];
        let after = rest.trim_start_matches([' ', '\t']);
        if after.starts_with('\n') {
            end += rest.len() - after.len() + 1;
        }
        let rest = &text[end..];
        if rest.starts_with('\n') {
            end += 1;
        }
        let start = line_start(text, decl.span.start);
        return Ok(splice(text, start..end, ""));
    }
    Ok(cut_lines(text, &entry.span))
}

/// Whether some entry of `text` names `upstream` as its source.
pub fn upstream_is_used(text: &str, upstream: &str) -> Result<Vec<String>> {
    let layout = scan(text)?;
    let mut users = Vec::new();
    for entry in &layout.entries {
        let value = &text[entry.value.clone()];
        let mut rest = value;
        while let Some(at) = rest.find("upstream") {
            let after = rest[at + "upstream".len()..].trim_start();
            if let Some(after) = after.strip_prefix(':') {
                let after = after.trim_start().trim_start_matches('"');
                let name: String = after.chars().take_while(|c| is_name_char(*c)).collect();
                if name == upstream {
                    users.push(entry.key.clone());
                    break;
                }
            }
            rest = &rest[at + "upstream".len()..];
        }
    }
    Ok(users)
}

fn source_value_text(source: &SourceText) -> String {
    match source {
        SourceText::Upstream {
            name,
            path,
            keep_symlink,
        } => {
            let symlink = if *keep_symlink { ", symlink: keep" } else { "" };
            format!(
                "{{ upstream: {}, path: {}{symlink} }}",
                scalar(name),
                quoted(path)
            )
        }
        SourceText::Git {
            git,
            rev,
            path,
            keep_symlink,
        } => {
            let symlink = if *keep_symlink { ", symlink: keep" } else { "" };
            format!(
                "{{ git: {}, rev: {}, path: {}{symlink} }}",
                quoted(git),
                scalar(rev),
                quoted(path)
            )
        }
        SourceText::Local { path } => quoted(path),
    }
}

fn entry_text(dest: &str, source: &SourceText) -> String {
    let source = source_value_text(source);
    format!("{}: {{\n  source: {source}\n}}\n", quoted(dest))
}

fn upstream_text(upstream: &UpstreamText) -> String {
    let mut text = format!(
        "{{ git: {}, rev: {}",
        quoted(&upstream.git),
        scalar(&upstream.rev)
    );
    if let Some(root) = &upstream.root {
        text.push_str(&format!(", root: {}", scalar_path(root)));
    }
    text.push_str(" }");
    text
}

/// A map key, quoted unless it is a plain word.
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
    if word && !matches!(value, "null" | "true" | "false") {
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
