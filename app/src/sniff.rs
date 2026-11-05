//! `Sniff.java`: what a file is, from its first bytes.

use std::io::Read;
use std::path::Path;

/// `Sniff.TEXT`.
pub const TEXT: i32 = 0;
/// `Sniff.IMAGE`.
pub const IMAGE: i32 = 2;
/// `Sniff.BINARY`.
pub const BINARY: i32 = -1;

const HEAD: usize = 8192;

/// The first 8 KB of a file (less for a shorter one), or `None` when it cannot be read.
pub fn head(path: &Path) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; HEAD];
    let mut n = 0;
    while n < buf.len() {
        match file.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(got) => n += got,
            Err(_) => return None,
        }
    }
    buf.truncate(n);
    Some(buf)
}

/// What a file is: [`TEXT`], [`IMAGE`] or [`BINARY`].
pub fn kind(path: &Path) -> i32 {
    let Some(b) = head(path) else {
        return BINARY;
    };
    if b.is_empty() {
        return TEXT;
    }
    if let Some(m) = magic_mime(&b) {
        return if is_viewable_image(m) { IMAGE } else { BINARY };
    }
    if looks_text(&b) { TEXT } else { BINARY }
}

/// The MIME type to hand to other apps (`*/*` when nothing is known).
pub fn mime(path: &Path) -> &'static str {
    let Some(b) = head(path) else {
        return "*/*";
    };
    if b.is_empty() {
        return "text/plain";
    }
    if let Some(m) = magic_mime(&b) {
        return m;
    }
    if looks_text(&b) { "text/plain" } else { "*/*" }
}

fn is_viewable_image(m: &str) -> bool {
    matches!(
        m,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp"
    )
}

fn at(b: &[u8], off: usize, v: &[u8]) -> bool {
    b.len() >= off + v.len() && &b[off..off + v.len()] == v
}

/// The type a file's first bytes announce, if they announce one.
pub fn magic_mime(b: &[u8]) -> Option<&'static str> {
    let table: [(usize, &[u8], &str); 12] = [
        (
            0,
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            "image/png",
        ),
        (0, &[0xFF, 0xD8, 0xFF], "image/jpeg"),
        (0, b"GIF87a", "image/gif"),
        (0, b"GIF89a", "image/gif"),
        (0, b"%PDF-", "application/pdf"),
        (
            0,
            &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C],
            "application/x-7z-compressed",
        ),
        (0, &[0x1F, 0x8B], "application/gzip"),
        (0, b"Rar!", "application/vnd.rar"),
        (0, b"OggS", "audio/ogg"),
        (0, b"fLaC", "audio/flac"),
        (4, b"ftyp", "video/mp4"),
        (0, &[0x1A, 0x45, 0xDF, 0xA3], "video/x-matroska"),
    ];
    for (off, bytes, mime) in table {
        if at(b, off, bytes) {
            return Some(mime);
        }
    }
    if at(b, 0, b"RIFF") && at(b, 8, b"WEBP") {
        return Some("image/webp");
    }
    if at(b, 0, b"RIFF") && at(b, 8, b"WAVE") {
        return Some("audio/x-wav");
    }
    if at(b, 0, b"BM") && b.len() > 14 && at(b, 6, &[0, 0, 0, 0]) {
        return Some("image/bmp");
    }
    if at(b, 0, b"PK") && (at(b, 2, &[3, 4]) || at(b, 2, &[5, 6])) {
        return Some("application/zip");
    }
    let mp3 = [[0xFF, 0xFB], [0xFF, 0xFA], [0xFF, 0xF3], [0xFF, 0xF2]];
    if at(b, 0, b"ID3") || mp3.iter().any(|m| at(b, 0, m)) {
        return Some("audio/mpeg");
    }
    if at(b, 0, b"SQLite format 3") {
        return Some("application/vnd.sqlite3");
    }
    if at(b, 0, &[0x7F, 0x45, 0x4C, 0x46]) {
        return Some("application/x-executable");
    }
    // a dex file, and a .kiln archive
    if at(b, 0, b"dex\n") || at(b, 0, &[0x89, 0x4B, 0x49, 0x4C, 0x4E, 0x0D, 0x0A, 0x1A]) {
        return Some("application/octet-stream");
    }
    None
}

/// Text: no NUL byte, at most 2% control characters, and valid UTF-8 (a character cut off by the
/// end of the 8 KB window is fine).
pub fn looks_text(b: &[u8]) -> bool {
    let mut ctl = 0usize;
    for &v in b {
        if v == 0 {
            return false;
        }
        if v < 32 && !matches!(v, b'\n' | b'\r' | b'\t' | 12 | 27 | 8) {
            ctl += 1;
        }
    }
    if ctl * 50 > b.len() {
        return false;
    }
    match std::str::from_utf8(b) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(),
    }
}
