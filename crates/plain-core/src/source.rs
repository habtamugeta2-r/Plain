//! Source text storage plus offset/line/column translation.

use crate::span::{FileId, Span};

/// One input file, with a precomputed line index.
#[derive(Debug, Clone)]
pub struct SourceFile {
    /// Display name, e.g. `src/app.pln`.
    pub name: String,
    /// Full text.
    pub text: String,
    /// Byte offset of the first character of every line. `line_starts[0] == 0`.
    line_starts: Vec<u32>,
}

impl SourceFile {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        let mut line_starts = vec![0u32];
        for (i, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        SourceFile {
            name: name.into(),
            text,
            line_starts,
        }
    }

    /// Number of lines (a trailing newline starts one more, empty line).
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Converts a byte offset into a 1-based `(line, column)` pair.
    ///
    /// Columns are counted in Unicode characters, not bytes, so that the caret
    /// underlines line up in a terminal that renders UTF-8.
    pub fn line_col(&self, offset: u32) -> (usize, usize) {
        let offset = offset.min(self.text.len() as u32);
        let idx = match self.line_starts.binary_search(&offset) {
            Ok(exact) => exact,
            Err(insert) => insert.saturating_sub(1),
        };
        let line_start = self.line_starts[idx] as usize;
        let slice = &self.text[line_start..offset as usize];
        (idx + 1, slice.chars().count() + 1)
    }

    /// Returns the 1-based `line`'s text without its line terminator.
    pub fn line_text(&self, line: usize) -> &str {
        if line == 0 || line > self.line_starts.len() {
            return "";
        }
        let start = self.line_starts[line - 1] as usize;
        let end = if line < self.line_starts.len() {
            self.line_starts[line] as usize
        } else {
            self.text.len()
        };
        self.text[start..end].trim_end_matches(['\r', '\n'])
    }

    /// Clamps a span into this file's bounds.
    pub fn clamp(&self, span: Span) -> Span {
        let len = self.text.len() as u32;
        Span::new(
            span.start.min(len),
            span.end.min(len).max(span.start.min(len)),
        )
    }

    /// The text covered by `span`.
    pub fn slice(&self, span: Span) -> &str {
        let span = self.clamp(span);
        self.text
            .get(span.start as usize..span.end as usize)
            .unwrap_or("")
    }
}

/// Every source file known to one compilation.
#[derive(Debug, Default, Clone)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        SourceMap { files: Vec::new() }
    }

    /// Adds a file and returns its id.
    pub fn add(&mut self, name: &str, text: &str) -> FileId {
        self.files.push(SourceFile::new(name, text));
        FileId((self.files.len() - 1) as u32)
    }

    /// Adds a file only if `name` is not present yet; returns the existing id otherwise.
    pub fn add_once(&mut self, name: &str, text: &str) -> FileId {
        if let Some(id) = self.lookup(name) {
            return id;
        }
        self.add(name, text)
    }

    pub fn get(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.0 as usize)
    }

    pub fn lookup(&self, name: &str) -> Option<FileId> {
        self.files
            .iter()
            .position(|f| f.name == name)
            .map(|i| FileId(i as u32))
    }

    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_and_columns() {
        let f = SourceFile::new("t.pln", "ab\ncde\n\nf");
        assert_eq!(f.line_count(), 4);
        assert_eq!(f.line_col(0), (1, 1));
        assert_eq!(f.line_col(2), (1, 3));
        assert_eq!(f.line_col(3), (2, 1));
        assert_eq!(f.line_col(5), (2, 3));
        assert_eq!(f.line_col(7), (3, 1));
        assert_eq!(f.line_col(8), (4, 1));
        assert_eq!(f.line_text(2), "cde");
        assert_eq!(f.line_text(3), "");
        assert_eq!(f.line_text(9), "");
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        let f = SourceFile::new("t.pln", "héllo");
        // 'é' is two bytes; the byte offset of 'l' is 3 but the column is 3.
        assert_eq!(f.line_col(3), (1, 3));
    }

    #[test]
    fn source_map_dedupes_by_name() {
        let mut m = SourceMap::new();
        let a = m.add_once("x.pln", "one");
        let b = m.add_once("x.pln", "two");
        assert_eq!(a, b);
        assert_eq!(m.get(a).unwrap().text, "one");
    }

    #[test]
    fn slice_and_clamp() {
        let f = SourceFile::new("t.pln", "hello");
        assert_eq!(f.slice(Span::new(1, 3)), "el");
        assert_eq!(f.slice(Span::new(3, 99)), "lo");
    }
}
