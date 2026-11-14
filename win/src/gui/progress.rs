//! The progress window: a title that carries the percent, two lines of text, a bar and Cancel,
//! like 7-Zip's. The work runs on its own thread; the window only polls it every 50 ms, so no
//! message ever crosses threads. It stays hidden for the first 400 ms, so a quick operation never
//! flashes a window.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::size_of;
use std::panic::{self, AssertUnwindSafe};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use kiln::cancel;
use kiln_win::ops::{self, Outcome};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{CreateFontW, HBRUSH};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{INITCOMMONCONTROLSEX, InitCommonControlsEx};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, IDC_ARROW,
    IsDialogMessageW, KillTimer, LoadCursorW, MSG, PostQuitMessage, RegisterClassExW, SendMessageW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage,
    WNDCLASSEXW,
};

use crate::w::{scale, wide};

/// The work: it reports `(bytes_done, bytes_total)` through the callback it is given.
pub type Work = Box<dyn FnOnce(&mut dyn FnMut(u64, u64)) -> Outcome + Send>;

const WM_DESTROY: u32 = 0x0002;
const WM_SETFONT: u32 = 0x0030;
const WM_CLOSE: u32 = 0x0010;
const WM_NCCREATE: u32 = 0x0081;
const WM_COMMAND: u32 = 0x0111;
const WM_TIMER: u32 = 0x0113;
const PBM_SETPOS: u32 = 0x0402;
const PBM_SETRANGE32: u32 = 0x0406;
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const WS_CAPTION: u32 = 0x00C0_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CHILD: u32 = 0x4000_0000;
const BS_DEFPUSHBUTTON: u32 = 0x0001;
const SS_ENDELLIPSIS: u32 = 0x4000;
const SW_SHOW: i32 = 5;
const GWLP_USERDATA: i32 = -21;
const IDCANCEL: usize = 2;
const TIMER_ID: usize = 1;
const SHOW_AFTER: Duration = Duration::from_millis(400);

/// What the worker thread and the window share.
struct Shared {
    done: AtomicU64,
    total: AtomicU64,
    finished: AtomicBool,
    result: Mutex<Option<Outcome>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The window's state; `GWLP_USERDATA` points at it while the window lives.
struct Ui {
    shared: Arc<Shared>,
    verb: &'static str,
    started: Instant,
    detail: Cell<HWND>,
    bar: Cell<HWND>,
    cancel: Cell<HWND>,
    shown: Cell<bool>,
    cancelling: Cell<bool>,
    last_text: RefCell<String>,
}

impl Ui {
    fn tick(&self, hwnd: HWND) {
        if self.shared.finished.load(Ordering::Acquire) {
            unsafe { DestroyWindow(hwnd) };
            return;
        }
        if !self.shown.get() {
            if self.started.elapsed() < SHOW_AFTER {
                return;
            }
            self.shown.set(true);
            unsafe {
                ShowWindow(hwnd, SW_SHOW);
                SetForegroundWindow(hwnd);
            }
        }
        let done = self.shared.done.load(Ordering::Relaxed);
        let total = self.shared.total.load(Ordering::Relaxed);
        let ratio = if total == 0 {
            0.0
        } else {
            done.min(total) as f64 / total as f64
        };
        unsafe { SendMessageW(self.bar.get(), PBM_SETPOS, (ratio * 1000.0) as usize, 0) };

        let secs = self.started.elapsed().as_secs();
        let elapsed = format!("{}:{:02}", secs / 60, secs % 60);
        let pct = (ratio * 100.0) as u32;
        let text = if self.cancelling.get() {
            "Cancelling...".to_string()
        } else if total == 0 {
            format!("Preparing...   {elapsed}")
        } else {
            let (d, t) = (ops::fmt_bytes(done), ops::fmt_bytes(total));
            format!("{pct}%   {d} of {t}   {elapsed}")
        };
        if *self.last_text.borrow() != text {
            set_text(self.detail.get(), &text);
            set_text(hwnd, &format!("{pct}% {}", self.verb));
            *self.last_text.borrow_mut() = text;
        }
    }

