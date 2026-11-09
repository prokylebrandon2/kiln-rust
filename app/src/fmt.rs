//! How sizes, dates and names are written in the lists.

use std::sync::atomic::{AtomicBool, Ordering};

/// Count 1000 bytes to a kB instead of 1024 to a KB (Settings > File sizes).
static DECIMAL: AtomicBool = AtomicBool::new(false);

/// Choose decimal (1000) or binary (1024) sizes.
pub fn set_decimal(on: bool) {
    DECIMAL.store(on, Ordering::Relaxed);
}

/// "?" when unknown, bytes below 1 KB, then KB, MB, GB and TB with two decimals under
/// 10 and one decimal from 10 up, rounded half up. Decimal sizes count
/// 1000 and write kB.
pub fn size(n: i64) -> String {
    let decimal = DECIMAL.load(Ordering::Relaxed);
    let units: [&str; 4] = if decimal {
        ["kB", "MB", "GB", "TB"]
    } else {
        ["KB", "MB", "GB", "TB"]
    };
    let base = if decimal { 1000.0 } else { 1024.0 };
    if n < 0 {
        return "?".to_string();
    }
    if (n as f64) < base {
        return format!("{n} B");
    }
    let mut v = n as f64 / base;
    let mut u = 0;
    while v >= base && u < units.len() - 1 {
        v /= base;
        u += 1;
    }
    let digits = if v < 10.0 { 2 } else { 1 };
    format!("{} {}", fixed(v, digits), units[u])
}

fn fixed(v: f64, digits: u32) -> String {
    let scale = 10_i64.pow(digits);
    let scaled = (v * scale as f64).round() as i64;
    let width = digits as usize;
    format!("{}.{:0width$}", scaled / scale, scaled % scale)
}

/// "yyyy-MM-dd HH:mm" for a time in epoch seconds, in the zone whose offset from UTC
/// is `utc_offset` seconds; empty for no time.
pub fn date(epoch_seconds: i64, utc_offset: i32) -> String {
    if epoch_seconds <= 0 {
        return String::new();
    }
    let t = epoch_seconds + i64::from(utc_offset);
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    let (hour, minute) = (secs / 3600, secs % 3600 / 60);
    format!("{y:04}-{m:02}-{d:02} {hour:02}:{minute:02}")
}

/// Year, month and day of a day count since 1970-01-01 (proleptic Gregorian calendar).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// The lower-case text after the last dot, or "".
pub fn ext(name: &str) -> String {
    match name.rfind('.') {
        Some(i) => name[i + 1..].to_lowercase(),
        None => String::new(),
    }
}

/// "1 item", "3 items".
pub fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// An I/O error, without Rust's " (os error N)".
pub fn io_text(e: &std::io::Error) -> String {
    let mut text = e.to_string();
    if let Some(i) = text.find(" (os error") {
        text.truncate(i);
    }
    text
}
