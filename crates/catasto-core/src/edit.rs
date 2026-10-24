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

use std::ops::Range;

use anyhow::{Result, anyhow, bail};

/// Where a managed file comes from, as the text of one entry writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceText {
    /// `{ upstream: name, path: "..." }`
    Upstream { name: String, path: String },
    /// `{ git: "...", rev: ..., path: "..." }`
    Git {
        git: String,
        rev: String,
        path: String,
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

/// A `::name` declaration at the top of the file.
#[derive(Debug, Clone)]
struct Declaration {
    name: String,
    /// From the `::` to the end of its last token.
    span: Range<usize>,
    /// The `{ ... }` with its braces, if it has one.
    data: Option<Range<usize>>,
}

/// One `key: value` line group.
#[derive(Debug, Clone)]
struct Entry {
    key: String,
    /// The key and value, and the comment lines directly above them.
    span: Range<usize>,
    /// The value only.
    value: Range<usize>,
}

#[derive(Debug, Default)]
struct Layout {
    declarations: Vec<Declaration>,
    entries: Vec<Entry>,
}

/// Declares `dest` with `source`, after the last entry.
pub fn add_entry(text: &str, dest: &str, source: &SourceText) -> Result<String> {
    let layout = scan(text)?;
    if layout.entries.iter().any(|entry| entry.key == dest) {
        bail!("{dest} is already declared");
    }
    let mut out = text.trim_end().to_owned();
    if !out.is_empty() {
        out.push('\n');
    }
    // A blank line between the header or the comments and the first entry; none
    // between entries, which is how the files in this ecosystem are written.
    if layout.entries.is_empty() && !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&entry_text(dest, source));
    Ok(out)
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

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

// ---- text of the things this module writes ---------------------------------

fn entry_text(dest: &str, source: &SourceText) -> String {
    let source = match source {
        SourceText::Upstream { name, path } => {
            format!("{{ upstream: {}, path: {} }}", scalar(name), quoted(path))
        }
        SourceText::Git { git, rev, path } => format!(
            "{{ git: {}, rev: {}, path: {} }}",
            quoted(git),
            scalar(rev),
            quoted(path)
        ),
        SourceText::Local { path } => quoted(path),
    };
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

// ---- cutting and pasting ---------------------------------------------------

fn splice(text: &str, range: Range<usize>, with: &str) -> String {
    format!("{}{with}{}", &text[..range.start], &text[range.end..])
}

/// Removes `span` and the whole lines it sits on, so no empty line is left.
fn cut_lines(text: &str, span: &Range<usize>) -> String {
    let start = line_start(text, span.start);
    let end = text[span.end..]
        .find('\n')
        .map_or(text.len(), |at| span.end + at + 1);
    splice(text, start..end, "")
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

// ---- reading where things are ----------------------------------------------

impl Layout {
    /// Where a new declaration at the top goes: before the first declaration or
    /// entry, below the comments that open the file.
    fn first_item_start(&self, text: &str) -> usize {
        let first = self
            .declarations
            .iter()
            .map(|d| d.span.start)
            .chain(self.entries.iter().map(|e| e.span.start))
            .min();
        first.map_or(text.len(), |at| line_start(text, at))
    }
}

fn scan(text: &str) -> Result<Layout> {
    let mut cursor = Cursor::new(text, 0..text.len());
    let mut layout = Layout::default();

    // The declarations at the top.
    loop {
        let before = cursor.at;
        cursor.skip_blank();
        if !cursor.rest().starts_with("::") {
            // What was skipped may be the comment lines of the first entry.
            cursor.at = before;
            break;
        }
        let start = cursor.at;
        cursor.at += 2;
        let name = cursor.take_while(is_name_char).to_owned();
        let mut data = None;
        loop {
            match cursor.peek() {
                Some('(') => cursor.skip_balanced()?,
                Some('{') => {
                    let from = cursor.at;
                    cursor.skip_balanced()?;
                    data = Some(from..cursor.at);
                }
                _ => break,
            }
        }
        layout.declarations.push(Declaration {
            name,
            span: start..cursor.at,
            data,
        });
    }

    layout.entries = scan_entries_from(text, cursor.at..text.len())?;
    Ok(layout)
}

fn scan_entries(text: &str, range: Range<usize>) -> Result<Vec<Entry>> {
    scan_entries_from(text, range)
}

fn scan_entries_from(text: &str, range: Range<usize>) -> Result<Vec<Entry>> {
    let floor = range.start;
    let mut cursor = Cursor::new(text, range);
    let mut entries = Vec::new();
    loop {
        cursor.skip_blank();
        if cursor.at >= cursor.end {
            break;
        }
        let key_start = cursor.at;
        let key = match cursor.peek() {
            Some('"') => cursor.string()?,
            _ => cursor
                .take_while(|c| !matches!(c, ':' | '\n' | ' ' | '\t' | ',' | '{' | '}'))
                .to_owned(),
        };
        if key.is_empty() {
            bail!("expected a key at byte {}", cursor.at);
        }
        cursor.skip_inline();
        if cursor.peek() != Some(':') {
            bail!("expected `:` after `{key}`");
        }
        cursor.at += 1;
        cursor.skip_inline();
        // A value that starts on the next line.
        if matches!(cursor.peek(), Some('\n') | None) {
            cursor.skip_blank();
        }
        let value_start = cursor.at;
        let value_end = cursor.value_to_eol()?;
        // A comma between entries written on one line is not part of the value.
        let mut value_end = value_end;
        while text[value_start..value_end].ends_with(',') {
            value_end -= 1;
        }
        entries.push(Entry {
            key,
            span: attach_comments(text, floor, key_start)..value_end,
            value: value_start..value_end,
        });
    }
    Ok(entries)
}

/// The start of the `//` lines directly above `at` (no blank line between), but
/// not when they open the file: those describe the file.
fn attach_comments(text: &str, floor: usize, at: usize) -> usize {
    let mut start = line_start(text, at);
    // Only what is alone on its line is attached; an entry after another on one
    // line has no lines above it that are its own.
    if !text[start..at].trim().is_empty() {
        return at;
    }
    while start > floor {
        let previous = line_start(text, start - 1);
        let line = text[previous..start - 1].trim();
        if !line.starts_with("//") {
            break;
        }
        if previous == 0 {
            // These open the file.
            break;
        }
        start = previous;
    }
    start
}

struct Cursor<'a> {
    text: &'a str,
    at: usize,
    end: usize,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str, range: Range<usize>) -> Self {
        Self {
            text,
            at: range.start,
            end: range.end,
        }
    }

    fn rest(&self) -> &'a str {
        &self.text[self.at..self.end]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn take_while(&mut self, f: impl Fn(char) -> bool) -> &'a str {
        let rest = self.rest();
        let len = rest.find(|c| !f(c)).unwrap_or(rest.len());
        self.at += len;
        &rest[..len]
    }

    fn skip_inline(&mut self) {
        self.take_while(|c| c == ' ' || c == '\t');
    }

    /// Spaces, newlines, commas between entries, and `//` comments.
    fn skip_blank(&mut self) {
        loop {
            self.take_while(|c| c.is_whitespace() || c == ',');
            if self.rest().starts_with("//") {
                self.skip_comment();
            } else {
                break;
            }
        }
    }

    fn skip_comment(&mut self) {
        let rest = self.rest();
        self.at += rest.find('\n').unwrap_or(rest.len());
    }

    /// A `"..."` string; returns its content with the escapes resolved.
    fn string(&mut self) -> Result<String> {
        debug_assert_eq!(self.peek(), Some('"'));
        self.at += 1;
        let mut out = String::new();
        let mut chars = self.rest().char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    self.at += i + 1;
                    return Ok(out);
                }
                '\\' => match chars.next() {
                    Some((_, 'n')) => out.push('\n'),
                    Some((_, 't')) => out.push('\t'),
                    Some((_, 'r')) => out.push('\r'),
                    Some((_, other)) => out.push(other),
                    None => break,
                },
                c => out.push(c),
            }
        }
        bail!("a string is not closed");
    }

    /// Past the group that opens at the cursor, however it nests.
    fn skip_balanced(&mut self) -> Result<()> {
        let mut stack = Vec::new();
        while let Some(c) = self.peek() {
            match c {
                '"' => {
                    self.string()?;
                    continue;
                }
                '/' if self.rest().starts_with("//") => {
                    self.skip_comment();
                    continue;
                }
                '{' => stack.push('}'),
                '[' => stack.push(']'),
                '(' => stack.push(')'),
                '}' | ']' | ')' => {
                    if stack.pop() != Some(c) {
                        bail!("`{c}` does not match what it closes");
                    }
                    self.at += 1;
                    if stack.is_empty() {
                        return Ok(());
                    }
                    continue;
                }
                _ => {}
            }
            self.at += c.len_utf8();
        }
        bail!("a group is not closed")
    }

    /// To the end of the value that starts here: the end of its line, once every
    /// group on it is closed. Returns where the value's last token ends.
    fn value_to_eol(&mut self) -> Result<usize> {
        let mut last = self.at;
        while let Some(c) = self.peek() {
            match c {
                '\n' => break,
                '"' => {
                    self.string()?;
                    last = self.at;
                }
                '{' | '[' | '(' => {
                    self.skip_balanced()?;
                    last = self.at;
                }
                '/' if self.rest().starts_with("//") => break,
                c if c.is_whitespace() => self.at += c.len_utf8(),
                c => {
                    self.at += c.len_utf8();
                    last = self.at;
                }
            }
        }
        Ok(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Manifest;

    const VOCAB: &str = "\
// The files catasto manages in this workspace.
::upstreams{
  vocabularies: { git: \"https://example.com/v.git\", rev: main, root: vocabularies }
}

// the rust one
\"schema/rust.tmt\": {
  source: { upstream: vocabularies, path: \"rust/rust.tmt\" }
}
\"schema/writ.tmt\": {
  source: { upstream: vocabularies, path: \"writ/writ.tmt\" }
}
";

    fn up(name: &str, path: &str) -> SourceText {
        SourceText::Upstream {
            name: name.into(),
            path: path.into(),
        }
    }

    #[test]
    fn scanning_finds_the_declaration_and_the_entries() {
        let layout = scan(VOCAB).unwrap();
        assert_eq!(layout.declarations.len(), 1);
        assert_eq!(layout.declarations[0].name, "upstreams");
        let keys: Vec<_> = layout.entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["schema/rust.tmt", "schema/writ.tmt"]);
    }

    #[test]
    fn an_entry_is_added_after_the_last_and_the_rest_is_untouched() {
        let text = add_entry(VOCAB, "schema/ui.tmt", &up("vocabularies", "ui/ui.tmt")).unwrap();
        assert!(text.starts_with(VOCAB), "{text}");
        assert!(text.ends_with(
            "\"schema/ui.tmt\": {\n  source: { upstream: vocabularies, path: \"ui/ui.tmt\" }\n}\n"
        ));
        assert!(Manifest::parse(&text).is_ok());
    }

    #[test]
    fn the_first_entry_goes_below_a_blank_line() {
        let text = add_entry(
            "// nothing yet\n",
            "a.tmt",
            &SourceText::Local {
                path: "../a.tmt".into(),
            },
        )
        .unwrap();
        assert_eq!(
            text,
            "// nothing yet\n\n\"a.tmt\": {\n  source: \"../a.tmt\"\n}\n"
        );
        assert!(Manifest::parse(&text).is_ok());
    }

    #[test]
    fn an_entry_is_not_added_twice() {
        let err = add_entry(VOCAB, "schema/rust.tmt", &up("v", "p")).unwrap_err();
        assert!(err.to_string().contains("already declared"), "{err}");
    }

    #[test]
    fn a_removed_entry_takes_its_comment_lines_and_nothing_else() {
        let text = remove_entry(VOCAB, "schema/rust.tmt").unwrap();
        assert!(!text.contains("rust"), "{text}");
        assert!(!text.contains("// the rust one"), "{text}");
        assert!(text.contains("// The files catasto manages"));
        assert!(text.contains("schema/writ.tmt"));
        assert!(Manifest::parse(&text).is_ok());
    }

    #[test]
    fn the_comment_that_opens_the_file_is_not_an_entrys() {
        let text = "// about this file\n\"a.tmt\": { source: \"x\" }\n";
        assert_eq!(remove_entry(text, "a.tmt").unwrap(), "// about this file\n");
    }

    #[test]
    fn removing_what_is_not_there_says_so() {
        let err = remove_entry(VOCAB, "nope.tmt").unwrap_err();
        assert!(err.to_string().contains("not declared"), "{err}");
    }

    #[test]
    fn a_value_with_braces_and_strings_inside_is_read_whole() {
        let text = "\"a\": {\n  source: { git: \"https://x/}.git\", rev: main, path: \"p // q\" } // trailing\n}\n\"b\": { source: \"y\" }\n";
        let layout = scan(text).unwrap();
        assert_eq!(layout.entries.len(), 2);
        assert_eq!(layout.entries[1].key, "b");
        let after = remove_entry(text, "a").unwrap();
        assert_eq!(after, "\"b\": { source: \"y\" }\n");
    }

    #[test]
    fn an_upstream_goes_above_the_closing_brace_of_a_block_on_lines_of_its_own() {
        let text = add_upstream(
            VOCAB,
            "shared",
            &UpstreamText {
                git: "https://example.com/s.git".into(),
                rev: "v1".into(),
                root: None,
            },
        )
        .unwrap();
        assert!(text.contains(
            "  vocabularies: { git: \"https://example.com/v.git\", rev: main, root: vocabularies }\n  shared: { git: \"https://example.com/s.git\", rev: v1 }\n}\n"
        ), "{text}");
        assert!(Manifest::parse(&text).is_ok());
    }

    #[test]
    fn a_block_on_one_line_is_written_out_when_it_gains_a_place() {
        let one = "::upstreams{ a: { git: \"https://x/a.git\", rev: main } }\n\"f\": { source: { upstream: a, path: \"p\" } }\n";
        let text = add_upstream(
            one,
            "b",
            &UpstreamText {
                git: "https://x/b.git".into(),
                rev: "main".into(),
                root: Some("dir/sub".into()),
            },
        )
        .unwrap();
        assert!(
            text.starts_with(
                "::upstreams{\n  a: { git: \"https://x/a.git\", rev: main }\n  b: { git: \"https://x/b.git\", rev: main, root: dir/sub }\n}\n"
            ),
            "{text}"
        );
        assert!(Manifest::parse(&text).is_ok(), "{text}");
    }

    #[test]
    fn the_first_upstream_creates_the_declaration_above_the_entries() {
        let text = add_upstream(
            "// header\n\n\"f\": { source: \"x\" }\n",
            "v",
            &UpstreamText {
                git: "https://x/v.git".into(),
                rev: "main".into(),
                root: None,
            },
        )
        .unwrap();
        assert!(text.starts_with(
            "// header\n\n::upstreams{\n  v: { git: \"https://x/v.git\", rev: main }\n}\n\n\"f\""
        ), "{text}");
        assert!(Manifest::parse(&text).is_ok());
    }

    #[test]
    fn a_rev_that_reads_as_a_number_is_quoted() {
        let text = add_upstream(
            "",
            "v",
            &UpstreamText {
                git: "https://x/v.git".into(),
                rev: "1.2".into(),
                root: None,
            },
        )
        .unwrap();
        assert!(text.contains("rev: \"1.2\""), "{text}");
        assert!(Manifest::parse(&text).is_ok());
    }

    #[test]
    fn an_upstream_is_removed_and_the_declaration_goes_with_the_last() {
        let two = add_upstream(
            VOCAB,
            "shared",
            &UpstreamText {
                git: "https://example.com/s.git".into(),
                rev: "main".into(),
                root: None,
            },
        )
        .unwrap();
        let one = remove_upstream(&two, "shared").unwrap();
        assert_eq!(one, VOCAB);

        let none = remove_upstream("::upstreams{\n  a: { git: \"https://x/a.git\", rev: main }\n}\n\n\"f\": { source: \"x\" }\n", "a").unwrap();
        assert_eq!(none, "\"f\": { source: \"x\" }\n");
    }

    #[test]
    fn who_uses_an_upstream_is_found_in_the_entries() {
        assert_eq!(
            upstream_is_used(VOCAB, "vocabularies").unwrap(),
            ["schema/rust.tmt", "schema/writ.tmt"]
        );
        assert!(upstream_is_used(VOCAB, "other").unwrap().is_empty());
    }

    #[test]
    fn a_name_with_punctuation_is_refused() {
        let err = add_upstream(
            "",
            "a b",
            &UpstreamText {
                git: "https://x".into(),
                rev: "main".into(),
                root: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("upstream name"), "{err}");
    }
}
