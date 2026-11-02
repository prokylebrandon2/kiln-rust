//! Encryption: key derivation (scrypt of the NFKC-normalised password) and the encrypted
//! record source (AES-256-GCM). Port of derive_key / Cipher / CryptSource in kiln_v14.py.

use crate::container::{KilnError, Records, Result};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use std::io::Read;
use unicode_normalization::UnicodeNormalization;

pub const AUTH_FAIL: &str = "authentication failed: wrong password, or the archive is damaged or has been tampered with";

/// scrypt(NFKC(password).utf8, salt, N = 2^log_n, r, p, 32 bytes)
pub fn derive_key(password: &str, salt: &[u8], log_n: u8, r: u32, p: u32) -> Result<[u8; 32]> {
    let pw: String = password.nfkc().collect();
    let params = scrypt::Params::new(log_n, r, p, 32)
        .map_err(|_| KilnError("this device can't run scrypt with the archive's settings".to_string()))?;
    let mut key = [0u8; 32];
    scrypt::scrypt(pw.as_bytes(), salt, &params, &mut key)
        .map_err(|_| KilnError("this device can't run scrypt with the archive's settings".to_string()))?;
    Ok(key)
}

pub fn make_cipher(key: &[u8; 32]) -> Aes256Gcm {
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
}

/// nonce = u8 stream | 3 zero bytes | u64 counter (big-endian)
pub fn nonce(stream: u8, counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[0] = stream;
    n[4..12].copy_from_slice(&counter.to_be_bytes());
    n
}

fn read_upto<R: Read>(r: &mut R, n: usize) -> Result<Vec<u8>> {
    let mut v = Vec::new();
    r.take(n as u64)
        .read_to_end(&mut v)
        .map_err(|e| KilnError(format!("read error: {e}")))?;
    Ok(v)
}

fn open_with(cipher: &Aes256Gcm, nonce: &[u8; 12], ct: &[u8], header: &[u8], fin: u8) -> Option<Vec<u8>> {
    let mut aad = header.to_vec();
    aad.push(fin);
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad: &aad })
        .ok()
}

/// Records of one encrypted stream (0 = data, 1 = table). `n` is the record counter.
pub struct CryptSource<'a, R: Read> {
    pub f: &'a mut R,
    pub cipher: &'a Aes256Gcm,
    pub header: &'a [u8],
    pub stream: u8,
    pub n: u64,
}

impl<'a, R: Read> Records for CryptSource<'a, R> {
    fn read_record(&mut self) -> Result<Vec<u8>> {
        let raw = read_upto(self.f, 4)?;
        if raw.len() < 4 {
            return Err(KilnError("archive is truncated (stream has no end)".to_string()));
        }
        let length = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
        if length < 17 {
            return Err(KilnError("corrupt data: bad record length".to_string()));
        }
        let ct = read_upto(self.f, length)?;
        if ct.len() < length {
            return Err(KilnError("archive is truncated (cut inside a record)".to_string()));
        }
        let nonce = nonce(self.stream, self.n);
        let fail = || KilnError(AUTH_FAIL.to_string());
        let pt = match open_with(self.cipher, &nonce, &ct, self.header, 0) {
            Some(pt) => {
                if pt.first() == Some(&2) {
                    return Err(fail()); // an end marker is only valid as the final record
                }
                pt
            }
            None => match open_with(self.cipher, &nonce, &ct, self.header, 1) {
                Some(pt) if pt == [2u8] => pt,
                _ => return Err(fail()),
            },
        };
        self.n += 1;
        Ok(pt)
    }
}
