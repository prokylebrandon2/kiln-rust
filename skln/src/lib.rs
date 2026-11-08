//! The `.skln` container, version 1 (specified in `docs/FORMAT.md`): authenticated encryption of a
//! byte string under a raw 32-byte key. Bytes in, bytes out: no paths, no temp files, no password
//! KDF. A password layer (scrypt) sits above this crate and hands it the key; the optional
//! 26-byte password block in the header is opaque here and is only bound into the authentication.
//!
//! The format is public. The only secret is the key.

#![forbid(unsafe_code)]

use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

/// `\x89SKLN\r\n\x1a`.
pub const MAGIC: [u8; 8] = [0x89, b'S', b'K', b'L', b'N', 0x0D, 0x0A, 0x1A];
pub const VERSION: u8 = 1;
/// The fixed header; a password block of [`PASSWORD_BLOCK_LEN`] bytes follows when flag bit 0 is set.
pub const HEADER_LEN: usize = 36;
pub const PASSWORD_BLOCK_LEN: usize = 26;
pub const TAG_LEN: usize = 16;
pub const MIN_CHUNK_LOG2: u8 = 12;
pub const MAX_CHUNK_LOG2: u8 = 24;
pub const DEFAULT_CHUNK_LOG2: u8 = 20;

const FLAG_PASSWORD: u8 = 1;
const FILE_KEY_INFO: &[u8] = b"kiln skln v1 file key";

/// What the decrypted bytes are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A complete plain `.kiln` v2 archive.
    Archive = 0,
    /// A bundle of named members (Silk's vault).
    Bundle = 1,
}

impl Kind {
    fn from_byte(b: u8) -> Option<Kind> {
        match b {
            0 => Some(Kind::Archive),
            1 => Some(Kind::Bundle),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The bytes do not start with the `.skln` magic.
    NotSkln,
    /// A version or flag this reader does not know.
    Unsupported,
    /// A wrong key, a damaged file, a cut file or a tampered file: they cannot be told apart.
    Corrupt,
    /// A parameter outside the allowed range.
    Limit,
    /// The system random generator failed.
    Random,
    /// A primitive refused its input; not reachable with valid inputs.
    Internal,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Error::NotSkln => "not a .skln file",
            Error::Unsupported => "unsupported .skln version or feature",
            Error::Corrupt => {
                "authentication failed: wrong key, or the file is damaged or has been tampered with"
            }
            Error::Limit => "a .skln parameter is out of range",
            Error::Random => "the system random generator failed",
            Error::Internal => "internal encryption error",
        })
    }
}

impl std::error::Error for Error {}

/// What goes into a new file besides the key and the bytes.
#[derive(Clone, Debug)]
pub struct Params {
    pub kind: Kind,
    /// Raised by one on every save; 0 when the caller does not track it.
    pub revision: u64,
    /// Records hold up to `2^chunk_log2` plaintext bytes; allowed 12..=24.
    pub chunk_log2: u8,
    /// Set by the password layer; this crate only copies it into the header.
    pub password_block: Option<[u8; PASSWORD_BLOCK_LEN]>,
}

impl Params {
    pub fn new(kind: Kind, revision: u64) -> Params {
        Params {
            kind,
            revision,
            chunk_log2: DEFAULT_CHUNK_LOG2,
            password_block: None,
        }
    }
}

/// What the plain header says. Readable without the key; only trustworthy after [`open`] succeeds.
#[derive(Clone, Debug)]
pub struct Info {
    pub kind: Kind,
    pub revision: u64,
    pub chunk_log2: u8,
    pub password_block: Option<[u8; PASSWORD_BLOCK_LEN]>,
    salt: [u8; 16],
    header_len: usize,
}

/// A decrypted file. The bytes are wiped when this is dropped.
pub struct Opened {
    pub kind: Kind,
    pub revision: u64,
    pub plaintext: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for Opened {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Opened")
            .field("kind", &self.kind)
            .field("revision", &self.revision)
            .field("plaintext_len", &self.plaintext.len())
            .finish()
    }
}

