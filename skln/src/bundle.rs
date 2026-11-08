//! The kind-1 payload of a `.skln`: a bundle of named members (specified in `docs/FORMAT.md`).
//!
//! ```text
//! count u32 | { name_len u16 | name (UTF-8) | data_len u32 | data }*       all little-endian
//! ```
//!
//! Names are labels, not paths: not empty, no `/` or `\`, not `.` or `..`, no control characters,
//! at most 255 bytes, no duplicates. At most 1024 members and 64 MiB in all. Every length is
//! checked against what is left of the input, so a crafted bundle can neither make the reader
//! allocate nor read past its buffer.

use std::collections::HashSet;
use std::fmt;

use zeroize::{Zeroize, Zeroizing};

use crate::{open, seal_with, Error, Kind, Params};

pub const MAX_MEMBERS: usize = 1024;
pub const MAX_NAME_LEN: usize = 255;
/// The most the encoded bundle may take, counting every length field.
pub const MAX_BUNDLE_LEN: usize = 64 * 1024 * 1024;

/// One named blob. The data is wiped when the member is dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub data: Vec<u8>,
}

impl Member {
    pub fn new(name: &str, data: Vec<u8>) -> Member {
        Member {
            name: name.to_owned(),
            data,
        }
    }
}

impl Drop for Member {
    fn drop(&mut self) {
        self.data.zeroize();
    }
}

// The data is secret in Silk's case, so it is never printed.
impl fmt::Debug for Member {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Member")
            .field("name", &self.name)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// A decrypted bundle file.
#[derive(Debug)]
pub struct OpenedBundle {
    pub revision: u64,
    pub members: Vec<Member>,
}

impl OpenedBundle {
    pub fn get(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name == name)
    }
}

fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
}