    fn cancel_clicked(&self) {
        if self.cancelling.replace(true) {
            return;
        }
        cancel::request();
        unsafe { EnableWindow(self.cancel.get(), 0) };
    }
}

fn set_text(hwnd: HWND, text: &str) {
    let text = wide(text);
    unsafe { SetWindowTextW(hwnd, text.as_ptr()) };
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = lparam as *const CREATESTRUCTW;
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*create).lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
    }
    let ui = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Ui;
    if ui.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // The state outlives the window (see `run`) and is only touched from this thread.
    let ui = unsafe { &*ui };
    match msg {
        WM_TIMER => {
            ui.tick(hwnd);
            0
        }
        WM_COMMAND if wparam & 0xFFFF == IDCANCEL => {
            ui.cancel_clicked();
            0
        }
        WM_CLOSE => {
            if ui.shared.finished.load(Ordering::Acquire) {
                unsafe { DestroyWindow(hwnd) };
            } else {
                ui.cancel_clicked();
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                KillTimer(hwnd, TIMER_ID);
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// A child control at `at` = (x, y, width, height) in 96-dpi pixels.
fn child(
    parent: HWND,
    class: &str,
    text: &str,
    style: u32,
    at: (i32, i32, i32, i32),
    id: usize,
) -> HWND {
    let class = wide(class);
    let text = wide(text);
    let (x, y, w, h) = at;
    unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            text.as_ptr(),
            style | WS_CHILD | WS_VISIBLE,
            scale(x),
            scale(y),
            scale(w),
            scale(h),
            parent,
            id as _,
            GetModuleHandleW(null()),
            null(),
        )
    }
}

fn label(parent: HWND, text: &str, y: i32, id: usize) -> HWND {
    child(parent, "STATIC", text, SS_ENDELLIPSIS, (16, y, 408, 18), id)
}

/// Make the (still hidden) window and its controls. Null when Windows refuses.
fn create_window(ui: &Ui, headline: &str) -> HWND {
    let class = wide("KilnProgress");
    let title = wide(&format!("0% {}", ui.verb));
    let style = WS_CAPTION | WS_SYSMENU;
    let hwnd = unsafe {
        let hinst = GetModuleHandleW(null());
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: 0x20, // ICC_PROGRESS_CLASS
        };
        InitCommonControlsEx(&icc);

        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(wnd_proc);
        wc.hInstance = hinst;
        wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        wc.hbrBackground = 16usize as HBRUSH; // COLOR_BTNFACE + 1
        wc.lpszClassName = class.as_ptr();
        // Fails with "already registered" on the second run of one process; the class is there.
        RegisterClassExW(&wc);

        let mut rect = RECT {
            left: 0,
            top: 0,
            right: scale(440),
            bottom: scale(142),
        };
        AdjustWindowRectEx(&mut rect, style, 0, 0);
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let x = (GetSystemMetrics(0) - w) / 2;
        let y = (GetSystemMetrics(1) - h) / 3;
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            style,
            x,
            y,
            w,
            h,
            null_mut(),
            null_mut(),
            hinst,
            ui as *const Ui as *const c_void,
        )
    };
    if hwnd.is_null() {
        return hwnd;
    }

    let headline = label(hwnd, headline, 14, 100);
    let detail = label(hwnd, "Preparing...", 40, 101);
    let bar = child(hwnd, "msctls_progress32", "", 0, (16, 68, 408, 20), 102);
    let button = child(
        hwnd,
        "BUTTON",
        "Cancel",
        WS_TABSTOP | BS_DEFPUSHBUTTON,
        (344, 104, 80, 26),
        IDCANCEL,
    );
    ui.detail.set(detail);
    ui.bar.set(bar);
    ui.cancel.set(button);
    unsafe {
        let face = wide("Segoe UI");
        let font = CreateFontW(
            -scale(12),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            0,
            face.as_ptr(),
        );
        for control in [headline, detail, bar, button] {
            SendMessageW(control, WM_SETFONT, font as usize, 1);
        }
        SendMessageW(bar, PBM_SETRANGE32, 0, 1000);
        SetTimer(hwnd, TIMER_ID, 50, None);
    }
    hwnd
}

fn pump(hwnd: HWND) {
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
        if unsafe { IsDialogMessageW(hwnd, &msg) } == 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

/// Run `work` on a thread behind a progress window and return how it ended. `verb` ("Extracting",
/// "Testing", "Adding") goes in the title after the percent; `headline` is the first line.
pub fn run(verb: &'static str, headline: &str, work: Work) -> Outcome {
    cancel::clear();
    let shared = Arc::new(Shared {
        done: AtomicU64::new(0),
        total: AtomicU64::new(0),
        finished: AtomicBool::new(false),
        result: Mutex::new(None),
    });
    let ui = Ui {
        shared: Arc::clone(&shared),
        verb,
        started: Instant::now(),
        detail: Cell::new(null_mut()),
        bar: Cell::new(null_mut()),
        cancel: Cell::new(null_mut()),
        shown: Cell::new(false),
        cancelling: Cell::new(false),
        last_text: RefCell::new(String::new()),
    };

    let worker = {
        let shared = Arc::clone(&shared);
        thread::spawn(move || {
            let mut report = |done: u64, total: u64| {
                shared.done.store(done, Ordering::Relaxed);
                shared.total.store(total, Ordering::Relaxed);
            };
            let outcome = panic::catch_unwind(AssertUnwindSafe(|| work(&mut report)))
                .unwrap_or_else(|_| Outcome::Failed("The operation stopped unexpectedly.".into()));
            *lock(&shared.result) = Some(outcome);
            shared.finished.store(true, Ordering::Release);
        })
    };

    let hwnd = create_window(&ui, headline);
    if !hwnd.is_null() {
        pump(hwnd);
    }
    // The window only closes once the work is over, so this does not wait.
    let _ = worker.join();
    lock(&shared.result)
        .take()
        .unwrap_or_else(|| Outcome::Failed("The operation did not finish.".into()))
}
