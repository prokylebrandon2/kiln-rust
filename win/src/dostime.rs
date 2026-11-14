//! Time helpers for ZIP: DOS date/time fields, civil-date arithmetic and the local UTC offset.
//!
//! ZIP stores a file's time as local wall-clock time in two 16-bit fields. The standard library
//! cannot tell the local UTC offset, so on Windows it is asked from the system; elsewhere it is
//! taken as zero (times are then read and written as UTC).

/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m as i64 - 3 } else { m as i64 + 9 };
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date (year, month 1-12, day 1-31) for days since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Seconds to add to a UTC Unix time to get the local wall-clock time at that moment.
#[cfg(windows)]
pub fn local_offset(utc_secs: i64) -> i64 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::FileTimeToLocalFileTime;

    const UNIX_TO_FILETIME_SECS: i64 = 11_644_473_600;
    let secs = (utc_secs + UNIX_TO_FILETIME_SECS).max(0) as u64;
    let ticks = secs * 10_000_000;
    let utc = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut local = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    // SAFETY: both pointers refer to live, properly aligned FILETIME values for the whole call.
    let ok = unsafe { FileTimeToLocalFileTime(&utc, &mut local) };
    if ok == 0 {
        return 0;
    }
    let local_ticks = ((local.dwHighDateTime as u64) << 32) | local.dwLowDateTime as u64;
    (local_ticks / 10_000_000) as i64 - secs as i64
}

/// Seconds to add to a UTC Unix time to get the local wall-clock time (zero off Windows).
#[cfg(not(windows))]
pub fn local_offset(_utc_secs: i64) -> i64 {
    0
}

/// Convert a UTC Unix time to ZIP's `(date, time)` fields in local time.
///
/// Times before 1980 become 1980-01-01 and times after 2107 become the last representable day.
pub fn unix_to_dos(utc_secs: i64) -> (u16, u16) {
    let local = utc_secs + local_offset(utc_secs);
    let days = local.div_euclid(86_400);
    let sod = local.rem_euclid(86_400);
    let (mut y, mut m, mut d) = civil_from_days(days);
    if y < 1980 {
        return ((1 << 5) | 1, 0);
    }
    if y > 2107 {
        y = 2107;
        m = 12;
        d = 31;
    }
    let date = (((y - 1980) as u16) << 9) | ((m as u16) << 5) | d as u16;
    let time = (((sod / 3600) as u16) << 11)
        | ((((sod % 3600) / 60) as u16) << 5)
        | ((sod % 60 / 2) as u16);
    (date, time)
}

/// Convert ZIP's local `(date, time)` fields to a UTC Unix time; `None` for an empty or invalid
/// date.
pub fn dos_to_unix(date: u16, time: u16) -> Option<i64> {
    if date == 0 {
        return None;
    }
    let day = (date & 31) as u32;
    let month = ((date >> 5) & 15) as u32;
    let year = 1980 + (date >> 9) as i64;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let hour = (time >> 11) as i64;
    let minute = ((time >> 5) & 63) as i64;
    let second = ((time & 31) * 2) as i64;
    let local = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    Some(local - local_offset(local))
}

/// `YYYY-MM-DD HH:MM` in local time, for listings.
pub fn format_local(utc_secs: i64) -> String {
    let local = utc_secs + local_offset(utc_secs);
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    let sod = local.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        sod / 3600,
        sod % 3600 / 60
    )
}