fn make_cipher(key: &[u8; 32], salt: &[u8; 16]) -> Result<ChaCha20Poly1305, Error> {
    let hk = Hkdf::<Sha256>::new(Some(&salt[..]), &key[..]);
    let mut file_key = Zeroizing::new([0u8; 32]);
    hk.expand(FILE_KEY_INFO, &mut file_key[..])
        .map_err(|_| Error::Internal)?;
    ChaCha20Poly1305::new_from_slice(&file_key[..]).map_err(|_| Error::Internal)
}

/// 4 zero bytes, then the record number.
fn nonce(i: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&i.to_le_bytes());
    n
}

/// Seals `plaintext` with default parameters.
pub fn seal(key: &[u8; 32], kind: Kind, revision: u64, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
    seal_with(key, &Params::new(kind, revision), plaintext)
}

/// Seals `plaintext`. Every call draws a fresh random file salt, so one key can seal any number of
/// files without a key and nonce pair ever repeating.
pub fn seal_with(key: &[u8; 32], params: &Params, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
    if params.chunk_log2 < MIN_CHUNK_LOG2 || params.chunk_log2 > MAX_CHUNK_LOG2 {
        return Err(Error::Limit);
    }
    let chunk = 1usize << params.chunk_log2;
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|_| Error::Random)?;

    let mut header = Vec::with_capacity(HEADER_LEN + PASSWORD_BLOCK_LEN);
    header.extend_from_slice(&MAGIC);
    header.push(VERSION);
    header.push(if params.password_block.is_some() {
        FLAG_PASSWORD
    } else {
        0
    });
    header.push(params.kind as u8);
    header.push(params.chunk_log2);
    header.extend_from_slice(&params.revision.to_le_bytes());
    header.extend_from_slice(&salt);
    if let Some(block) = &params.password_block {
        header.extend_from_slice(block);
    }

    let cipher = make_cipher(key, &salt)?;
    // At least one record, so an empty payload still has a "last" record to authenticate.
    let pieces: Vec<&[u8]> = if plaintext.is_empty() {
        vec![&plaintext[..]]
    } else {
        plaintext.chunks(chunk).collect()
    };
    let mut out =
        Vec::with_capacity(header.len() + plaintext.len() + pieces.len() * (4 + TAG_LEN));
    out.extend_from_slice(&header);
    let flag_at = header.len();
    let mut aad = header;
    aad.push(0);
    for (i, piece) in pieces.iter().enumerate() {
        aad[flag_at] = u8::from(i + 1 == pieces.len());
        let n = nonce(i as u64);
        let ct = cipher
            .encrypt(
                Nonce::from_slice(&n),
                Payload {
                    msg: *piece,
                    aad: &aad,
                },
            )
            .map_err(|_| Error::Internal)?;
        out.extend_from_slice(&(ct.len() as u32).to_le_bytes());
        out.extend_from_slice(&ct);
    }
    Ok(out)
}

fn parse(file: &[u8]) -> Result<Info, Error> {
    if file.len() < MAGIC.len() || file[..MAGIC.len()] != MAGIC {
        return Err(Error::NotSkln);
    }
    if file.len() <= MAGIC.len() {
        return Err(Error::Corrupt);
    }
    if file[8] != VERSION {
        return Err(Error::Unsupported);
    }
    if file.len() < HEADER_LEN {
        return Err(Error::Corrupt);
    }
    let flags = file[9];
    if flags & !FLAG_PASSWORD != 0 {
        return Err(Error::Unsupported);
    }
    let kind = Kind::from_byte(file[10]).ok_or(Error::Corrupt)?;
    let chunk_log2 = file[11];
    if !(MIN_CHUNK_LOG2..=MAX_CHUNK_LOG2).contains(&chunk_log2) {
        return Err(Error::Corrupt);
    }
    let mut rev = [0u8; 8];
    rev.copy_from_slice(&file[12..20]);
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&file[20..36]);
    let (password_block, header_len) = if flags & FLAG_PASSWORD != 0 {
        if file.len() < HEADER_LEN + PASSWORD_BLOCK_LEN {
            return Err(Error::Corrupt);
        }
        let mut block = [0u8; PASSWORD_BLOCK_LEN];
        block.copy_from_slice(&file[HEADER_LEN..HEADER_LEN + PASSWORD_BLOCK_LEN]);
        (Some(block), HEADER_LEN + PASSWORD_BLOCK_LEN)
    } else {
        (None, HEADER_LEN)
    };
    Ok(Info {
        kind,
        revision: u64::from_le_bytes(rev),
        chunk_log2,
        password_block,
        salt,
        header_len,
    })
}

