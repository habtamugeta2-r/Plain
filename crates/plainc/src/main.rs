//! `plain` — the Plain compiler command line.
//!
//! Two commands matter in v0.1: `check`, which reads a program and says what is
//! wrong with it without writing anything, and `build`, which writes the four
//! files of a static app. Both print every diagnostic they find, because a
//! compiler that reports one mistake per run teaches people to dread running it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use plain_lang::{compile, is_plain_file, render_all, Bundle, Options, SourceMap};

/// What the command line was asked to do.
enum Command {
    Build,
    Check,
    Help,
    Version,
}

/// A parsed command line.
struct Request {
    command: Command,
    paths: Vec<String>,
    options: Options,
}

/// Usage mistakes, which are the caller's problem rather than the program's.
struct Usage(String);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let request = match parse(&args) {
        Ok(request) => request,
        Err(Usage(message)) => {
            eprintln!("plain: {message}\n");
            eprint!("{}", plain_lang::cli::USAGE);
            return ExitCode::from(2);
        }
    };

    match request.command {
        Command::Help => {
            print!("{}", plain_lang::cli::USAGE);
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("plain {}", plain_lang::version());
            ExitCode::SUCCESS
        }
        Command::Check => match run(&request, false) {
            Ok(code) => code,
            Err(Usage(message)) => fail(&message),
        },
        Command::Build => match run(&request, true) {
            Ok(code) => code,
            Err(Usage(message)) => fail(&message),
        },
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("plain: {message}");
    ExitCode::from(2)
}

/// Reads the program, reports everything wrong with it, and writes the bundle
/// when asked to and when the program is sound.
fn run(request: &Request, write: bool) -> Result<ExitCode, Usage> {
    let entry = entry_point(&request.paths)?;
    let source = match fs::read_to_string(&entry) {
        Ok(source) => source,
        Err(error) => return Err(Usage(format!("cannot read {}: {error}", entry.display()))),
    };

    let name = entry.display().to_string();
    let mut sources = SourceMap::new();
    let (bundle, diagnostics) = compile(&mut sources, &name, &source, &request.options);

    let report = render_all(&diagnostics, &sources);
    if !report.is_empty() {
        eprint!("{report}");
    }
    let errors = diagnostics.iter().filter(|d| d.is_error()).count();
    let warnings = diagnostics.len() - errors;
    if errors + warnings > 0 {
        eprintln!(
            "plain: {} and {}",
            count(errors, "error"),
            count(warnings, "warning")
        );
    }

    if errors > 0 {
        // Writing a bundle the compiler knows is broken would only produce a
        // page that fails in a way that is harder to see than this message.
        if write {
            eprintln!("plain: nothing written");
        }
        return Ok(ExitCode::FAILURE);
    }

    if write {
        emit(&bundle, &request.options.out_dir)?;
    } else {
        println!("plain: {} is sound", entry.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn count(number: usize, word: &str) -> String {
    if number == 1 {
        format!("1 {word}")
    } else {
        format!("{number} {word}s")
    }
}

/// Writes every file of a bundle, creating the directory it needs.
fn emit(bundle: &Bundle, out_dir: &str) -> Result<(), Usage> {
    let root = Path::new(out_dir);
    for (name, contents) in &bundle.files {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| Usage(format!("cannot create {}: {error}", parent.display())))?;
        }
        fs::write(&path, contents)
            .map_err(|error| Usage(format!("cannot write {}: {error}", path.display())))?;
        println!("plain: wrote {} ({})", path.display(), size(contents.len()));
    }
    Ok(())
}

/// A byte count a person can read.
fn size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} kB", bytes as f64 / 1024.0)
    }
}

