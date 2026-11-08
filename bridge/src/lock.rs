//! The lock layer: password-protected `.kiln` files.
//!
//! The engine writes and reads plain archives. A locked archive is a plain one wrapped in an
//! authenticated envelope, so the engine itself never sees a password:
//!
//! ```text
//! 0..24    an ordinary Kiln header (same magic) whose incompatible-feature bit 0 is set. The
//!          file browser recognises "encrypted" from that bit alone, and the plain engine refuses
//!          the file as "unsupported" instead of misreading it.
//! 24..59   kind u8 (0 archive, 1 set index) | log_n u8 | r u32 | p u32 | salt[16]
//!          | nonce prefix[8] | chunk_log2 u8                         (little endian)
//! 59..     records: len u32 | ChaCha20-Poly1305(chunk) with a 16-byte tag.
//! ```
//!
//! The key is scrypt of the NFKC-normalised password. Every chunk is `2^chunk_log2` bytes of the
//! plain archive (the last may be shorter, even empty); the nonce is the 8-byte prefix plus the
//! chunk number; the associated data is the whole 59-byte header plus a "this is the last
//! record" byte. A wrong password fails the first record; a cut, reordered or edited file fails
//! some record. Records have a fixed size, so any chunk can be reached directly: listing a locked
//! archive only decrypts its footer and index.
//!
//! All files of one random-name set share a salt (and so one key derivation) with a fresh random
//! nonce prefix each.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use kiln::Profile;
use kiln::format::{FOOTER_LEN, HEADER_LEN, Header, MAGIC};
use unicode_normalization::UnicodeNormalization;

/// Shown (and recognised by the app through "wrong password") when the first record won't open.
pub const WRONG: &str =
    "authentication failed: wrong password, or the archive is damaged or has been tampered with";

pub const KIND_ARCHIVE: u8 = 0;
pub const KIND_INDEX: u8 = 1;

const EXT_LEN: usize = 35;
/// Size of the envelope header.
pub const LOCK_HEADER_LEN: usize = HEADER_LEN + EXT_LEN;
const TAG: u64 = 16;
const CHUNK_LOG2: u8 = 20;
const LOG_N: u8 = 17;
const R: u32 = 8;
const P: u32 = 1;
/// The most memory a key derivation read from a file may ask for.
const MAX_SCRYPT_BYTES: u64 = 512 << 20;
/// The most that is decrypted in one piece for an index or a footer.
const MAX_PIECE: u64 = 300 << 20;

/// Why a lock operation did not finish.
#[derive(Debug)]
pub enum LockError {
    /// The first record would not open: wrong password (or a damaged file).
    Wrong,
    /// The file is damaged, cut short or has been tampered with.
    Damaged(String),
    /// The app asked the job to stop.
    Cancelled,
    /// Reading or writing failed.
    Io(String),
    /// This device can't derive the key.
    Setup(String),
}

impl LockError {
    /// The text shown to the person.
    pub fn message(&self) -> String {
        match self {
            LockError::Wrong => WRONG.to_string(),
            LockError::Damaged(m) | LockError::Io(m) | LockError::Setup(m) => m.clone(),
            LockError::Cancelled => "Cancelled".to_string(),
        }
    }
}

impl From<std::io::Error> for LockError {
    fn from(e: std::io::Error) -> Self {
        LockError::Io(crate::io::os_msg(&e))
    }
}

fn damaged(what: &str) -> LockError {
    LockError::Damaged(format!(
        "authentication failed: {what}; the archive is damaged or has been tampered with"
    ))
}

// ---------------------------------------------------------------------------------- random

/// `n` random bytes from the operating system.
pub fn random(n: usize) -> Result<Vec<u8>, LockError> {
    let mut buf = vec![0u8; n];
    File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf)
}

/// 32 random hex characters: the name of a file in a set.
pub fn random_name() -> Result<String, LockError> {
    Ok(random(16)?.iter().map(|b| format!("{b:02x}")).collect())
}

// ---------------------------------------------------------------------------------- keys

fn derive(password: &str, log_n: u8, r: u32, p: u32, salt: &[u8]) -> Result<[u8; 32], LockError> {
    let cant = || {
        LockError::Setup(
            "this device can't run the key derivation with the archive's settings".to_string(),
        )
    };
    let pw: String = password.nfkc().collect();
    let params = scrypt::Params::new(log_n, r, p, 32).map_err(|_| cant())?;
    let mut key = [0u8; 32];
    scrypt::scrypt(pw.as_bytes(), salt, &params, &mut key).map_err(|_| cant())?;
    Ok(key)
}

