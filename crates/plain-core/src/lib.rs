//! `plain-core` holds the compiler plumbing that every other Plain crate needs
//! and that has no language knowledge: source storage, byte spans, and
//! diagnostics.
//!
//! Keeping it separate means the lexer, the language server and the formatter
//! can all report problems the same way.

#![deny(missing_docs)]

pub mod diagnostic;
pub mod source;
pub mod span;

pub use diagnostic::{render_all, Diagnostic, Label, Severity};
pub use source::{SourceFile, SourceMap};
pub use span::{FileId, Span};

/// Everything one compilation produced that is not an error.
#[derive(Debug, Default, Clone)]
pub struct Emitted {
    /// File name relative to the output directory, plus its contents.
    pub files: Vec<(String, String)>,
}

impl Emitted {
    pub fn new() -> Self {
        Emitted { files: Vec::new() }
    }

    /// Records one output file, replacing an earlier file with the same name.
    pub fn push(&mut self, name: impl Into<String>, contents: impl Into<String>) {
        let name = name.into();
        if let Some(slot) = self.files.iter_mut().find(|(n, _)| *n == name) {
            slot.1 = contents.into();
        } else {
            self.files.push((name, contents.into()));
        }
    }

    /// Looks an output file up by name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, c)| c.as_str())
    }
}