/// Encodes members. A bad name, a duplicate, too many members or too much data is [`Error::Limit`].
/// The buffer is allocated once at its final size, so reallocation leaves no stale copy behind,
/// and it is wiped on drop.
pub fn pack(members: &[Member]) -> Result<Zeroizing<Vec<u8>>, Error> {
    if members.len() > MAX_MEMBERS {
        return Err(Error::Limit);
    }
    let mut seen = HashSet::new();
    let mut total: usize = 4;
    for m in members {
        if !name_ok(&m.name)
            || !seen.insert(m.name.as_str())
            || u32::try_from(m.data.len()).is_err()
        {
            return Err(Error::Limit);
        }
        total = total
            .checked_add(2 + m.name.len() + 4 + m.data.len())
            .ok_or(Error::Limit)?;
    }
    if total > MAX_BUNDLE_LEN {
        return Err(Error::Limit);
    }
    let mut out = Zeroizing::new(Vec::with_capacity(total));
    out.extend_from_slice(&(members.len() as u32).to_le_bytes());
    for m in members {
        out.extend_from_slice(&(m.name.len() as u16).to_le_bytes());
        out.extend_from_slice(m.name.as_bytes());
        out.extend_from_slice(&(m.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&m.data);
    }
    Ok(out)
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .ok_or(Error::Corrupt)?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u16(&mut self) -> Result<u16, Error> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Decodes members. Truncation, trailing bytes, duplicate or unsafe names and over-limit counts
/// are all [`Error::Corrupt`] (the bytes were authenticated, so a well-behaved writer never made them).
pub fn unpack(bytes: &[u8]) -> Result<Vec<Member>, Error> {
    if bytes.len() > MAX_BUNDLE_LEN {
        return Err(Error::Corrupt);
    }
    let mut r = Reader { buf: bytes, pos: 0 };
    let count = r.u32()? as usize;
    if count > MAX_MEMBERS {
        return Err(Error::Corrupt);
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len = r.u16()? as usize;
        let name = std::str::from_utf8(r.take(name_len)?).map_err(|_| Error::Corrupt)?;
        if !name_ok(name) || !seen.insert(name.to_owned()) {
            return Err(Error::Corrupt);
        }
        let data_len = r.u32()? as usize;
        let data = r.take(data_len)?;
        out.push(Member::new(name, data.to_vec()));
    }
    if r.pos != bytes.len() {
        return Err(Error::Corrupt);
    }
    Ok(out)
}

/// Packs `members` and seals them as a kind-1 file with the default record size.
pub fn seal_bundle(key: &[u8; 32], revision: u64, members: &[Member]) -> Result<Vec<u8>, Error> {
    let packed = pack(members)?;
    seal_with(key, &Params::new(Kind::Bundle, revision), &packed)
}

/// Opens a file and unpacks it. A file of another kind is [`Error::Unsupported`], so a caller that
/// expects a bundle never mistakes an archive for one.
pub fn open_bundle(key: &[u8; 32], file: &[u8]) -> Result<OpenedBundle, Error> {
    let opened = open(key, file)?;
    if opened.kind != Kind::Bundle {
        return Err(Error::Unsupported);
    }
    Ok(OpenedBundle {
        revision: opened.revision,
        members: unpack(&opened.plaintext)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [5u8; 32];

    fn sample() -> Vec<Member> {
        vec![
            Member::new("vault.json", b"{\"entries\":[]}".to_vec()),
            Member::new("readme.txt", b"hello".to_vec()),
            Member::new("empty", Vec::new()),
        ]
    }

    #[test]
    fn the_layout_is_little_endian() {
        let packed = pack(&[Member::new("ab", vec![9, 8, 7])]).unwrap();
        assert_eq!(
            &packed[..],
            &[1, 0, 0, 0, 2, 0, b'a', b'b', 3, 0, 0, 0, 9, 8, 7]
        );
    }

    #[test]
    fn pack_then_unpack_keeps_members_and_order() {
        let packed = pack(&sample()).unwrap();
        assert_eq!(unpack(&packed).unwrap(), sample());
        assert_eq!(unpack(&pack(&[]).unwrap()).unwrap(), Vec::<Member>::new());
    }

    #[test]
    fn seal_and_open_round_trip() {
        let f = seal_bundle(&KEY, 7, &sample()).unwrap();
        assert_eq!(crate::peek(&f).unwrap().kind, Kind::Bundle);
        let o = open_bundle(&KEY, &f).unwrap();
        assert_eq!(o.revision, 7);
        assert_eq!(o.members, sample());
        assert_eq!(o.get("readme.txt").unwrap().data, b"hello");
        assert!(o.get("missing").is_none());
        assert_eq!(open_bundle(&[6u8; 32], &f).err(), Some(Error::Corrupt));
    }

    #[test]
    fn a_bundle_larger_than_one_record_round_trips() {
        let big = Member::new("big", (0..3_000_000u32).map(|i| (i % 251) as u8).collect());
        let f = seal_bundle(&KEY, 1, std::slice::from_ref(&big)).unwrap();
        assert_eq!(open_bundle(&KEY, &f).unwrap().members, vec![big]);
    }

    #[test]
    fn an_archive_file_is_not_a_bundle() {
        let f = crate::seal(&KEY, Kind::Archive, 1, &pack(&sample()).unwrap()).unwrap();
        assert_eq!(open_bundle(&KEY, &f).err(), Some(Error::Unsupported));
    }

    #[test]
    fn unpack_rejects_truncated_and_trailing_data() {
        let packed = pack(&sample()).unwrap().to_vec();
        for len in 0..packed.len() {
            assert_eq!(
                unpack(&packed[..len]).err(),
                Some(Error::Corrupt),
                "cut {len}"
            );
        }
        let mut extra = packed.clone();
        extra.push(0);
        assert_eq!(unpack(&extra).err(), Some(Error::Corrupt));
    }

    #[test]
    fn a_huge_count_or_length_fails_cleanly() {
        assert_eq!(unpack(&u32::MAX.to_le_bytes()).err(), Some(Error::Corrupt));
        let too_many = ((MAX_MEMBERS + 1) as u32).to_le_bytes();
        assert_eq!(unpack(&too_many).err(), Some(Error::Corrupt));
        // One member whose data length points far past the end of the input.
        let mut b = vec![1, 0, 0, 0, 1, 0, b'a'];
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(unpack(&b).err(), Some(Error::Corrupt));
    }

    #[test]
    fn unsafe_and_duplicate_names_are_refused_both_ways() {
        let dup = [Member::new("a", vec![]), Member::new("a", vec![])];
        assert_eq!(pack(&dup).err(), Some(Error::Limit));
        let long = "x".repeat(MAX_NAME_LEN + 1);
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "nul\0",
            "tab\t",
            long.as_str(),
        ] {
            assert_eq!(
                pack(&[Member::new(bad, vec![])]).err(),
                Some(Error::Limit),
                "{bad:?}"
            );
            // The reader applies the same rules to bytes it did not write itself.
            let mut raw = 1u32.to_le_bytes().to_vec();
            raw.extend_from_slice(&(bad.len() as u16).to_le_bytes());
            raw.extend_from_slice(bad.as_bytes());
            raw.extend_from_slice(&0u32.to_le_bytes());
            assert_eq!(unpack(&raw).err(), Some(Error::Corrupt), "{bad:?}");
        }
        let ok = "x".repeat(MAX_NAME_LEN);
        assert!(pack(&[Member::new(&ok, vec![])]).is_ok());

        // A duplicate and invalid UTF-8 in raw bytes.
        let mut raw = 2u32.to_le_bytes().to_vec();
        for _ in 0..2 {
            raw.extend_from_slice(&[1, 0, b'a', 0, 0, 0, 0]);
        }
        assert_eq!(unpack(&raw).err(), Some(Error::Corrupt));
        let raw = [1, 0, 0, 0, 1, 0, 0xff, 0, 0, 0, 0];
        assert_eq!(unpack(&raw).err(), Some(Error::Corrupt));
    }

    #[test]
    fn the_member_and_size_limits_hold() {
        let many: Vec<Member> = (0..=MAX_MEMBERS)
            .map(|i| Member::new(&format!("m{i}"), vec![]))
            .collect();
        assert_eq!(pack(&many).err(), Some(Error::Limit));
        assert!(pack(&many[..MAX_MEMBERS]).is_ok());

        let huge = Member::new("huge", vec![0u8; MAX_BUNDLE_LEN]);
        assert_eq!(pack(&[huge]).err(), Some(Error::Limit));
    }
}
