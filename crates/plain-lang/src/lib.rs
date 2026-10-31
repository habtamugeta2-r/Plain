//! The Plain language: lexer, parser, semantic analysis and code generation.
//!
//! The pipeline is deliberately boring and easy to follow:
//!
//! ```text
//! source -> lexer -> tokens -> parser -> AST -> check -> typed AST
//!        -> css::emit + js::emit -> Bundle
//! ```
//!
//! Every stage returns plain data and a list of [`Diagnostic`]s. No stage
//! panics on bad input: the lexer recovers, the parser recovers, and the
//! semantic pass reports. That is what makes `plain check` usable while you
//! type.

pub mod ast;
pub mod cli;
pub mod lexer;
pub mod parser;
pub mod runtime;
pub mod tables;

use plain_core::{Diagnostic, FileId, SourceMap};

/// How a bundle should be produced.
#[derive(Debug, Clone)]
pub struct Options {
    /// Directory the generated files are written to.
    pub out_dir: String,
    /// Value of `<html lang>`.
    pub lang: String,
    /// Document title.
    pub title: String,
    /// Prefix for generated scope classes.
    pub class_prefix: String,
    /// Emit minified CSS and JS.
    pub minify: bool,
    /// Mount selector used by the generated entry point.
    pub mount: String,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            out_dir: "dist".to_string(),
            lang: "en".to_string(),
            title: "Plain app".to_string(),
            class_prefix: "p".to_string(),
            minify: false,
            mount: "#app".to_string(),
        }
    }
}

/// Everything one compilation produced.
#[derive(Debug, Clone, Default)]
pub struct Bundle {
    /// Output file name relative to the output directory, plus contents.
    pub files: Vec<(String, String)>,
    /// Warnings that did not stop the build.
    pub diagnostics: Vec<Diagnostic>,
}

impl Bundle {
    /// Looks an output file up by name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, c)| c.as_str())
    }

    /// Adds an output file.
    pub fn push(&mut self, name: impl Into<String>, contents: impl Into<String>) {
        self.files.push((name.into(), contents.into()));
    }
}

/// Lexes `source` and reports every lexical problem rather than stopping at the
/// first one, which is what an editor wants.
pub fn lex_source(sources: &mut SourceMap, name: &str, source: &str) -> (FileId, Vec<Diagnostic>) {
    let file = sources.add_once(name, source);
    let (_, errors) = lexer::lex(file, source);
    (file, errors)
}

/// The Plain version, for diagnostics and `plain --version`.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The file extension Plain sources use.
pub const EXTENSION: &str = "pln";

/// True when `path` looks like a Plain source file.
pub fn is_plain_file(path: &str) -> bool {
    let path = path.replace('\\', "/");
    match path.rsplit_once('.') {
        Some((_, extension)) => extension.eq_ignore_ascii_case(EXTENSION),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_plain_files() {
        assert!(is_plain_file("app.pln"));
        assert!(is_plain_file("src/pages/Home.PLN"));
        assert!(is_plain_file("C:\\projects\\app.pln"));
        assert!(!is_plain_file("app.rs"));
        assert!(!is_plain_file("Makefile"));
    }

    #[test]
    fn default_options_are_sane() {
        let options = Options::default();
        assert_eq!(options.out_dir, "dist");
        assert_eq!(options.lang, "en");
        assert!(!options.minify);
    }

    #[test]
    fn lexing_reports_every_problem_not_just_the_first() {
        let mut sources = SourceMap::new();
        let (_, errors) = lex_source(&mut sources, "bad.pln", "text {a};\n<div>\n");
        assert!(
            errors.len() >= 3,
            "expected several diagnostics, got {errors:?}"
        );
    }
}
