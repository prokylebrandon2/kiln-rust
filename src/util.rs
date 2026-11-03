//! Small formatting and parsing helpers shared by the CLI and the benchmark.

/// Format a byte count with binary units, e.g. `4.00 MiB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2} {}", UNITS[unit])
}

/// Parse a size such as `4MiB`, `512k`, `1G` or `65536` (all units are powers of 1024).
pub fn parse_size(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, suffix) = text.split_at(split);
    let number: u64 = digits
        .parse()
        .map_err(|_| format!("invalid size: {text:?}"))?;
    let shift = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 0,
        "k" | "kb" | "kib" => 10,
        "m" | "mb" | "mib" => 20,
        "g" | "gb" | "gib" => 30,
        _ => return Err(format!("unknown size suffix in {text:?} (use K, M or G)")),
    };
    number
        .checked_mul(1u64 << shift)
        .ok_or_else(|| format!("size too large: {text:?}"))
}

/// Compression ratio `raw / packed` (higher is better); `0.0` when `packed` is zero.
pub fn ratio(raw: u64, packed: u64) -> f64 {
    if packed == 0 {
        0.0
    } else {
        raw as f64 / packed as f64
    }
}

/// Throughput in MiB/s; `0.0` when no time elapsed.
pub fn mib_per_sec(bytes: u64, secs: f64) -> f64 {
    if secs <= 0.0 {
        0.0
    } else {
        bytes as f64 / (1024.0 * 1024.0) / secs
    }
}

/// Format Unix seconds as `YYYY-MM-DD HH:MM:SS` (UTC), without any date-time dependency.
pub fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm), valid for the whole proleptic calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_format() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.00 KiB");
        assert_eq!(format_size(4 << 20), "4.00 MiB");
        assert_eq!(format_size(u64::MAX), "16384.00 PiB");
    }

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_size("65536"), Ok(65536));
        assert_eq!(parse_size("4MiB"), Ok(4 << 20));
        assert_eq!(parse_size(" 512k "), Ok(512 << 10));
        assert_eq!(parse_size("1G"), Ok(1 << 30));
        assert!(parse_size("").is_err());
        assert!(parse_size("MiB").is_err());
        assert!(parse_size("4 parsecs").is_err());
        assert!(parse_size("99999999999999999999").is_err());
        assert!(parse_size("17179869184G").is_err()); // overflows u64
    }

    #[test]
    fn utc_formatting_matches_known_dates() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00");
        assert_eq!(format_utc(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14 22:13:20");
        assert_eq!(format_utc(-1), "1969-12-31 23:59:59");
        assert_eq!(format_utc(4_102_444_800), "2100-01-01 00:00:00");
    }

    #[test]
    fn ratio_and_speed_are_safe_on_zero() {
        assert_eq!(ratio(10, 0), 0.0);
        assert_eq!(ratio(10, 5), 2.0);
        assert_eq!(mib_per_sec(1 << 20, 0.0), 0.0);
        assert_eq!(mib_per_sec(1 << 20, 0.5), 2.0);
    }
}
