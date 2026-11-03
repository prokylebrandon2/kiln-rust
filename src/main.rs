//! The `kiln` command-line tool.

mod cli;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    match cli::run(cli::Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
