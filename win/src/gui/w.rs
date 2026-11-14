//! The few Win32 calls the helper needs outside the progress window.

use std::ptr::null_mut;

use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;

const MB_YESNO: u32 = 0x0004;
const MB_ICONERROR: u32 = 0x0010;
const MB_ICONQUESTION: u32 = 0x0020;
const MB_ICONINFORMATION: u32 = 0x0040;
const MB_SETFOREGROUND: u32 = 0x0001_0000;

pub const IDYES: i32 = 6;
pub const IDNO: i32 = 7;

/// A NUL-terminated UTF-16 copy of `s`.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// With `KILN_GUI_QUIET` set no box is shown: on a build server nobody could click it. The text
/// goes to standard error instead (there is none unless the caller redirected it), and every
/// question is answered "no".
fn quiet() -> bool {
    std::env::var_os("KILN_GUI_QUIET").is_some()
}

fn show(title: &str, text: &str, style: u32) -> i32 {
    if quiet() {
        eprintln!("{title}: {text}");
        return IDNO;
    }
    let title = wide(title);
    let text = wide(text);
    let style = style | MB_SETFOREGROUND;
    unsafe { MessageBoxW(null_mut(), text.as_ptr(), title.as_ptr(), style) }
}

pub fn info(title: &str, text: &str) {
    show(title, text, MB_ICONINFORMATION);
}

pub fn error(title: &str, text: &str) {
    show(title, text, MB_ICONERROR);
}

/// A Yes / No question.
pub fn ask(title: &str, text: &str) -> bool {
    show(title, text, MB_YESNO | MB_ICONQUESTION) == IDYES
}

/// Draw at the screen's real resolution instead of letting Windows stretch the window. Call it
/// before the first window. (-2 is `DPI_AWARENESS_CONTEXT_SYSTEM_AWARE`.)
pub fn dpi_aware() {
    unsafe { SetProcessDpiAwarenessContext(-2isize as _) };
}

/// `v` pixels at 96 dpi, in pixels on this screen.
pub fn scale(v: i32) -> i32 {
    let dpi = unsafe { GetDpiForSystem() };
    let dpi = if dpi == 0 { 96 } else { dpi };
    (i64::from(v) * i64::from(dpi) / 96) as i32
}
