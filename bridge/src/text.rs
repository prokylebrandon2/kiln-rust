//! Wording and result shapes shared by the archive functions. The app reads some of these texts
//! back, so their wording is part of the interface.

use std::time::Duration;

use kiln::Error;
use serde_json::{Value, json};

use crate::io::os_msg;

/// A size the way the app has always shown it: `88 B`, `3.4 KB`, `12.0 MB`, `1.5 GB`.
pub fn fmt_size(n: u64) -> String {
    let mut x = n as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if x < 1024.0 || unit == "GB" {
            return if unit == "B" {
                format!("{x:.0} {unit}")
            } else {
                format!("{x:.1} {unit}")
            };
        }
        x /= 1024.0;
    }
    unreachable!()
}

/// Seconds with one decimal, as in "in 0.4s".
pub fn secs(d: Duration) -> String {
    format!("{:.1}", d.as_secs_f64())
}

/// What the person sees for an engine error.
pub fn describe(e: &Error) -> String {
    match e {
        Error::Io { path, source } => format!("{}: {}", path.display(), os_msg(source)),
        Error::IoBare(source) => os_msg(source),
        Error::Exists(p) => format!("{} already exists (use -f to overwrite)", p.display()),
        other => other.to_string(),
    }
}

/// True when the error is "the storage is full".
pub fn storage_full(e: &Error) -> bool {
    match e {
        Error::Io { source, .. } | Error::IoBare(source) => {
            source.kind() == std::io::ErrorKind::StorageFull
        }
        _ => false,
    }
}

/// `{ok:false, cancelled:true, message:"Cancelled"}`.
pub fn cancelled() -> Value {
    json!({ "ok": false, "cancelled": true, "message": "Cancelled" })
}

/// `{ok:false, message, done, bad_password}`: a failed run.
pub fn failed(msg: &str, done: u64) -> Value {
    json!({ "ok": false, "message": msg, "done": done, "bad_password": msg.contains("wrong password") })
}

/// The answer for an engine error: the cancelled shape for a cancel, else a failure message.
pub fn from_error(e: &Error) -> Value {
    if kiln::cancel::is_cancelled(e) {
        cancelled()
    } else {
        failed(&describe(e), 0)
    }
}
