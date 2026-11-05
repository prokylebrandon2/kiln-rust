//! "If Kiln ever stops with an error, the details are kept and shown on the next start."
//!
//! The Java app wrote `files/last-crash.txt` from its uncaught-exception handler and showed the
//! first 2500 bytes in a sheet next time. This is the same, from a panic hook. Only a panic on the
//! UI thread counts: the engine's own panics are caught by the bridge and reported as an ordinary
//! error result, and the app does not stop because of them.

use std::backtrace::Backtrace;
use std::path::{Path, PathBuf};
use std::thread::ThreadId;

/// How much of the file the next start shows.
const SHOWN_BYTES: usize = 2500;

/// Keep a crash report in `file` whenever the calling (UI) thread panics. Call it once, from the
/// UI thread, as early as possible.
pub fn install(file: PathBuf, version: String) {
    let ui_thread: ThreadId = std::thread::current().id();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        if thread.id() == ui_thread {
            let name = thread.name().unwrap_or("main");
            let text = format!(
                "Kiln {version}, thread {name}\n{info}\n{}\n",
                Backtrace::force_capture()
            );
            let _ = std::fs::write(&file, text);
        }
        previous(info);
    }));
}

/// The report left by the last run, if there is one. The file is removed, so it is shown once.
pub fn take(file: &Path) -> Option<String> {
    let bytes = std::fs::read(file).ok()?;
    let _ = std::fs::remove_file(file);
    let head = &bytes[..bytes.len().min(SHOWN_BYTES)];
    let text = String::from_utf8_lossy(head).into_owned();
    if text.is_empty() { None } else { Some(text) }
}
