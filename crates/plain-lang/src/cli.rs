//! Shared command-line text, so the binary and the docs cannot drift apart.

/// The usage banner printed by `plain help`.
pub const USAGE: &str = r#"plain — the Plain compiler

Usage:
  plain build [paths...] [options]   Compile Plain sources into a bundle
  plain check [paths...]             Report errors and warnings, write nothing
  plain version                      Print the compiler version
  plain help                         Print this message

Options:
  -o, --out <dir>        Output directory (default: dist)
      --title <text>     Document title
      --lang <tag>       Value of <html lang> (default: en)
      --mount <selector> Element the app mounts into (default: #app)
      --prefix <text>    Prefix for generated class names (default: p)
      --minify           Minify the stylesheet
  -h, --help             Print this message

v0.1 compiles a single-file, client-rendered app. Routing, data fetching,
motion and multi-file imports are specified in docs/ and land in v0.2.
"#;
