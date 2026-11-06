//! The facts the Properties box shows (`Props.java`): sizes, dates, permissions, the mount a file
//! lives on, and the totals of a folder or a selection. No screen code lives here.

use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::{fmt, sniff};

/// A walk reports at most this often while it counts (`Props.walk`).
const REPORT_EVERY: Duration = Duration::from_millis(150);

/// `Props.sizeText`: "512 B", or "1.50 MB (1572864 B)".
pub fn size_text(n: i64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else {
        format!("{} ({n} B)", fmt::size(n))
    }
}

/// `File.lastModified`, in epoch seconds (0 when the file cannot be read).
pub fn modified(path: &Path) -> i64 {
    std::fs::metadata(path).map_or(0, |m| m.mtime())
}

/// "yyyy-MM-dd HH:mm:ss" in the zone whose offset from UTC is `utc_offset` seconds.
pub fn modified_text(epoch_seconds: i64, utc_offset: i32) -> String {
    let date = fmt::date(epoch_seconds, utc_offset);
    if date.is_empty() {
        return date;
    }
    let second = (epoch_seconds + i64::from(utc_offset)).rem_euclid(60);
    format!("{date}:{second:02}")
}

/// Three permission bits as "rw-".
fn triple(bits: u32) -> String {
    [(4, 'r'), (2, 'w'), (1, 'x')]
        .iter()
        .map(|&(bit, letter)| if bits & bit != 0 { letter } else { '-' })
        .collect()
}

/// `Props.permissions`: "rw- rw- --- (660)", or `None` when the file cannot be read.
pub fn permissions(path: &Path) -> Option<String> {
    let mode = std::fs::metadata(path).ok()?.mode() & 0o777;
    let [user, group, other] = [6, 3, 0].map(|shift| triple((mode >> shift) & 7));
    Some(format!("{user} {group} {other} ({mode:o})"))
}

/// `Props.mountOf`: the mount point that holds `path` and its file system type, read from
/// `/proc/mounts`. The longest mount point that contains the path wins, the later one on a tie.
pub fn mount_of(path: &Path) -> Option<(String, String)> {
    let path = std::fs::canonicalize(path)
        .ok()?
        .to_string_lossy()
        .into_owned();
    let table = std::fs::read("/proc/mounts").ok()?;
    let mut best: Option<(String, String)> = None;
    for line in String::from_utf8_lossy(&table).lines() {
        let parts: Vec<&str> = line.split(' ').collect();
        let [_, mount, kind, ..] = parts.as_slice() else {
            continue;
        };
        let mount = mount.replace("\\040", " ");
        let inside = mount == "/" || path == mount || path.starts_with(&format!("{mount}/"));
        let longer = best
            .as_ref()
            .is_none_or(|(have, _)| mount.len() >= have.len());
        if inside && longer {
            best = Some((mount, (*kind).to_string()));
        }
    }
    best
}

/// What a walk has counted so far.
#[derive(Clone, Copy, Debug, Default)]
pub struct Totals {
    pub files: u64,
    pub bytes: u64,
    pub folders: u64,
}

struct Walk<'a> {
    totals: Totals,
    last: Option<Instant>,
    stop: &'a AtomicBool,
    folders_too: bool,
    report: &'a mut dyn FnMut(Totals, bool),
}

impl Walk<'_> {
    /// `Props.walk`: count one entry and what is below it. False when `stop` was set meanwhile.
    fn visit(&mut self, path: &Path, top: bool) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return false;
        }
        let is_dir = std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
        if is_dir {
            if !top || self.folders_too {
                self.totals.folders += 1;
            }
            if let Ok(kids) = std::fs::read_dir(path) {
                for kid in kids.flatten() {
                    if !self.visit(&kid.path(), false) {
                        return false;
                    }
                }
            }
        } else {
            self.totals.files += 1;
            self.totals.bytes += std::fs::metadata(path).map_or(0, |m| m.len());
        }
        if self.last.is_none_or(|at| at.elapsed() > REPORT_EVERY) {
            self.last = Some(Instant::now());
            (self.report)(self.totals, true);
        }
        true
    }
}

/// `Props.totals`: walk the roots (links are counted as files, not followed) and call `report`
/// with the counts so far and `true` about every 150 ms, then once with the final counts and
/// `false`. `folders_too` counts the roots that are folders as well (a selection of several, not
/// a single folder). Returns false, without a final report, when `stop` was set.
pub fn totals(
    roots: &[&Path],
    folders_too: bool,
    stop: &AtomicBool,
    report: &mut dyn FnMut(Totals, bool),
) -> bool {
    let mut walk = Walk {
        totals: Totals::default(),
        last: None,
        stop,
        folders_too,
        report,
    };
    for root in roots {
        if !walk.visit(root, true) {
            return false;
        }
    }
    (walk.report)(walk.totals, false);
    true
}