/// The plain header, without the key: lets a password layer read its block, and lets a caller
/// look at the revision.
pub fn peek(file: &[u8]) -> Result<Info, Error> {
    parse(file)
}

/// The revision from the plain header, without the key.
pub fn revision(file: &[u8]) -> Result<u64, Error> {
    parse(file).map(|i| i.revision)
}

/// Decrypts a whole file. A wrong key, a changed header, an edited, reordered or cut record and
/// appended bytes all fail with [`Error::Corrupt`].
pub fn open(key: &[u8; 32], file: &[u8]) -> Result<Opened, Error> {
    let info = parse(file)?;
    let header = &file[..info.header_len];
    let cipher = make_cipher(key, &info.salt)?;
    let full = (1usize << info.chunk_log2) + TAG_LEN;
    let flag_at = header.len();
    let mut aad = Vec::with_capacity(header.len() + 1);
    aad.extend_from_slice(header);
    aad.push(0);
    // Sized once for the largest possible result, so no stale copies are left by reallocation.
    let mut out = Zeroizing::new(Vec::<u8>::with_capacity(file.len()));
    let mut pos = info.header_len;
    let mut index: u64 = 0;
    loop {
        let len_end = pos.checked_add(4).ok_or(Error::Corrupt)?;
        let lb = file.get(pos..len_end).ok_or(Error::Corrupt)?;
        let len = u32::from_le_bytes([lb[0], lb[1], lb[2], lb[3]]) as usize;
        let end = len_end.checked_add(len).ok_or(Error::Corrupt)?;
        let body = file.get(len_end..end).ok_or(Error::Corrupt)?;
        let last = end == file.len();
        let sized = if last {
            (TAG_LEN..=full).contains(&len)
        } else {
            len == full
        };
        if !sized {
            return Err(Error::Corrupt);
        }
        aad[flag_at] = u8::from(last);
        let n = nonce(index);
        let plain = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(&n),
                    Payload {
                        msg: body,
                        aad: &aad,
                    },
                )
                .map_err(|_| Error::Corrupt)?,
        );
        out.extend_from_slice(&plain[..]);
        if last {
            break;
        }
        pos = end;
        index += 1;
    }
    Ok(Opened {
        kind: info.kind,
        revision: info.revision,
        plaintext: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        [7u8; 32]
    }

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 % 251) as u8).collect()
    }

    /// 4 KiB records, so a few thousand bytes already span several of them.
    fn small(revision: u64) -> Params {
        Params {
            chunk_log2: 12,
            ..Params::new(Kind::Bundle, revision)
        }
    }

    #[test]
    fn round_trips_lengths_around_the_chunk_size() {
        for n in [0usize, 1, 4095, 4096, 4097, 8192, 10_000] {
            let f = seal_with(&key(), &small(3), &data(n)).unwrap();
            let o = open(&key(), &f).unwrap();
            assert_eq!(o.kind, Kind::Bundle);
            assert_eq!(o.revision, 3);
            assert_eq!(&o.plaintext[..], &data(n)[..], "length {n}");
        }
    }

    #[test]
    fn round_trips_with_the_default_chunk_size() {
        let f = seal(&key(), Kind::Archive, 0, b"hello").unwrap();
        let o = open(&key(), &f).unwrap();
        assert_eq!(o.kind, Kind::Archive);
        assert_eq!(&o.plaintext[..], b"hello");
    }

    #[test]
    fn a_wrong_key_fails() {
        let f = seal_with(&key(), &small(1), &data(5000)).unwrap();
        assert!(matches!(open(&[8u8; 32], &f), Err(Error::Corrupt)));
    }

    #[test]
    fn every_single_byte_change_is_detected() {
        let f = seal_with(&key(), &small(9), &data(5000)).unwrap();
        for i in 0..f.len() {
            let mut bad = f.clone();
            bad[i] ^= 1;
            assert!(open(&key(), &bad).is_err(), "byte {i}");
        }
    }

    #[test]
    fn every_truncation_is_detected() {
        let f = seal_with(&key(), &small(9), &data(5000)).unwrap();
        for len in 0..f.len() {
            assert!(open(&key(), &f[..len]).is_err(), "cut at {len}");
        }
    }

    #[test]
    fn appended_bytes_are_detected() {
        let f = seal_with(&key(), &small(1), &data(5000)).unwrap();
        let mut bad = f.clone();
        bad.push(0);
        assert!(open(&key(), &bad).is_err());
    }

    #[test]
    fn swapped_records_are_detected() {
        let f = seal_with(&key(), &small(1), &data(3 * 4096)).unwrap();
        let rec = 4 + 4096 + TAG_LEN;
        assert_eq!(f.len(), HEADER_LEN + 3 * rec);
        let mut bad = f.clone();
        bad[HEADER_LEN..HEADER_LEN + rec].copy_from_slice(&f[HEADER_LEN + rec..HEADER_LEN + 2 * rec]);
        bad[HEADER_LEN + rec..HEADER_LEN + 2 * rec].copy_from_slice(&f[HEADER_LEN..HEADER_LEN + rec]);
        assert!(open(&key(), &bad).is_err());
    }

    #[test]
    fn header_values_are_checked_before_decrypting() {
        let f = seal_with(&key(), &small(1), &data(100)).unwrap();
        let mut bad = f.clone();
        bad[0] ^= 1;
        assert!(matches!(open(&key(), &bad), Err(Error::NotSkln)));
        let mut bad = f.clone();
        bad[8] = 2;
        assert!(matches!(open(&key(), &bad), Err(Error::Unsupported)));
        let mut bad = f.clone();
        bad[9] = 2;
        assert!(matches!(open(&key(), &bad), Err(Error::Unsupported)));
        let mut bad = f.clone();
        bad[10] = 9;
        assert!(matches!(open(&key(), &bad), Err(Error::Corrupt)));
        for v in [11u8, 25] {
            let mut bad = f.clone();
            bad[11] = v;
            assert!(matches!(open(&key(), &bad), Err(Error::Corrupt)), "chunk_log2 {v}");
        }
        assert!(matches!(open(&key(), b""), Err(Error::NotSkln)));
        assert!(matches!(open(&key(), b"hello"), Err(Error::NotSkln)));
    }

    #[test]
    fn chunk_sizes_outside_the_range_are_refused() {
        for v in [11u8, 25] {
            let p = Params {
                chunk_log2: v,
                ..Params::new(Kind::Archive, 0)
            };
            assert!(matches!(seal_with(&key(), &p, b"x"), Err(Error::Limit)));
        }
    }

    #[test]
    fn the_header_is_readable_without_the_key() {
        let f = seal_with(&key(), &small(42), &data(10)).unwrap();
        let i = peek(&f).unwrap();
        assert_eq!(i.kind, Kind::Bundle);
        assert_eq!(i.revision, 42);
        assert_eq!(i.chunk_log2, 12);
        assert!(i.password_block.is_none());
        assert_eq!(revision(&f).unwrap(), 42);
    }

    #[test]
    fn the_password_block_is_carried_and_authenticated() {
        let block = [0xABu8; PASSWORD_BLOCK_LEN];
        let p = Params {
            password_block: Some(block),
            ..small(5)
        };
        let f = seal_with(&key(), &p, &data(5000)).unwrap();
        assert_eq!(peek(&f).unwrap().password_block, Some(block));
        assert_eq!(&open(&key(), &f).unwrap().plaintext[..], &data(5000)[..]);
        let mut bad = f.clone();
        bad[HEADER_LEN + 3] ^= 1;
        assert!(open(&key(), &bad).is_err());
    }

    #[test]
    fn every_save_uses_a_fresh_salt() {
        let a = seal_with(&key(), &small(1), &data(100)).unwrap();
        let b = seal_with(&key(), &small(1), &data(100)).unwrap();
        assert_ne!(&a[20..36], &b[20..36]);
        assert_ne!(a, b);
        assert!(open(&key(), &a).is_ok() && open(&key(), &b).is_ok());
    }
}