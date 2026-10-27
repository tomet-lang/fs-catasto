use std::ops::Range;

use anyhow::{Result, bail};

/// A `::name` declaration at the top of the file.
#[derive(Debug, Clone)]
pub(super) struct Declaration {
    pub(super) name: String,
    /// From the `::` to the end of its last token.
    pub(super) span: Range<usize>,
    /// The `{ ... }` with its braces, if it has one.
    pub(super) data: Option<Range<usize>>,
}

/// One `key: value` line group.
#[derive(Debug, Clone)]
pub(super) struct Entry {
    pub(super) key: String,
    /// The key and value, and the comment lines directly above them.
    pub(super) span: Range<usize>,
    /// The value only.
    pub(super) value: Range<usize>,
}

#[derive(Debug, Default)]
pub(super) struct Layout {
    pub(super) declarations: Vec<Declaration>,
    pub(super) entries: Vec<Entry>,
}

pub(super) fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

pub(super) fn splice(text: &str, range: Range<usize>, with: &str) -> String {
    format!("{}{with}{}", &text[..range.start], &text[range.end..])
}

/// Removes `span` and the whole lines it sits on, so no empty line is left.
pub(super) fn cut_lines(text: &str, span: &Range<usize>) -> String {
    let start = line_start(text, span.start);
    let end = text[span.end..]
        .find('\n')
        .map_or(text.len(), |at| span.end + at + 1);
    splice(text, start..end, "")
}

pub(super) fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

impl Layout {
    /// Where a new declaration at the top goes: before the first declaration or
    /// entry, below the comments that open the file.
    pub(super) fn first_item_start(&self, text: &str) -> usize {
        let first = self
            .declarations
            .iter()
            .map(|d| d.span.start)
            .chain(self.entries.iter().map(|e| e.span.start))
            .min();
        first.map_or(text.len(), |at| line_start(text, at))
    }
}

pub(super) fn scan(text: &str) -> Result<Layout> {
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

pub(super) fn scan_entries(text: &str, range: Range<usize>) -> Result<Vec<Entry>> {
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
                '\n' | ',' => break,
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