/// The file to compile. A directory is searched for the conventional entry
/// point, and no argument at all means "the project I am standing in".
fn entry_point(paths: &[String]) -> Result<PathBuf, Usage> {
    const CANDIDATES: &[&str] = &["app.pln", "src/app.pln", "main.pln", "src/main.pln"];

    if paths.is_empty() {
        for candidate in CANDIDATES {
            let path = Path::new(candidate);
            if path.is_file() {
                return Ok(path.to_path_buf());
            }
        }
        return Err(Usage(format!(
            "no Plain source found, and none of {} exists here",
            CANDIDATES
                .iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if paths.len() > 1 {
        return Err(Usage(
            "v0.1 compiles one file; `import` and multi-file projects land in v0.2".to_string(),
        ));
    }
    let path = Path::new(&paths[0]);
    if path.is_dir() {
        for candidate in CANDIDATES {
            let inside = path.join(candidate);
            if inside.is_file() {
                return Ok(inside);
            }
        }
        return Err(Usage(format!(
            "{} is a directory with no {} in it",
            path.display(),
            CANDIDATES.join(" or ")
        )));
    }
    if !is_plain_file(&paths[0]) {
        return Err(Usage(format!(
            "{} is not a .{} file",
            paths[0],
            plain_lang::EXTENSION
        )));
    }
    if !path.exists() {
        return Err(Usage(format!("{} does not exist", paths[0])));
    }
    Ok(path.to_path_buf())
}

/// Reads the arguments. Anything that is not an option is a path.
fn parse(args: &[String]) -> Result<Request, Usage> {
    let mut command = None;
    let mut paths = Vec::new();
    let mut options = Options::default();
    let mut index = 0;

    while index < args.len() {
        let arg = args[index].as_str();
        index += 1;
        match arg {
            "-h" | "--help" | "help" => command = Some(Command::Help),
            "-V" | "--version" | "version" => command = Some(Command::Version),
            "build" if command.is_none() => command = Some(Command::Build),
            "check" if command.is_none() => command = Some(Command::Check),
            "-o" | "--out" => options.out_dir = value(args, &mut index, arg)?,
            "--title" => options.title = value(args, &mut index, arg)?,
            "--lang" => options.lang = value(args, &mut index, arg)?,
            "--mount" => options.mount = value(args, &mut index, arg)?,
            "--prefix" => options.class_prefix = value(args, &mut index, arg)?,
            "--minify" => options.minify = true,
            other if other.starts_with("--") => {
                return Err(Usage(format!("unknown option `{other}`")))
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(Usage(format!("unknown option `{other}`")))
            }
            other => paths.push(other.to_string()),
        }
    }

    let command = match command {
        Some(command) => command,
        // `plain app.pln` is the thing someone types first, and building is
        // what they meant.
        None if !paths.is_empty() => Command::Build,
        None => Command::Help,
    };
    if matches!(command, Command::Help | Command::Version) && !paths.is_empty() {
        return Err(Usage("a path makes no sense with `help`".to_string()));
    }
    Ok(Request {
        command,
        paths,
        options,
    })
}

/// Takes the value of an option, which may be the next argument.
fn value(args: &[String], index: &mut usize, option: &str) -> Result<String, Usage> {
    match args.get(*index) {
        Some(next) if !next.starts_with('-') => {
            *index += 1;
            Ok(next.clone())
        }
        Some(_) | None => Err(Usage(format!("`{option}` needs a value"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(request: &Request) -> Vec<&str> {
        request.paths.iter().map(String::as_str).collect()
    }

    #[test]
    fn no_arguments_at_all_prints_help() {
        let request = parse(&[]).expect("parses");
        assert!(matches!(request.command, Command::Help));
    }

    #[test]
    fn a_path_on_its_own_is_a_build() {
        let request = parse(&["app.pln".to_string()]).expect("parses");
        assert!(matches!(request.command, Command::Build));
        assert_eq!(paths(&request), vec!["app.pln"]);
        assert_eq!(request.options.out_dir, "dist");
    }

    #[test]
    fn options_are_read_and_defaulted() {
        let request = parse(&[
            "build".to_string(),
            "-o".to_string(),
            "public".to_string(),
            "--title".to_string(),
            "My page".to_string(),
            "--lang".to_string(),
            "am".to_string(),
            "--minify".to_string(),
            "app.pln".to_string(),
        ])
        .expect("parses");
        assert_eq!(request.options.out_dir, "public");
        assert_eq!(request.options.title, "My page");
        assert_eq!(request.options.lang, "am");
        assert!(request.options.minify);
        assert_eq!(paths(&request), vec!["app.pln"]);
    }

    #[test]
    fn an_option_without_a_value_is_a_usage_error() {
        let error = parse(&["build".to_string(), "--title".to_string()]).unwrap_err();
        assert!(error.0.contains("needs a value"), "{}", error.0);
    }

    #[test]
    fn an_unknown_option_is_a_usage_error() {
        let error = parse(&["build".to_string(), "--wat".to_string()]).unwrap_err();
        assert!(error.0.contains("unknown option"), "{}", error.0);
    }

    #[test]
    fn v01_compiles_one_file_and_says_so() {
        let request = parse(&["a.pln".to_string(), "b.pln".to_string()]).expect("parses");
        let error = entry_point(&request.paths).unwrap_err();
        assert!(error.0.contains("one file"), "{}", error.0);
    }

    #[test]
    fn a_missing_entry_point_is_explained() {
        let error = entry_point(&[]).unwrap_err();
        assert!(error.0.contains("no Plain source found"), "{}", error.0);

        let error = entry_point(&["notes.rs".to_string()]).unwrap_err();
        assert!(error.0.contains("not a .pln file"), "{}", error.0);
    }

    #[test]
    fn a_byte_count_is_readable() {
        assert_eq!(size(42), "42 B");
        assert_eq!(size(2048), "2.0 kB");
    }

    #[test]
    fn counts_are_pluralised_only_when_they_should_be() {
        assert_eq!(count(1, "error"), "1 error");
        assert_eq!(count(3, "error"), "3 errors");
        assert_eq!(count(0, "warning"), "0 warnings");
    }
}
