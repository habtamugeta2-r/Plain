//! `plain` — the Plain compiler command line.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("help") | Some("--help") | Some("-h") => {
            print!("{}", plain_lang::cli::USAGE);
            ExitCode::SUCCESS
        }
        Some("--version") | Some("-V") | Some("version") => {
            println!("plain {}", plain_lang::version());
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!(
                "plain: unknown command `{command}`\n\n{}",
                plain_lang::cli::USAGE
            );
            ExitCode::from(2)
        }
    }
}
