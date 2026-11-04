//! The bridge protocol: one JSON request line on stdin, JSON lines on stdout.
//!
//! stdin:  line 1 is the request `{"fn": "...", "args": [...], "jobs": N}`. A later line `cancel`
//!         (or the end of stdin) asks the running job to stop cleanly.
//! stdout: zero or more `{"progress": {...}}` lines, then exactly one `{"result": {...}}` line.

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

static CANCEL: AtomicBool = AtomicBool::new(false);

/// True once Java asked this job to stop (or closed stdin).
pub fn cancelled() -> bool {
    CANCEL.load(Ordering::Relaxed)
}

/// Raise the cancel flag here and in the engine (`kiln::cancel`, the UI-HOOK the archive pipelines
/// check at their progress point), so a running pack, extract or test returns "Cancelled".
fn raise() {
    CANCEL.store(true, Ordering::Relaxed);
    kiln::cancel::request();
}

/// Read the request line.
pub fn read_request() -> Result<Value, String> {
    let stdin = std::io::stdin();
    let mut line = String::new();
    let n = stdin
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("no request was given to the engine".to_string());
    }
    serde_json::from_str(line.trim()).map_err(|e| format!("bad request: {e}"))
}

/// Watch the rest of stdin on a helper thread and raise the cancel flag when told to.
pub fn watch_cancel() {
    std::thread::spawn(|| {
        let stdin = std::io::stdin();
        let mut lock = stdin.lock();
        let mut line = String::new();
        loop {
            line.clear();
            match lock.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    raise();
                    return;
                }
                Ok(_) => {
                    if line.trim() == "cancel" {
                        raise();
                        return;
                    }
                }
            }
        }
    });
}

fn write_line(v: &Value) {
    let text = v.to_string();
    let out = std::io::stdout();
    let mut lock = out.lock();
    let _ = lock.write_all(text.as_bytes());
    let _ = lock.write_all(b"\n");
    let _ = lock.flush();
}

/// Send the final answer.
pub fn result(v: Value) {
    write_line(&json!({ "result": v }));
}

/// "Permission denied (os error 13)" -> "Permission denied", like Python's `strerror`.
pub fn os_msg(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

/// Throttled progress events: at most one every 100 ms, so about 10 per second.
pub struct Meter {
    last: Option<Instant>,
    /// Where this stretch of work starts on the bar, and how much of the bar it covers (a locked
    /// archive spends the first part of the bar on decrypting).
    base: f64,
    span: f64,
    /// A job made of several stretches (extract, pack, lock) places this whole meter inside part
    /// of the bar: `outer_base`/`outer_span` (both 0..1).
    outer_base: f64,
    outer_span: f64,
}

impl Meter {
    /// A meter that has not sent anything yet (so the first event always goes out).
    pub fn new() -> Self {
        Self {
            last: None,
            base: 0.0,
            span: 1.0,
            outer_base: 0.0,
            outer_span: 1.0,
        }
    }

    /// Everything this meter sends from now on lies in `span` of the bar starting at `base`.
    pub fn set_outer(&mut self, base: f64, span: f64) {
        self.outer_base = base;
        self.outer_span = span;
    }

    /// The next events cover `span` of the bar starting at `base` (both 0..1).
    pub fn set_span(&mut self, base: f64, span: f64) {
        self.base = base;
        self.span = span;
    }

    /// Send `done` of `total` (and the current file name) unless one went out very recently.
    /// `force` skips the throttle (use it for the last event).
    pub fn tick(&mut self, done: u64, total: u64, file: &str, force: bool) {
        if !force {
            if let Some(t) = self.last {
                if t.elapsed() < Duration::from_millis(100) {
                    return;
                }
            }
        }
        self.last = Some(Instant::now());
        let frac = if total == 0 {
            -1.0
        } else {
            let inner = self.base + self.span * (done as f64 / total as f64).min(1.0);
            self.outer_base + self.outer_span * inner
        };
        write_line(&json!({
            "progress": { "frac": frac, "done": done, "total": total, "file": file }
        }));
    }
}
