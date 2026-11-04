//! A process-wide "stop now" flag for hosts that have a Cancel button. // UI-HOOK
//!
//! The command-line tool never sets it, so for every other caller the only cost is one relaxed
//! atomic load per chunk, taken at the point where progress is already reported. The Android
//! bridge raises it when the app asks a running job to stop; [`pack_with_progress`],
//! [`extract_with_progress`] and [`verify_with_progress`] then return
//! [`Error::Invalid`]`("Cancelled")`, which [`is_cancelled`] recognises. The usual cleanup runs on
//! that path: a half-written archive is a temp file that is removed, never renamed into place.
//!
//! [`pack_with_progress`]: crate::pack_with_progress
//! [`extract_with_progress`]: crate::extract_with_progress
//! [`verify_with_progress`]: crate::verify_with_progress

use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, Result};

static CANCEL: AtomicBool = AtomicBool::new(false);

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

/// True once [`request`] has been called (and not [`clear`]ed).
#[inline]
pub fn is_set() -> bool {
    CANCEL.load(Ordering::Relaxed)
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