/// What a new locked file (or a whole set) is written with: one salt and the key derived from it.
pub struct Setup {
    log_n: u8,
    r: u32,
    p: u32,
    salt: [u8; 16],
    key: [u8; 32],
}

impl Setup {
    /// A fresh salt for `password` (this is the slow part: do it once per archive or set).
    pub fn new(password: &str) -> Result<Setup, LockError> {
        let salt: [u8; 16] = random(16)?
            .try_into()
            .map_err(|_| LockError::Io("no random bytes".to_string()))?;
        let key = derive(password, LOG_N, R, P, &salt)?;
        Ok(Setup {
            log_n: LOG_N,
            r: R,
            p: P,
            salt,
            key,
        })
    }
}

/// The last derived key: the header's `log_n`, `r`, `p`, the salt and the key.
type LastKey = (u8, u32, u32, [u8; 16], [u8; 32]);

/// Derives keys for files being read, and remembers the last one (a set shares a salt).
pub struct Ring {
    password: String,
    last: Option<LastKey>,
}

impl Ring {
    pub fn new(password: &str) -> Ring {
        Ring {
            password: password.to_string(),
            last: None,
        }
    }

    fn key(&mut self, l: &Lock) -> Result<[u8; 32], LockError> {
        if let Some((ln, r, p, salt, key)) = &self.last {
            if *ln == l.log_n && *r == l.r && *p == l.p && *salt == l.salt {
                return Ok(*key);
            }
        }
        let key = derive(&self.password, l.log_n, l.r, l.p, &l.salt)?;
        self.last = Some((l.log_n, l.r, l.p, l.salt, key));
        Ok(key)
    }
}

// ---------------------------------------------------------------------------------- header

fn build_header(kind: u8, s: &Setup, prefix: &[u8; 8]) -> Vec<u8> {
    let mut h = Header::new(Profile::Store, CHUNK_LOG2);
    h.incompat_flags = 1; // bit 0: encrypted
    let mut v = h.encode().to_vec();
    v.push(kind);
    v.push(s.log_n);
    v.extend_from_slice(&s.r.to_le_bytes());
    v.extend_from_slice(&s.p.to_le_bytes());
    v.extend_from_slice(&s.salt);
    v.extend_from_slice(prefix);
    v.push(CHUNK_LOG2);
    v
}

/// The envelope header of a locked file.
pub struct Lock {
    raw: Vec<u8>,
    kind: u8,
    log_n: u8,
    r: u32,
    p: u32,
    salt: [u8; 16],
    prefix: [u8; 8],
    chunk_log2: u8,
}

impl Lock {
    /// Or `KIND_INDEX`.
    pub fn kind(&self) -> u8 {
        self.kind
    }
}

fn read_full(f: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match f.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(got)
}

fn le32(b: &[u8]) -> u32 {
    let mut a = [0u8; 4];
    a.copy_from_slice(&b[..4]);
    u32::from_le_bytes(a)
}

fn le64(b: &[u8]) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[..8]);
    u64::from_le_bytes(a)
}

/// `Some(lock)` when the file is a locked Kiln file, `None` when it is not (a plain archive or
/// something else: the engine decides).
pub fn peek(f: &mut File) -> Result<Option<Lock>, LockError> {
    f.seek(SeekFrom::Start(0))?;
    let mut buf = [0u8; LOCK_HEADER_LEN];
    let got = read_full(f, &mut buf)?;
    if got < 13 || buf[..8] != MAGIC || buf[12] & 1 == 0 {
        return Ok(None);
    }
    if got < LOCK_HEADER_LEN {
        return Err(damaged("the lock header is cut short"));
    }
    let e = &buf[HEADER_LEN..];
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&e[10..26]);
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&e[26..34]);
    let lock = Lock {
        raw: buf.to_vec(),
        kind: e[0],
        log_n: e[1],
        r: le32(&e[2..6]),
        p: le32(&e[6..10]),
        salt,
        prefix,
        chunk_log2: e[34],
    };
    let memory = (128u64 * u64::from(lock.r.max(1))) << lock.log_n.min(40);
    let sane = (lock.kind == KIND_ARCHIVE || lock.kind == KIND_INDEX)
        && (10..=22).contains(&lock.log_n)
        && (1..=16).contains(&lock.r)
        && (1..=4).contains(&lock.p)
        && (12..=24).contains(&lock.chunk_log2)
        && memory <= MAX_SCRYPT_BYTES;
    if !sane {
        return Err(damaged("the lock header is not valid"));
    }
    Ok(Some(lock))
}

fn nonce(prefix: &[u8; 8], i: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(prefix);
    n[8..].copy_from_slice(&(i as u32).to_le_bytes());
    n
}

// ---------------------------------------------------------------------------------- reading

