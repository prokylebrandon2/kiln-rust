//! What each `kiln-gui` command does: pick the work, run it behind the progress window, and tell
//! the person how it ended.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kiln_win::ops::{self, Outcome};

use crate::progress;
use crate::shell;
use crate::w;

const TITLE: &str = "Kiln";

const EXIT_FAILED: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_CANCELLED: u8 = 3;

const USAGE: &str = "kiln-gui x|xf|t <archive>    extract here, extract to a folder, test\n\
                     kiln-gui az|ak <file or folder>    add to a .zip, add to a .kiln\n\
                     kiln-gui register|unregister    add or remove the Explorer menu";

pub fn run() -> ExitCode {
    w::dpi_aware();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["x", path] => extract(Path::new(path), false),
        ["xf", path] => extract(Path::new(path), true),
        ["t", path] => test(Path::new(path)),
        ["az", path] => add(Path::new(path), "zip"),
        ["ak", path] => add(Path::new(path), "kiln"),
        ["register"] => register(),
        ["unregister"] => unregister(),
        _ => {
            w::error(TITLE, USAGE);
            ExitCode::from(EXIT_USAGE)
        }
    }
}

/// The last part of a path, for the first line of the progress window.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Show how a job ended and turn that into the exit code.
fn finish(outcome: Outcome) -> ExitCode {
    match outcome {
        Outcome::Done(None) => ExitCode::SUCCESS,
        Outcome::Done(Some(text)) => {
            w::info(TITLE, &text);
            ExitCode::SUCCESS
        }
        Outcome::Cancelled => ExitCode::from(EXIT_CANCELLED),
        Outcome::Exists(path) => {
            w::error(TITLE, &format!("{path} already exists."));
            ExitCode::from(EXIT_FAILED)
        }
        Outcome::Failed(message) => {
            w::error(TITLE, &message);
            ExitCode::from(EXIT_FAILED)
        }
    }
}

fn extract(archive: &Path, subfolder: bool) -> ExitCode {
    let dest = ops::extract_target(archive, subfolder);
    let headline = format!("Extracting {}", name_of(archive));
    let mut outcome = run_extract(archive, &dest, false, &headline);
    // A file that would be replaced stops the job before it replaces anything; ask once.
    if let Outcome::Exists(path) = &outcome {
        let question = format!("{path} already exists.\n\nReplace existing files?");
        if w::ask(TITLE, &question) {
            outcome = run_extract(archive, &dest, true, &headline);
        }
    }
    finish(outcome)
}

fn run_extract(archive: &Path, dest: &Path, overwrite: bool, headline: &str) -> Outcome {
    let archive: PathBuf = archive.to_path_buf();
    let dest: PathBuf = dest.to_path_buf();
    let work =
        move |report: &mut dyn FnMut(u64, u64)| ops::extract(&archive, &dest, overwrite, report);
    progress::run("Extracting", headline, Box::new(work))
}

fn test(archive: &Path) -> ExitCode {
    let archive: PathBuf = archive.to_path_buf();
    let headline = format!("Testing {}", name_of(&archive));
    let work = move |report: &mut dyn FnMut(u64, u64)| ops::test(&archive, report);
    finish(progress::run("Testing", &headline, Box::new(work)))
}

fn add(input: &Path, ext: &str) -> ExitCode {
    let output = ops::archive_path_for(input, ext);
    let headline = format!("Adding to {}", name_of(&output));
    let input: PathBuf = input.to_path_buf();
    let work = move |report: &mut dyn FnMut(u64, u64)| ops::add(&output, &input, report);
    finish(progress::run("Adding", &headline, Box::new(work)))
}

fn register() -> ExitCode {
    let result = std::env::current_exe()
        .map_err(|e| format!("Cannot find kiln-gui.exe: {e}"))
        .and_then(|exe| shell::register(&exe));
    match result {
        Ok(()) => {
            w::info(TITLE, "The Kiln menu was added to Explorer.");
            ExitCode::SUCCESS
        }
        Err(message) => {
            w::error(TITLE, &message);
            ExitCode::from(EXIT_FAILED)
        }
    }
}

fn unregister() -> ExitCode {
    match shell::unregister() {
        Ok(()) => {
            w::info(TITLE, "The Kiln menu was removed from Explorer.");
            ExitCode::SUCCESS
        }
        Err(message) => {
            w::error(TITLE, &message);
            ExitCode::from(EXIT_FAILED)
        }
    }
}