/// File types by extension. Android's `MimeTypeMap` knows many more; this is the common ones.
const MIME_BY_EXT: &[(&str, &str)] = &[
    ("txt", "text/plain"),
    ("md", "text/markdown"),
    ("html", "text/html"),
    ("htm", "text/html"),
    ("css", "text/css"),
    ("csv", "text/csv"),
    ("xml", "text/xml"),
    ("js", "text/javascript"),
    ("json", "application/json"),
    ("pdf", "application/pdf"),
    ("rtf", "application/rtf"),
    ("epub", "application/epub+zip"),
    ("zip", "application/zip"),
    ("gz", "application/gzip"),
    ("tar", "application/x-tar"),
    ("7z", "application/x-7z-compressed"),
    ("rar", "application/x-rar-compressed"),
    ("jar", "application/java-archive"),
    ("apk", "application/vnd.android.package-archive"),
    ("sh", "application/x-sh"),
    ("doc", "application/msword"),
    ("xls", "application/vnd.ms-excel"),
    ("ppt", "application/vnd.ms-powerpoint"),
    ("odt", "application/vnd.oasis.opendocument.text"),
    ("ods", "application/vnd.oasis.opendocument.spreadsheet"),
    ("odp", "application/vnd.oasis.opendocument.presentation"),
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("svg", "image/svg+xml"),
    ("ico", "image/x-icon"),
    ("heic", "image/heic"),
    ("heif", "image/heif"),
    ("avif", "image/avif"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("mp3", "audio/mpeg"),
    ("m4a", "audio/mp4"),
    ("wav", "audio/x-wav"),
    ("ogg", "audio/ogg"),
    ("opus", "audio/ogg"),
    ("flac", "audio/flac"),
    ("aac", "audio/aac"),
    ("mid", "audio/midi"),
    ("mp4", "video/mp4"),
    ("m4v", "video/x-m4v"),
    ("mkv", "video/x-matroska"),
    ("webm", "video/webm"),
    ("3gp", "video/3gpp"),
    ("avi", "video/x-msvideo"),
    ("mov", "video/quicktime"),
    ("flv", "video/x-flv"),
    ("wmv", "video/x-ms-wmv"),
];

/// The type for a lower-case extension, if the table knows it.
pub fn mime_by_ext(ext: &str) -> Option<&'static str> {
    MIME_BY_EXT.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
}