/// A locked file opened with the right password.
pub struct Opened {
    f: File,
    lock: Lock,
    cipher: ChaCha20Poly1305,
    chunk: u64,
    n: u64,
    plain_len: u64,
}

impl Opened {
    /// Derive the key and check the password against the first record.
    pub fn open(path: &Path, lock: Lock, ring: &mut Ring) -> Result<Opened, LockError> {
        let f = File::open(path)?;
        let size = f.metadata()?.len();
        let key = ring.key(&lock)?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
        let chunk = 1u64 << lock.chunk_log2;
        let rec = 4 + chunk + TAG;
        let head = LOCK_HEADER_LEN as u64;
        if size < head + 4 + TAG {
            return Err(damaged("the file is cut short"));
        }
        let body = size - head;
        let n = body.div_ceil(rec);
        let last_rec = body - (n - 1) * rec;
        if last_rec < 4 + TAG || n > u64::from(u32::MAX) {
            return Err(damaged("the file is cut short"));
        }
        let plain_len = (n - 1) * chunk + (last_rec - 4 - TAG);
        let mut o = Opened {
            f,
            lock,
            cipher,
            chunk,
            n,
            plain_len,
        };
        o.chunk(0)?; // the password check
        Ok(o)
    }

    fn chunk(&mut self, i: u64) -> Result<Vec<u8>, LockError> {
        let rec = 4 + self.chunk + TAG;
        self.f
            .seek(SeekFrom::Start(LOCK_HEADER_LEN as u64 + i * rec))?;
        let cut = |e: std::io::Error| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                damaged("the file is cut short")
            } else {
                LockError::from(e)
            }
        };
        let mut len_bytes = [0u8; 4];
        self.f.read_exact(&mut len_bytes).map_err(cut)?;
        let len = u64::from(u32::from_le_bytes(len_bytes));
        let last = i + 1 == self.n;
        let expect = if last {
            self.plain_len - (self.n - 1) * self.chunk + TAG
        } else {
            self.chunk + TAG
        };
        if len != expect {
            return Err(damaged("a record has the wrong length"));
        }
        let mut ct = vec![0u8; len as usize];
        self.f.read_exact(&mut ct).map_err(cut)?;
        let mut aad = self.lock.raw.clone();
        aad.push(u8::from(last));
        let nc = nonce(&self.lock.prefix, i);
        match self.cipher.decrypt(
            Nonce::from_slice(&nc),
            Payload {
                msg: &ct[..],
                aad: &aad[..],
            },
        ) {
            Ok(p) => Ok(p),
            Err(_) if i == 0 => Err(LockError::Wrong),
            Err(_) => Err(damaged("a record failed its check")),
        }
    }

    /// Decrypt everything into `dst` (a plain archive). `progress(done, total)` is in bytes.
    pub fn decrypt_to(
        &mut self,
        dst: &Path,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<(), LockError> {
        let mut out = File::create(dst)?;
        let mut done = 0u64;
        for i in 0..self.n {
            if kiln::cancel::is_set() {
                return Err(LockError::Cancelled);
            }
            let p = self.chunk(i)?;
            out.write_all(&p)?;
            done += p.len() as u64;
            progress(done, self.plain_len);
        }
        out.flush()?;
        Ok(())
    }

    /// `len` bytes of the plain archive starting at `a`.
    fn read_plain(&mut self, a: u64, len: u64) -> Result<Vec<u8>, LockError> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let end = match a.checked_add(len) {
            Some(e) if e <= self.plain_len && len <= MAX_PIECE => e,
            _ => return Err(damaged("the archive inside is not valid")),
        };
        let mut out = Vec::with_capacity(len as usize);
        let first = a / self.chunk;
        let last = (end - 1) / self.chunk;
        for i in first..=last {
            let p = self.chunk(i)?;
            let start = if i == first {
                (a - i * self.chunk) as usize
            } else {
                0
            };
            let stop = if i == last {
                (end - i * self.chunk) as usize
            } else {
                p.len()
            };
            out.extend_from_slice(&p[start..stop]);
        }
        Ok(out)
    }

    /// Everything inside, in memory (for a small index file).
    pub fn read_all(&mut self) -> Result<Vec<u8>, LockError> {
        self.read_plain(0, self.plain_len)
    }

    /// A file of the same length as the archive inside, holding only its header, footer and
    /// index (the rest is a hole): enough for `Archive::open`, so listing is fast.
    pub fn reveal_index(&mut self, dst: &Path) -> Result<(), LockError> {
        let foot_len = FOOTER_LEN as u64;
        if self.plain_len < (HEADER_LEN + FOOTER_LEN) as u64 {
            return Err(damaged("the archive inside is cut short"));
        }
        let head = self.read_plain(0, HEADER_LEN as u64)?;
        let foot = self.read_plain(self.plain_len - foot_len, foot_len)?;
        let index_offset = le64(&foot);
        if index_offset < HEADER_LEN as u64 || index_offset > self.plain_len - foot_len {
            return Err(damaged("the archive inside is not valid"));
        }
        let tail = self.read_plain(index_offset, self.plain_len - index_offset)?;
        let mut out = File::create(dst)?;
        out.set_len(self.plain_len)?;
        out.write_all(&head)?;
        out.seek(SeekFrom::Start(index_offset))?;
        out.write_all(&tail)?;
        out.flush()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------------- writing

/// Encrypt everything `src` yields (`total` bytes, for progress) into the new file `out`.
/// Returns the size written. A failed or cancelled run removes `out`.
fn seal_from(
    src: &mut dyn Read,
    total: u64,
    out: &Path,
    s: &Setup,
    kind: u8,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<u64, LockError> {
    let res = seal_inner(src, total, out, s, kind, progress);
    if res.is_err() {
        let _ = fs::remove_file(out);
    }
    res
}

fn fill(src: &mut dyn Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match src.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(got)
}

fn seal_inner(
    src: &mut dyn Read,
    total: u64,
    out: &Path,
    s: &Setup,
    kind: u8,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<u64, LockError> {
    let prefix: [u8; 8] = random(8)?
        .try_into()
        .map_err(|_| LockError::Io("no random bytes".to_string()))?;
    let header = build_header(kind, s, &prefix);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&s.key));
    let chunk = 1usize << CHUNK_LOG2;
    let mut f = File::create(out)?;
    f.write_all(&header)?;
    let mut written = header.len() as u64;

    let mut cur = vec![0u8; chunk];
    let mut next = vec![0u8; chunk];
    let mut cur_n = fill(src, &mut cur)?;
    let (mut i, mut done) = (0u64, 0u64);
    loop {
        if kiln::cancel::is_set() {
            return Err(LockError::Cancelled);
        }
        // Only a full chunk can be followed by another one: look ahead to know which is last.
        let next_n = if cur_n == chunk {
            fill(src, &mut next)?
        } else {
            0
        };
        let last = next_n == 0;
        if i > u64::from(u32::MAX) {
            return Err(LockError::Io(
                "this archive is too large to lock".to_string(),
            ));
        }
        let mut aad = header.clone();
        aad.push(u8::from(last));
        let nc = nonce(&prefix, i);
        let ct = cipher
            .encrypt(
                Nonce::from_slice(&nc),
                Payload {
                    msg: &cur[..cur_n],
                    aad: &aad[..],
                },
            )
            .map_err(|_| LockError::Io("couldn't encrypt".to_string()))?;
        f.write_all(&(ct.len() as u32).to_le_bytes())?;
        f.write_all(&ct)?;
        written += 4 + ct.len() as u64;
        done += cur_n as u64;
        progress(done, total);
        if last {
            break;
        }
        std::mem::swap(&mut cur, &mut next);
        cur_n = next_n;
        i += 1;
    }
    f.flush()?;
    Ok(written)
}

/// Lock the plain archive `plain` into the new file `out`. Returns the size of `out`.
pub fn seal(
    plain: &Path,
    out: &Path,
    s: &Setup,
    kind: u8,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<u64, LockError> {
    let mut f = File::open(plain)?;
    let total = f.metadata()?.len();
    seal_from(&mut f, total, out, s, kind, progress)
}

/// Lock a small piece of data (a set's index) into the new file `out`.
pub fn seal_bytes(data: &[u8], out: &Path, s: &Setup, kind: u8) -> Result<u64, LockError> {
    let mut src = data;
    seal_from(&mut src, data.len() as u64, out, s, kind, &mut |_, _| {})
}

// ---------------------------------------------------------------------------------- scratch

/// A private temporary folder, removed when dropped. Folders left behind by a process that was
/// killed are swept up when the next one is made.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new() -> Result<Scratch, LockError> {
        let base = std::env::temp_dir();
        sweep(&base);
        let p = base.join(format!("kiln-lock-{}", random_name()?));
        fs::create_dir_all(&p)?;
        Ok(Scratch(p))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Remove `kiln-lock-*` folders older than six hours (nothing running now is that old).
fn sweep(base: &Path) {
    let Ok(rd) = fs::read_dir(base) else { return };
    for ent in rd.flatten() {
        if !ent.file_name().to_string_lossy().starts_with("kiln-lock-") {
            continue;
        }
        let old = ent
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > Duration::from_secs(6 * 3600));
        if old {
            let _ = fs::remove_dir_all(ent.path());
        }
    }
}
