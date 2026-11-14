//! `kiln-gui`: the helper the Explorer "Kiln" menu starts (like 7-Zip's `7zG.exe`). It shows a
//! progress window with Cancel while it extracts, tests or adds, and sets the menu up with
//! `register` / `unregister`.
//!
//! ```text
//! kiln-gui x  <archive>   Extract Here
//! kiln-gui xf <archive>   Extract to a folder named after the archive
//! kiln-gui t  <archive>   Test archive
//! kiln-gui az <path>      Add to <name>.zip (next to the file or folder)
//! kiln-gui ak <path>      Add to <name>.kiln
//! kiln-gui register | unregister
//! ```
//!
//! With `KILN_GUI_QUIET` set no message box is shown (the CI smoke test uses that).

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod progress;
#[cfg(windows)]
mod shell;
#[cfg(windows)]
mod w;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    app::run()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("kiln-gui only runs on Windows.");
}
