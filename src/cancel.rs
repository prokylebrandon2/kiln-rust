//! "Stop now" flags for hosts that have a Cancel button. // UI-HOOK
//!
//! There are two. The process-wide flag ([`request`]) stops everything that is running. A job
//! that shares its process with other jobs, as every job of the Android app does, instead runs
//! under its own [`Token`] ([`enter`]): the checks below look at the process-wide flag and at the
//! token of the thread they run on, so cancelling one job leaves the others alone.
//!
//! The command-line tool never sets either, so for every other caller the only cost is a couple of
//! relaxed atomic loads per chunk, taken at the point where progress is already reported. The
//! Android app raises a job's token when the user asks it to stop; [`pack_with_progress`],
//! [`extract_with_progress`] and [`verify_with_progress`] then return
//! [`Error::Invalid`]`("Cancelled")`, which [`is_cancelled`] recognises. The usual cleanup runs on
//! that path: a half-written archive is a temp file that is removed, never renamed into place.
//!
//! [`pack_with_progress`]: crate::pack_with_progress
//! [`extract_with_progress`]: crate::extract_with_progress
//! [`verify_with_progress`]: crate::verify_with_progress

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, Result};

static CANCEL: AtomicBool = AtomicBool::new(false);

/// One job's own "stop now" flag. Cloning shares the flag; store `true` to cancel the job.
pub type Token = Arc<AtomicBool>;

thread_local! {
    static CURRENT: RefCell<Option<Token>> = const { RefCell::new(None) };
}

/// Keeps a [`Token`] in force on this thread; the previous one comes back when it is dropped.
#[must_use = "the token is only in force while the guard is alive"]
pub struct Scope {
    previous: Option<Token>,
}

impl Drop for Scope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CURRENT.with(|c| *c.borrow_mut() = previous);
    }
}

/// Run this thread under `token` until the returned guard is dropped. Threads a job starts for
/// itself do not inherit it: they pass [`current`] on and call `enter` themselves.
pub fn enter(token: Token) -> Scope {
    let previous = CURRENT.with(|c| c.borrow_mut().replace(token));
    Scope { previous }
}

/// The token this thread runs under, if any.
pub fn current() -> Option<Token> {
    CURRENT.with(|c| c.borrow().clone())
}

/// The message carried by the error a cancelled job returns.
pub const CANCELLED: &str = "Cancelled";

/// Ask whatever is running to stop at its next progress point.
pub fn request() {
    CANCEL.store(true, Ordering::Relaxed);
}

/// Lower the flag again (for a host that runs several jobs in one process).
pub fn clear() {
    CANCEL.store(false, Ordering::Relaxed);
}

/// True once [`request`] has been called (and not [`clear`]ed), or once the token this thread
/// runs under has been raised.
#[inline]
pub fn is_set() -> bool {
    CANCEL.load(Ordering::Relaxed)
        || CURRENT.with(|c| c.borrow().as_ref().is_some_and(|t| t.load(Ordering::Relaxed)))
}

/// `Err(Cancelled)` when the flag is up, `Ok` otherwise.
#[inline]
pub fn check() -> Result<()> {
    if is_set() {
        Err(Error::Invalid(CANCELLED.to_string()))
    } else {
        Ok(())
    }
}

/// True when `e` is the error a cancelled job returns.
pub fn is_cancelled(e: &Error) -> bool {
    matches!(e, Error::Invalid(m) if m == CANCELLED)
}
