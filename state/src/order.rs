//! Sorting: folders first, then the chosen key, ties by name in natural order.

use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

use crate::entry::Entry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Date,
    Type,
}

impl SortKey {
    /// The code stored in Settings (`sort_key`).
    pub fn from_code(code: i32) -> Self {
        match code {
            1 => Self::Size,
            2 => Self::Date,
            3 => Self::Type,
            _ => Self::Name,
        }
    }

    pub fn code(self) -> i32 {
        match self {
            Self::Name => 0,
            Self::Size => 1,
            Self::Date => 2,
            Self::Type => 3,
        }
    }

    /// Whether the order depends on what the file system says (so it can change when the
    /// metadata arrives).
    pub fn needs_meta(self) -> bool {
        matches!(self, Self::Size | Self::Date)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Order {
    pub key: SortKey,
    pub ascending: bool,
}

impl Default for Order {
    fn default() -> Self {
        Self {
            key: SortKey::Name,
            ascending: true,
        }
    }
}

fn take_digits(it: &mut Peekable<Chars<'_>>) -> String {
    let mut out = String::new();
    while let Some(&c) = it.peek() {
        if !c.is_ascii_digit() {
            break;
        }
        out.push(c);
        it.next();
    }
    out
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Case-insensitive, and runs of digits compare as numbers: "img2" < "img10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                if x.is_ascii_digit() && y.is_ascii_digit() {
                    let na = take_digits(&mut ai);
                    let nb = take_digits(&mut bi);
                    let ta = na.trim_start_matches('0');
                    let tb = nb.trim_start_matches('0');
                    let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if o != Ordering::Equal {
                        return o;
                    }
                } else {
                    let o = fold(x).cmp(&fold(y));
                    if o != Ordering::Equal {
                        return o;
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

fn directed(o: Ordering, ascending: bool) -> Ordering {
    if ascending { o } else { o.reverse() }
}

/// The order of two entries. Folders always come first, whatever the direction.
pub fn compare(a: &Entry, b: &Entry, order: Order) -> Ordering {
    let (fa, fb) = (a.kind.is_folder(), b.kind.is_folder());
    if fa != fb {
        // folders (true) first
        return fb.cmp(&fa);
    }
    let name = natural_cmp(&a.name, &b.name).then_with(|| a.name.cmp(&b.name));
    let primary = match order.key {
        SortKey::Name => return directed(name, order.ascending),
        SortKey::Size if !fa => a.size().cmp(&b.size()),
        SortKey::Type if !fa => a.ext().cmp(&b.ext()),
        SortKey::Date => a.mtime().cmp(&b.mtime()),
        _ => Ordering::Equal,
    };
    directed(primary, order.ascending).then(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Kind;

    fn file(name: &str, size: u64, mtime: i64) -> Entry {
        Entry::with_meta(name, Kind::File, size, mtime)
    }

    fn names(mut v: Vec<Entry>, order: Order) -> Vec<String> {
        v.sort_by(|a, b| compare(a, b, order));
        v.into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn numbers_compare_as_numbers() {
        assert_eq!(natural_cmp("img2", "img10"), Ordering::Less);
        assert_eq!(natural_cmp("img10", "img2"), Ordering::Greater);
        assert_eq!(natural_cmp("a007", "a7"), Ordering::Equal);
        assert_eq!(natural_cmp("File", "file"), Ordering::Equal);
        assert_eq!(natural_cmp("a", "ab"), Ordering::Less);
    }

    #[test]
    fn folders_come_first_in_both_directions() {
        let v = vec![
            file("a.txt", 1, 1),
            Entry::new("zdir", Kind::Dir),
            Entry::new("Adir", Kind::Dir),
        ];
        let asc = Order::default();
        let desc = Order {
            ascending: false,
            ..asc
        };
        assert_eq!(names(v.clone(), asc), ["Adir", "zdir", "a.txt"]);
        assert_eq!(names(v, desc), ["zdir", "Adir", "a.txt"]);
    }

    #[test]
    fn size_and_date_sort_with_name_ties() {
        let v = vec![file("b", 5, 10), file("a", 5, 30), file("c", 1, 20)];
        let size = Order {
            key: SortKey::Size,
            ascending: true,
        };
        assert_eq!(names(v.clone(), size), ["c", "a", "b"]);
        let date_desc = Order {
            key: SortKey::Date,
            ascending: false,
        };
        assert_eq!(names(v, date_desc), ["a", "c", "b"]);
    }

    #[test]
    fn type_groups_by_extension() {
        let v = vec![
            file("x.zip", 1, 1),
            file("y.apk", 1, 1),
            file("a.zip", 1, 1),
        ];
        let t = Order {
            key: SortKey::Type,
            ascending: true,
        };
        assert_eq!(names(v, t), ["y.apk", "a.zip", "x.zip"]);
    }

    #[test]
    fn sort_key_codes_round_trip() {
        for k in [SortKey::Name, SortKey::Size, SortKey::Date, SortKey::Type] {
            assert_eq!(SortKey::from_code(k.code()), k);
        }
    }
}
