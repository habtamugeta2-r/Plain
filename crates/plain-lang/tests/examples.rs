//! Every example in `examples/` is compiled here.
//!
//! An example that does not compile is worse than no example: it is a claim
//! about the language that the language does not keep. This walks the directory
//! the documentation points at and holds each file to the standard the compiler
//! holds a real program to — no errors, and a bundle with the four files an app
//! needs.

use std::fs;
use std::path::{Path, PathBuf};

use plain_lang::{compile, has_errors, render_all, Bundle, Diagnostic, Options, SourceMap};

/// Each `examples/<name>/app.pln`, sorted so a failure is reproducible.
fn examples() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) => panic!("cannot read {}: {error}", root.display()),
    };
    let mut found: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let candidate = entry.path().join("app.pln");
        if candidate.is_file() {
            found.push(candidate);
        }
    }
    found.sort();
    assert!(
        !found.is_empty(),
        "no examples found under {}",
        root.display()
    );
    found
}

/// Compiles one example and reports anything it has to say.
fn compile_example(path: &Path) -> (Bundle, Vec<Diagnostic>, String) {
    let source = fs::read_to_string(path).expect("an example is readable");
    let mut sources = SourceMap::new();
    let (bundle, diagnostics) = compile(
        &mut sources,
        &path.display().to_string(),
        &source,
        &Options::default(),
    );
    let report = render_all(&diagnostics, &sources);
    (bundle, diagnostics, report)
}

#[test]
fn every_example_compiles_without_errors() {
    for path in examples() {
        let (_, diagnostics, report) = compile_example(&path);
        assert!(
            !has_errors(&diagnostics),
            "{} does not compile:\n{report}",
            path.display()
        );
    }
}

#[test]
fn every_example_produces_a_whole_app() {
    for path in examples() {
        let (bundle, diagnostics, report) = compile_example(&path);
        assert!(
            !has_errors(&diagnostics),
            "{} does not compile:\n{report}",
            path.display()
        );
        for file in ["index.html", "styles.css", "plain.runtime.js", "app.js"] {
            let contents = bundle
                .get(file)
                .unwrap_or_else(|| panic!("{} produced no {file}:\n{report}", path.display()));
            assert!(
                !contents.trim().is_empty(),
                "{} produced an empty {file}",
                path.display()
            );
        }

        // The document points at the module and the stylesheet it was given.
        let html = bundle.get("index.html").expect("a document");
        assert!(html.contains("app.js"), "{}", path.display());
        assert!(html.contains("styles.css"), "{}", path.display());

        // The module mounts the app it built, and imports the runtime it uses.
        let js = bundle.get("app.js").expect("a module");
        assert!(js.contains("P.mount(\"#app\", App);"), "{}", path.display());
        assert!(
            js.contains("import { P } from './plain.runtime.js';"),
            "{}",
            path.display()
        );

        // And the stylesheet carries the base reset, so one file is enough.
        let css = bundle.get("styles.css").expect("a stylesheet");
        assert!(css.contains(".pl-stack"), "{}", path.display());
    }
}

#[test]
fn has_errors_only_counts_errors() {
    let source = "main\n  heading \"One\"\n  heading \"Two\"\n";
    let mut sources = SourceMap::new();
    let (_, diagnostics) = compile(&mut sources, "app.pln", source, &Options::default());
    // Two top-level headings is worth mentioning, and is not a reason to refuse
    // to build.
    assert!(!diagnostics.is_empty(), "expected a warning");
    assert!(!has_errors(&diagnostics), "{diagnostics:?}");
}
