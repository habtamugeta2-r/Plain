//! Compiler diagnostics: structured, source-located, and rendered in a
//! style meant to teach rather than to scold.
//!
//! ```text
//! ── TYPE ERROR ──────────────────────────────── components/Card.pln:12:11 ──
//!
//!   `title` expects a string, but this is a number.
//!
//!   12 |     title 42
//!      |           ^^ this is a number
//!
//!   Suggestion: interpolate it into a string: title "~(42)"
//! ```

use crate::source::SourceMap;
use crate::span::Span;
use std::fmt;

/// How much a diagnostic should stop the build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "ERROR",
            Severity::Warning => "WARNING",
            Severity::Note => "NOTE",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One underlined region plus its message.
#[derive(Debug, Clone)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

/// A single diagnostic. `file` applies to every label; v0.1 diagnostics never
/// span two files.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable, documented code such as `E003` or `W101`.
    pub code: String,
    /// One-sentence summary shown in the header.
    pub title: String,
    pub file: crate::span::FileId,
    pub labels: Vec<Label>,
    /// Extra prose paragraphs.
    pub notes: Vec<String>,
    /// Concrete fix, printed last and prefixed with `Suggestion:`.
    pub suggestion: Option<String>,
}

impl Diagnostic {
    pub fn error(code: impl Into<String>, title: impl Into<String>, file: crate::span::FileId) -> Self {
        Diagnostic {
            severity: Severity::Error,
            code: code.into(),
            title: title.into(),
            file,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestion: None,
        }
    }

    pub fn warning(code: impl Into<String>, title: impl Into<String>, file: crate::span::FileId) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            code: code.into(),
            title: title.into(),
            file,
            labels: Vec::new(),
            notes: Vec::new(),
            suggestion: None,
        }
    }

    /// Builder: attach an underlined region.
    pub fn with_label(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    /// Builder: attach an explanatory paragraph.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Builder: attach a concrete fix.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// True when this diagnostic must fail the build.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Renders the diagnostic against `sources`.
    pub fn render(&self, sources: &SourceMap) -> String {
        let mut out = String::new();
        let file = match sources.get(self.file) {
            Some(f) => f,
            None => {
                out.push_str(&format!("{} {}: {}\n", self.severity, self.code, self.title));
                return out;
            }
        };

        // Header line, e.g. `── ERROR E003 ──── app.pln:12:11 ──`
        let origin = match self.labels.first() {
            Some(label) => {
                let (line, col) = file.line_col(label.span.start);
                format!("{}:{}:{}", file.name, line, col)
            }
            None => file.name.clone(),
        };
        let head = format!("{} {}", self.severity.as_str(), self.code);
        out.push_str(&rule(&head, &origin, 78));
        out.push('\n');

        out.push_str(&format!("\n  {}\n", self.title));

        // Source excerpt with carets. Only the first label is drawn inline in
        // v0.1; additional labels are listed as notes so the gutter stays simple.
        if let Some(label) = self.labels.first() {
            let (line, col) = file.line_col(label.span.start);
            let (_, end_col) = file.line_col(label.span.end);
            let text = file.line_text(line);
            let gutter = format!("{} | ", line);
            let pad = " ".repeat(gutter.chars().count());
            out.push('\n');
            out.push_str(&format!("{}{}\n", gutter, text.replace('\t', "    ")));
            let width = if end_col > col { end_col - col } else { 1 };
            let caret = format!(
                "{}{}{}",
                " ".repeat(col - 1),
                "^".repeat(width),
                if label.message.is_empty() {
                    String::new()
                } else {
                    format!(" {}", label.message)
                }
            );
            out.push_str(&format!("{}{}\n", pad, caret.replace('\t', "    ")));
        }

        for extra in self.labels.iter().skip(1) {
            let (line, col) = file.line_col(extra.span.start);
            out.push_str(&format!("\n  also at {}:{}: {}\n", line, col, extra.message));
        }

        for note in &self.notes {
            out.push_str(&format!("\n  {}\n", note));
        }

        if let Some(suggestion) = &self.suggestion {
            out.push_str(&format!("\n  Suggestion: {}\n", suggestion));
        }
        out.push('\n');
        out
    }
}

/// Builds a boxed rule line: `── left ─── right ──`, padded with `─` to `width`.
fn rule(left: &str, right: &str, width: usize) -> String {
    let body = format!(" {} ", left);
    let tail = format!(" {} ", right);
    let fill = width.saturating_sub(body.chars().count() + tail.chars().count() + 4);
    let mut s = String::from("──");
    s.push_str(&body);
    s.push_str(&"─".repeat(fill));
    s.push_str(&tail);
    s.push_str("──");
    s
}

/// Renders many diagnostics in order.
pub fn render_all(diagnostics: &[Diagnostic], sources: &SourceMap) -> String {
    let mut out = String::new();
    for d in diagnostics {
        out.push_str(&d.render(sources));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn map() -> (SourceMap, crate::span::FileId) {
        let mut m = SourceMap::new();
        let id = m.add("app.pln", "text \"hi\"\n  size 42\n");
        (m, id)
    }

    #[test]
    fn renders_header_location_and_caret() {
        let (m, id) = map();
        let d = Diagnostic::error("E003", "wrong type", id)
            .with_label(Span::new(17, 19), "this is a number")
            .with_suggestion("use size lg");
        let out = d.render(&m);
        assert!(out.contains("ERROR E003"), "{out}");
        assert!(out.contains("app.pln:2:8"), "{out}");
        assert!(out.contains("^^ this is a number"), "{out}");
        assert!(out.contains("Suggestion: use size lg"), "{out}");
    }

    #[test]
    fn rule_is_padded_to_width() {
        let line = rule("ERROR", "a.pln:1:1", 40);
        assert_eq!(line.chars().count(), 40, "{line}");
        assert!(line.starts_with("──"));
        assert!(line.ends_with("──"));
    }

    #[test]
    fn missing_file_degrades_gracefully() {
        let m = SourceMap::new();
        let d = Diagnostic::warning("W101", "hmm", crate::span::FileId(9));
        assert!(d.render(&m).contains("WARNING W101: hmm"));
    }
}
