//! How sizes, dates and counts are written. Pure functions: no global switches.

/// Count 1000 bytes to a kB (decimal) or 1024 to a KB (binary).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Units {
    #[default]
    Binary,
    Decimal,
}

/// "?" when unknown, bytes below one unit, then KB, MB, GB and TB with two decimals under 10 and
/// one from 10 up (the same rule as the list has always used).
pub fn size(n: i64, units: Units) -> String {
    let (base, names): (f64, [&str; 4]) = match units {
        Units::Binary => (1024.0, ["KB", "MB", "GB", "TB"]),
        Units::Decimal => (1000.0, ["kB", "MB", "GB", "TB"]),
    };
    if n < 0 {
        return "?".to_string();
    }
    if (n as f64) < base {
        return format!("{n} B");
    }
    let mut v = n as f64 / base;
    let mut u = 0;
    while v >= base && u < names.len() - 1 {
        v /= base;
        u += 1;
    }
    let digits = if v < 10.0 { 2 } else { 1 };
    format!("{} {}", fixed(v, digits), names[u])
}

fn fixed(v: f64, digits: u32) -> String {
    let scale = 10_i64.pow(digits);
    let scaled = (v * scale as f64).round() as i64;
    let width = digits as usize;
    format!("{}.{:0width$}", scaled / scale, scaled % scale)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "Oct 28, 2026" for a time in epoch seconds in the zone `utc_offset` seconds from UTC; empty
/// for no time.
pub fn date(epoch_seconds: i64, utc_offset: i32) -> String {
    if epoch_seconds <= 0 {
        return String::new();
    }
    let days = (epoch_seconds + i64::from(utc_offset)).div_euclid(86_400);
    let (y, m, d) = civil(days);
    format!("{} {d}, {y}", MONTHS[(m - 1) as usize])
}

/// Year, month (1..=12) and day of a day count since 1970-01-01 (proleptic Gregorian).
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

/// The lower-case text after the last dot, or "". A leading dot alone is not an extension.
pub fn ext(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// "1 item", "3 items".
pub fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// What a question about the selection is about: the quoted name for one item, "N items" for
/// more (`Delete "a.txt"?`, `Delete 3 items?`).
pub fn what(names: &[&str]) -> String {
    match names {
        [one] => format!("\"{one}\""),
        _ => plural(names.len() as u64, "item", "items"),
    }
}

/// What a paste says when names already exist in the folder: the title ("1 item already exists",
/// "3 items already exist") and the lines under it, the first five as "• name" and then "and N
/// more".
pub fn clashes(names: &[&str]) -> (String, Vec<String>) {
    let title = plural(
        names.len() as u64,
        "item already exists",
        "items already exist",
    );
    let mut lines: Vec<String> = names
        .iter()
        .take(5)
        .map(|n| format!("\u{2022} {n}"))
        .collect();
    if names.len() > 5 {
        lines.push(format!("and {} more", names.len() - 5));
    }
    (title, lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(-1, Units::Binary), "?");
        assert_eq!(size(0, Units::Binary), "0 B");
        assert_eq!(size(1023, Units::Binary), "1023 B");
        assert_eq!(size(1024, Units::Binary), "1.00 KB");
        assert_eq!(size(10 * 1024, Units::Binary), "10.0 KB");
        assert_eq!(size(1000, Units::Decimal), "1.00 kB");
        assert_eq!(size(999, Units::Decimal), "999 B");
        assert_eq!(size(5 * 1024 * 1024 * 1024, Units::Binary), "5.00 GB");
    }

    #[test]
    fn dates() {
        assert_eq!(date(0, 0), "");
        // 2026-10-28 12:00:00 UTC
        assert_eq!(date(1_793_188_800, 0), "Oct 28, 2026");
        // the same instant, 13 hours east, is already the next day
        assert_eq!(date(1_793_188_800, 13 * 3600), "Oct 29, 2026");
        assert_eq!(date(86_400, 0), "Jan 2, 1970");
    }

    #[test]
    fn extensions() {
        assert_eq!(ext("a.TXT"), "txt");
        assert_eq!(ext("archive.tar.gz"), "gz");
        assert_eq!(ext(".nomedia"), "");
        assert_eq!(ext("README"), "");
    }

    #[test]
    fn clashes_name_five_and_count_the_rest() {
        let (title, lines) = clashes(&["a"]);
        assert_eq!(title, "1 item already exists");
        assert_eq!(lines, ["\u{2022} a"]);
        let names = ["a", "b", "c", "d", "e", "f", "g"];
        let (title, lines) = clashes(&names);
        assert_eq!(title, "7 items already exist");
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[4], "\u{2022} e");
        assert_eq!(lines[5], "and 2 more");
        assert_eq!(clashes(&names[..5]).1.len(), 5);
    }

    #[test]
    fn what_the_selection_is() {
        assert_eq!(what(&["a.txt"]), "\"a.txt\"");
        assert_eq!(what(&["a", "b"]), "2 items");
        assert_eq!(what(&[]), "0 items");
    }

    #[test]
    fn plurals() {
        assert_eq!(plural(1, "item", "items"), "1 item");
        assert_eq!(plural(23, "item", "items"), "23 items");
    }
}
