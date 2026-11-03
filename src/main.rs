//! The `kiln` command-line tool.

mod cli;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    match cli::run(cli::Cli::parse()) {
        Ok(cli::Outcome::Clean) => ExitCode::SUCCESS,
        Ok(cli::Outcome::Warnings) => ExitCode::from(7),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(cli::exit_code(&e))
        }
    }
}
