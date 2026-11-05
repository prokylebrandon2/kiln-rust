//! What a running job shares with the app: its progress sink and its cancel flag.
//!
//! The engine used to run in a child process that spoke JSON lines on stdout and took `cancel` on
//! stdin. It now runs in the app's own process, so a [`Job`] takes the place of that pipe: the app
//! makes one per user operation, hands it to [`call_job`](crate::call_job), keeps a clone, and
//! calls [`Job::cancel`] on the clone to stop the operation.
//!
//! A job is in force on the thread that runs it (thread-locals below). The archive engine reports
//! progress and checks for cancellation on that same thread, so this is all that is needed; the
//! one place that starts threads of its own (`packing`) passes the job's token on explicitly.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Receives progress events: `{"frac": 0..1 or -1, "done": bytes, "total": bytes, "file": name}`.
/// It runs on the thread that runs the job, so it must be quick and must not block.
pub type Sink = Arc<dyn Fn(&Value) + Send + Sync>;

/// One user operation: a way to ask it to stop, and optionally somewhere to send its progress.
#[derive(Clone)]
pub struct Job {
    token: kiln::cancel::Token,
    sink: Option<Sink>,
}

impl Job {
    /// A job without progress reporting.
    pub fn new() -> Self {
        Self {
            token: Arc::default(),
            sink: None,
        }
    }

    /// A job that reports progress to `sink`.
    pub fn with_progress(sink: impl Fn(&Value) + Send + Sync + 'static) -> Self {
        Self {
            token: Arc::default(),
            sink: Some(Arc::new(sink)),
        }
    }

    /// Ask the job to stop at its next check. Safe to call from any thread, and more than once.
    pub fn cancel(&self) {
        self.token.store(true, Ordering::Relaxed);
    }

    /// True once [`cancel`](Self::cancel) has been called.
    pub fn is_cancelled(&self) -> bool {
        self.token.load(Ordering::Relaxed)
    }

    pub(crate) fn token(&self) -> kiln::cancel::Token {
        self.token.clone()
    }

    pub(crate) fn sink(&self) -> Option<Sink> {
        self.sink.clone()
    }
}

impl Default for Job {
    fn default() -> Self {
        Self::new()
    }
}

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Keeps a progress sink in force on this thread; the previous one comes back when it is dropped.
#[must_use = "the sink is only in force while the guard is alive"]
pub struct SinkScope {
    previous: Option<Sink>,
}

impl Drop for SinkScope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        SINK.with(|s| *s.borrow_mut() = previous);
    }
}

/// Send this thread's progress to `sink` (or nowhere, for `None`) until the guard is dropped.
pub(crate) fn set_sink(sink: Option<Sink>) -> SinkScope {
    let previous = SINK.with(|s| std::mem::replace(&mut *s.borrow_mut(), sink));
    SinkScope { previous }
}

fn emit(event: &Value) {
    let sink = SINK.with(|s| s.borrow().clone());
    if let Some(sink) = sink {
        sink(event);
    }
}

/// True once the job running on this thread was asked to stop.
pub fn cancelled() -> bool {
    kiln::cancel::is_set()
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
        emit(&json!({ "frac": frac, "done": done, "total": total, "file": file }));
    }
}