/// `Props.typeOf`: the "Type" row of a file. `kind` is how the file list knows it ("archive",
/// "set", "file", ...). By extension first, then by what the file starts with.
pub fn type_of(path: &Path, kind: &str) -> String {
    if kind == "archive" || kind == "set" {
        return "application/x-kiln".to_string();
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if let Some(m) = mime_by_ext(&fmt::ext(&name)) {
        return m.to_string();
    }
    if path.is_file() {
        let sniffed = sniff::mime(path);
        let found = if sniffed == "*/*" {
            "application/octet-stream"
        } else {
            sniffed
        };
        return found.to_string();
    }
    "Unknown".to_string()
}

/// The rows of "Archive properties" that come from reading the archive's table.
#[derive(Clone, Debug)]
pub struct ArchiveFacts {
    pub encrypted: String,
    pub files: String,
    pub content: String,
    pub ratio: String,
}

fn facts(encrypted: &str, files: &str, content: &str, ratio: &str) -> ArchiveFacts {
    ArchiveFacts {
        encrypted: encrypted.to_string(),
        files: files.to_string(),
        content: content.to_string(),
        ratio: ratio.to_string(),
    }
}

/// What `Props.single` shows for an archive or a set, from the engine's `open_archive` answer
/// `r`. `file_len` stands in for `packed` when the answer has none.
pub fn archive_facts(r: &Value, file_len: i64) -> ArchiveFacts {
    if r["need_password"].as_bool() == Some(true) {
        let n = "needs the password";
        return facts("Yes (open it to see more)", n, n, n);
    }
    if r["ok"].as_bool() != Some(true) {
        let why = r["error"].as_str().unwrap_or("couldn't read it");
        return facts("?", why, "?", "?");
    }
    let empty = Vec::new();
    let entries = r["entries"].as_array().unwrap_or(&empty);
    let total: i64 = entries
        .iter()
        .map(|e| e["s"].as_i64().unwrap_or(0).max(0))
        .sum();
    let packed = r["packed"].as_i64().unwrap_or(file_len);
    let encrypted = if r["encrypted"].as_bool() == Some(true) {
        "Yes"
    } else {
        "No"
    };
    let ratio = if total > 0 {
        let percent = (100.0 * packed as f64 / total as f64).round();
        format!("{}%", percent as i64)
    } else {
        "-".to_string()
    };
    facts(
        encrypted,
        &entries.len().to_string(),
        &size_text(total),
        &ratio,
    )
}

/// `Box.text`: what Copy puts on the clipboard. A line with no value is a heading.
pub fn copy_text(lines: &[(String, Option<String>)]) -> String {
    let mut out = String::new();
    for (label, value) in lines {
        match value {
            Some(v) => out.push_str(&format!("{label}: {v}\n")),
            None => out.push_str(&format!("\n{label}\n")),
        }
    }
    out.trim().to_string()
}

/// Rotate amounts of the 64 MD5 steps (RFC 1321).
const SHIFT: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

/// The 64 constants: the integer part of 2^32 * |sin(i + 1)|.
const SINES: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// MD5 (RFC 1321), fed in pieces. Only for the Properties box's checksum row, as
/// `MessageDigest.getInstance("MD5")` is in `Props.md5Of`; it is not for security.
struct Md5 {
    state: [u32; 4],
    block: [u8; 64],
    filled: usize,
    length: u64,
}

impl Md5 {
    fn new() -> Md5 {
        Md5 {
            state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
            block: [0; 64],
            filled: 0,
            length: 0,
        }
    }

    /// Mix one 64-byte block into the state.
    fn compress(state: &mut [u32; 4], block: &[u8; 64]) {
        let mut words = [0u32; 16];
        for (word, bytes) in words.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        let [mut a, mut b, mut c, mut d] = *state;
        for (i, (&sine, &shift)) in SINES.iter().zip(SHIFT.iter()).enumerate() {
            let (mix, at) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let sum = a
                .wrapping_add(mix)
                .wrapping_add(sine)
                .wrapping_add(words[at]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(sum.rotate_left(shift));
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
    }

    fn update(&mut self, mut data: &[u8]) {
        self.length += data.len() as u64;
        while !data.is_empty() {
            let take = (64 - self.filled).min(data.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 64 {
                Md5::compress(&mut self.state, &self.block);
                self.filled = 0;
            }
        }
    }

    /// Pad as the RFC says (a 1 bit, zeros, the length in bits) and return the digest as hex.
    fn finish(mut self) -> String {
        let bits = self.length.wrapping_mul(8);
        let zeros = (119 - self.filled) % 64;
        let mut tail = vec![0x80];
        tail.resize(1 + zeros, 0);
        tail.extend_from_slice(&bits.to_le_bytes());
        self.update(&tail);
        self.state
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

/// `Props.md5Of`: the MD5 of a file as 32 lower-case hex digits. `Ok(None)` when `stop` was set
/// meanwhile (the box was closed); the caller shows "unavailable" for an `Err`.
pub fn md5_of(path: &Path, stop: &AtomicBool) -> std::io::Result<Option<String>> {
    let mut file = std::fs::File::open(path)?;
    let mut md5 = Md5::new();
    let mut buf = vec![0u8; 1 << 18];
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let got = file.read(&mut buf)?;
        if got == 0 {
            return Ok(Some(md5.finish()));
        }
        md5.update(&buf[..got]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md5_hex(data: &[u8]) -> String {
        let mut md5 = Md5::new();
        md5.update(data);
        md5.finish()
    }

    #[test]
    fn md5_matches_the_rfc_1321_vectors() {
        let cases = [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (text, digest) in cases {
            assert_eq!(md5_hex(text.as_bytes()), digest, "{text:?}");
        }
    }

    #[test]
    fn md5_pads_correctly_around_the_block_size() {
        // Digests of bytes (i * 7 + 3) as u8 for i in 0..n, taken from Python's hashlib.
        let cases = [
            (55, "52c0e574e1198de5fe3f8f11440dcb1b"),
            (56, "46c9907fc908ee68b1e7b8e71286a518"),
            (64, "7160b8fb5e9e4023d549c3971fbaeead"),
            (65, "70bd662e7aefbda85a0f7244167b7897"),
        ];
        for (n, digest) in cases {
            let data: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
            assert_eq!(md5_hex(&data), digest, "{n} bytes");
            let mut md5 = Md5::new();
            for piece in data.chunks(13) {
                md5.update(piece);
            }
            assert_eq!(md5.finish(), digest, "{n} bytes in pieces");
        }
    }

    #[test]
    fn md5_of_reads_a_file_and_honours_stop() {
        let path = std::env::temp_dir().join("kiln_props_md5_test.bin");
        std::fs::write(&path, b"abc").unwrap();
        let go = AtomicBool::new(false);
        let digest = md5_of(&path, &go).unwrap();
        assert_eq!(digest.as_deref(), Some("900150983cd24fb0d6963f7d28e17f72"));
        assert_eq!(md5_of(&path, &AtomicBool::new(true)).unwrap(), None);
        assert!(md5_of(&path.with_extension("missing"), &go).is_err());
        std::fs::remove_file(&path).unwrap();
    }
}
