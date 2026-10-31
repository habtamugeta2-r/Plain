//! The Plain language: lexer, parser, semantic analysis and code generation.
//!
//! The pipeline is deliberately boring and easy to follow:
//!
//! ```text
//! source -> lexer -> tokens -> parser -> AST -> check -> css::collect
//!        -> js::module -> Bundle
//! ```
//!
//! Every stage returns plain data and a list of [`Diagnostic`]s. No stage
//! panics on bad input: the lexer recovers, the parser recovers, and the
//! semantic pass reports. That is what makes `plain check` usable while you
//! type.

pub mod ast;
pub mod check;
pub mod cli;
pub mod css;
pub mod expr;
pub mod js;
pub mod lexer;
pub mod parser;
pub mod runtime;
pub mod tables;

pub use plain_core::{render_all, Diagnostic, FileId, SourceMap, Span};

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
    /// Emit minified CSS. JavaScript is not minified in v0.1: it is already
    /// small, and readable output is worth more than a few bytes until there is
    /// a bundler to shrink it properly.
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

/// The anchor the skip link points at, which the first `main` in a program
/// gets automatically.
pub const MAIN_ID: &str = "plain-main";

/// Compiles one Plain source into the files of a static app.
///
/// Every stage runs even after an earlier one reports an error, because a
/// writer fixing ten mistakes wants to see ten mistakes. The bundle is returned
/// either way; the caller decides whether writing it out would be honest by
/// looking for errors in the diagnostics.
///
/// ```
/// use plain_lang::{compile, Options, SourceMap};
///
/// let mut sources = SourceMap::new();
/// let (bundle, diagnostics) = compile(
///     &mut sources,
///     "app.pln",
///     "main\n  heading \"Hello\"\n",
///     &Options::default(),
/// );
/// assert!(diagnostics.is_empty(), "{diagnostics:?}");
/// assert!(bundle.get("index.html").unwrap().contains("id=\"app\""));
/// assert!(bundle.get("app.js").unwrap().contains("P.el('main'"));
/// assert!(bundle.get("styles.css").unwrap().contains(".pl-stack"));
/// ```
pub fn compile(
    sources: &mut SourceMap,
    name: &str,
    source: &str,
    options: &Options,
) -> (Bundle, Vec<Diagnostic>) {
    let (file, mut diagnostics) = lex_source(sources, name, source);
    let (mut program, parse_errors) = parser::parse_source(file, source);
    diagnostics.extend(parse_errors);

    // Checking runs before the styles, because it is what decides which values
    // are not known until the program runs.
    diagnostics.extend(check::check(&mut program));

    let styles = css::collect(&mut program, &options.class_prefix);
    diagnostics.extend(styles.diagnostics.clone());

    let (module, module_diagnostics) = js::module(&program, &options.mount);
    diagnostics.extend(module_diagnostics);

    let mut bundle = Bundle::default();
    bundle.push("index.html", document(&program, options));
    bundle.push("styles.css", styles.stylesheet(options.minify));
    bundle.push(runtime::RUNTIME_FILE, runtime::RUNTIME);
    bundle.push("app.js", module);
    (bundle, diagnostics)
}

/// Compiles and renders every diagnostic the way a terminal wants it. Empty
/// when nothing is worth saying.
pub fn compile_to_text(
    sources: &mut SourceMap,
    name: &str,
    source: &str,
    options: &Options,
) -> (Bundle, String) {
    let (bundle, diagnostics) = compile(sources, name, source, options);
    (bundle, render_all(&diagnostics, sources))
}

/// True when any diagnostic would stop a build.
pub fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(Diagnostic::is_error)
}

/// The HTML document the generated module is mounted into.
///
/// Plain renders client-side, so the document is a shell: a skip link, the mount
/// point and the module. The skip link is only written when the program has a
/// `main` to skip to, because a link to an anchor that does not exist is worse
/// than no link.
fn document(program: &ast::Program, options: &Options) -> String {
    let skip = if has_main(&program.body) {
        format!(
            "    <a class=\"pl-skip\" href=\"#{MAIN_ID}\">Skip to content</a>\n"
        )
    } else {
        String::new()
    };
    format!(
        "<!doctype html>
<html lang=\"{}\">
  <head>
    <meta charset=\"utf-8\">
    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">
    <title>{}</title>
    <link rel=\"stylesheet\" href=\"styles.css\">
  </head>
  <body>
{}    <div id=\"app\"></div>
    <script type=\"module\" src=\"app.js\"></script>
  </body>
</html>
",
        escape(&options.lang),
        escape(&options.title),
        skip
    )
}

/// Escapes text that is going into HTML rather than into a text node.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

fn has_main(nodes: &[ast::Node]) -> bool {
    nodes.iter().any(|node| match node {
        ast::Node::Element(element) => {
            element.def.tag == "main" || has_main(&element.children)
        }
        ast::Node::Use(invocation) => has_main(&invocation.children),
        ast::Node::If(condition) => {
            condition.branches.iter().any(|(_, body)| has_main(body))
                || condition
                    .else_branch
                    .as_ref()
                    .is_some_and(|body| has_main(body))
        }
        ast::Node::Each(each) => has_main(&each.body),
        _ => false,
    })
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
