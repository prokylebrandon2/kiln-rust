//! The index: every entry's metadata plus the chunk table.
//!
//! The index is stored *columnar* (all flags, then all path lengths, then all sizes ...) with
//! LEB128 varints, because that compresses far better than interleaved fixed-width records and
//! keeps the per-entry cost low. Layout, before the index record's own compression:
//!
//! ```text
//! index_version u8 (= 1)
//! entry_count varint | chunk_count varint
//! flags     u8 x entry_count          bit 0 = directory, bits 1-3 = content class, rest zero
//! path_len  varint x entry_count      then all path bytes, concatenated (UTF-8)
//! size      varint x (files)
//! mtime     zigzag-varint seconds, delta from the previous entry, x entry_count
//! nanos     varint x entry_count
//! mode      varint x entry_count
//! hash      [u8; 16] x (files)        BLAKE3 truncated to 128 bits
//! raw_len   varint x chunk_count
//! comp_len  varint x chunk_count
//! method    u8 x chunk_count
//! extensions: { tag u16 | len varint | data } ... until the end of the index
//! ```
//!
//! File contents sit in the stream in entry order. Chunk *file offsets* are not stored: chunk `i`
//! starts where chunk `i-1`'s record ends, so offsets are derived from the lengths and the chunk
//! table always agrees with the layout of the data area.

use std::str;

use super::bytes::{Reader, Writer};
use super::record::RECORD_HEADER_LEN;
use super::{HEADER_LEN, MAX_ENTRIES};
use crate::classify::Class;
use crate::codec::Method;
use crate::error::{Error, Result};
use crate::integrity::{FileHash, HASH_LEN, empty_hash};
use crate::path::{MAX_PATH_LEN, validate_archive_path};

/// Version of the index layout above (independent of the container version).
pub const INDEX_VERSION: u8 = 1;
/// Extension tag: free-form UTF-8 name of the program that wrote the archive. Non-critical.
pub const TAG_CREATOR: u16 = 0x0001;
/// Extension tag: whole-file deduplication table (`docs/FORMAT.md`). Critical: without it the
/// entry sizes do not add up to the data stream, so a reader that lacks it must refuse.
pub const TAG_DEDUP: u16 = 0x8001;
/// Extension tag: owner table (`docs/FORMAT.md`): numeric user and group id of every entry.
/// Non-critical: a reader that ignores it just does not restore ownership.
pub const TAG_OWNER: u16 = 0x0002;
/// Extension tag: extended attributes of some entries. Non-critical.
pub const TAG_XATTR: u16 = 0x0003;
/// Extension tag: symbolic links and hard links. Critical: without it a symlink would read as
/// an empty file and a hard link's size would not add up to the data stream.
pub const TAG_LINKS: u16 = 0x8002;
/// Extension tags with this bit set are *critical*: a reader that does not know one must refuse
/// the archive. Tags without it may be skipped.
pub const TAG_CRITICAL: u16 = 0x8000;
/// Longest creator string accepted.
const MAX_CREATOR_LEN: usize = 256;
/// Longest symbolic-link target accepted, in bytes (Linux `PATH_MAX`).
pub const MAX_LINK_TARGET: usize = 4096;
/// Longest extended-attribute name accepted, in bytes.
pub const MAX_XATTR_NAME: usize = 255;
/// Longest extended-attribute value accepted, in bytes (the Linux limit).
pub const MAX_XATTR_VALUE: usize = 64 * 1024;
/// Most extended attributes accepted on one entry.
pub const MAX_XATTRS_PER_ENTRY: usize = 1024;

const LINK_SYMLINK: u8 = 1;
const LINK_HARD: u8 = 2;

/// What an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file whose bytes live in the data stream.
    File,
    /// A directory (kept so empty directories survive a round trip).
    Dir,
}

/// One archived file or directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Relative, `/`-separated name.
    pub path: String,
    /// File or directory.
    pub kind: EntryKind,
    /// Content class decided when the archive was written ([`Class::Unknown`] for directories and
    /// for profiles that do not classify).
    pub class: Class,
    /// Size in bytes (zero for directories).
    pub size: u64,
    /// Modification time, whole seconds since the Unix epoch.
    pub mtime_secs: i64,
    /// Modification time, nanosecond remainder (`0..1_000_000_000`).
    pub mtime_nanos: u32,
    /// Unix permission bits (`0o7777` mask).
    pub mode: u32,
    /// Truncated BLAKE3 of the file contents (all zero for directories).
    pub hash: FileHash,
}

/// What a [`File`](EntryKind::File) entry is when it is a link rather than a regular file with
/// data of its own. Links are recorded in the critical `0x8002` extension; the entry itself
/// stays an ordinary file entry, so tools that do not know about links see an empty file
/// (symlink) or a file with the source's content (hard link).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// A symbolic link. The entry has size 0 and the empty hash; the target is stored verbatim
    /// (raw bytes, not necessarily UTF-8, never containing NUL, and not interpreted: it may be
    /// absolute or contain `..`).
    Symlink(Vec<u8>),
    /// A hard link to an earlier entry (`source`, a regular file entry that is not itself a
    /// hard link or a symlink). The entry has the same size and hash as its source and owns no
    /// bytes in the data stream.
    HardLink {
        /// Index of the entry this one is linked to; always lower than this entry's own index.
        source: usize,
    },
}

/// One extended attribute (name and value are raw bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Xattr {
    /// Attribute name, e.g. `user.comment` (1..=255 bytes, no NUL).
    pub name: Vec<u8>,
    /// Attribute value (at most 64 KiB).
    pub value: Vec<u8>,
}

/// One chunk of the data stream, as recorded in the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkInfo {
    /// Bytes after decoding.
    pub raw_len: u32,
    /// Payload bytes as stored (the record adds a 13-byte header).
    pub comp_len: u32,
    /// How the payload is encoded.
    pub method: Method,
}

/// A tagged, length-prefixed extension record (see the module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extension {
    /// Tag; bit 15 marks the extension critical.
    pub tag: u16,
    /// Payload.
    pub data: Vec<u8>,
}

/// The decoded index plus the layout derived from it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    /// Entries in stream order.
    pub entries: Vec<Entry>,
    /// The chunk table, in stream order.
    pub chunks: Vec<ChunkInfo>,
    /// Extension records, in file order (unknown non-critical ones are preserved).
    pub extensions: Vec<Extension>,
    /// Total bytes in the data stream (derived).
    pub total_raw: u64,
    /// Absolute archive offset of each chunk record (derived).
    pub chunk_offsets: Vec<u64>,
    /// Stream offset of each chunk, plus a final element equal to `total_raw` (derived).
    pub chunk_starts: Vec<u64>,
    /// Stream offset of each entry; directories get the running offset (derived).
    pub entry_starts: Vec<u64>,
    /// Absolute archive offset where the chunk records end (derived).
    pub data_end: u64,
    /// Symbolic and hard links, ascending by entry number (derived from extension `0x8002`).
    pub links: Vec<(usize, Link)>,
    /// `(uid, gid)` of every entry, or empty when the archive records no ownership (derived
    /// from extension `0x0002`).
    pub owners: Vec<(u32, u32)>,
    /// Extended attributes, ascending by entry number (derived from extension `0x0003`).
    pub xattrs: Vec<(usize, Vec<Xattr>)>,
}

/// Smallest possible encoded directory entry: flags, path_len, one name byte, mtime, nanos, mode.
const MIN_ENTRY_BYTES: usize = 6;
/// Smallest possible encoded chunk: raw_len, comp_len, method.
const MIN_CHUNK_BYTES: usize = 3;

/// The deduplication extension for `(duplicate, source)` entry pairs: ascending by duplicate,
/// with every source before its duplicate.
pub fn dedup_extension(pairs: &[(usize, usize)]) -> Extension {
    let mut w = Writer::default();
    w.varint(pairs.len() as u64);
    let mut previous = 0usize;
    for &(dup, source) in pairs {
        w.varint((dup - previous) as u64);
        w.varint((dup - source) as u64);
        previous = dup;
    }
    Extension {
        tag: TAG_DEDUP,
        data: w.0,
    }
}

/// Parse a deduplication table for an index of `entry_count` entries.
fn parse_dedup(data: &[u8], entry_count: usize) -> Result<Vec<(usize, usize)>> {
    let mut r = Reader::new(data);
    let count = r.varint()?;
    if count > (r.remaining() / 2) as u64 {
        return Err(Error::corrupt(
            "deduplication table is larger than its data",
        ));
    }
    let mut pairs = Vec::with_capacity(count as usize);
    let mut previous = 0u64;
    for _ in 0..count {
        let step = r.varint()?;
        let back = r.varint()?;
        let dup = previous
            .checked_add(step)
            .filter(|&d| step > 0 && d < entry_count as u64)
            .ok_or_else(|| Error::corrupt("deduplication table names a bad entry"))?;
        if back == 0 || back > dup {
            return Err(Error::corrupt("deduplication table names a bad source"));
        }
        pairs.push((dup as usize, (dup - back) as usize));
        previous = dup;
    }
    if r.remaining() != 0 {
        return Err(Error::corrupt("deduplication table has trailing bytes"));
    }
    Ok(pairs)
}

/// The link table: `(entry, link)` pairs, ascending by entry. Hard-link sources must come
/// before the entries that link to them.
pub fn links_extension(links: &[(usize, Link)]) -> Extension {
    let mut w = Writer::default();
    w.varint(links.len() as u64);
    let mut previous: Option<usize> = None;
    for (entry, link) in links {
        w.varint((entry - previous.unwrap_or(0)) as u64);
        previous = Some(*entry);
        match link {
            Link::Symlink(target) => {
                w.u8(LINK_SYMLINK);
                w.varint(target.len() as u64);
                w.raw(target);
            }
            Link::HardLink { source } => {
                w.u8(LINK_HARD);
                w.varint((entry - source) as u64);
            }
        }
    }
    Extension {
        tag: TAG_LINKS,
        data: w.0,
    }
}

/// Parse a link table for an index of `entry_count` entries (syntax and ranges only; the
/// entries themselves are checked in `derive_layout`).
fn parse_links(data: &[u8], entry_count: usize) -> Result<Vec<(usize, Link)>> {
    let bad = |what: &str| Error::corrupt(format!("link table: {what}"));
    let mut r = Reader::new(data);
    let count = r.varint()?;
    if count > (r.remaining() / 3) as u64 {
        return Err(bad("larger than its data"));
    }
    let mut out = Vec::with_capacity(count as usize);
    let mut previous: Option<u64> = None;
    for _ in 0..count {
        let step = r.varint()?;
        let entry = match previous {
            None => step,
            Some(p) => p
                .checked_add(step)
                .filter(|_| step > 0)
                .ok_or_else(|| bad("entries are not ascending"))?,
        };
        if entry >= entry_count as u64 {
            return Err(bad("names a bad entry"));
        }
        let link = match r.u8()? {
            LINK_SYMLINK => {
                let len = r.varint()?;
                if len == 0 || len > MAX_LINK_TARGET as u64 || len > r.remaining() as u64 {
                    return Err(bad("symlink target has an invalid length"));
                }
                let target = r.take(len as usize)?;
                if target.contains(&0) {
                    return Err(bad("symlink target contains NUL"));
                }
                Link::Symlink(target.to_vec())
            }
            LINK_HARD => {
                let back = r.varint()?;
                if back == 0 || back > entry {
                    return Err(bad("hard link names a bad source"));
                }
                Link::HardLink {
                    source: (entry - back) as usize,
                }
            }
            _ => return Err(bad("unknown link kind")),
        };
        previous = Some(entry);
        out.push((entry as usize, link));
    }
    if r.remaining() != 0 {
        return Err(bad("trailing bytes"));
    }
    Ok(out)
}

/// The owner table: one `(uid, gid)` per entry, in entry order. Stored as a small table of the
/// distinct pairs plus one table index per entry, which compresses to almost nothing.
pub fn owner_extension(owners: &[(u32, u32)]) -> Extension {
    let mut pairs: Vec<(u32, u32)> = owners.to_vec();
    pairs.sort_unstable();
    pairs.dedup();
    let mut w = Writer::default();
    w.varint(pairs.len() as u64);
    for &(uid, gid) in &pairs {
        w.varint(u64::from(uid));
        w.varint(u64::from(gid));
    }
    for owner in owners {
        let at = pairs
            .binary_search(owner)
            .expect("pair was collected above");
        w.varint(at as u64);
    }
    Extension {
        tag: TAG_OWNER,
        data: w.0,
    }
}

/// Parse an owner table for an index of `entry_count` entries.
fn parse_owners(data: &[u8], entry_count: usize) -> Result<Vec<(u32, u32)>> {
    let bad = |what: &str| Error::corrupt(format!("owner table: {what}"));
    let mut r = Reader::new(data);
    let pair_count = r.varint()?;
    if pair_count > (r.remaining() / 2) as u64 {
        return Err(bad("larger than its data"));
    }
    let mut pairs = Vec::with_capacity(pair_count as usize);
    for _ in 0..pair_count {
        let uid = u32::try_from(r.varint()?).map_err(|_| bad("user id out of range"))?;
        let gid = u32::try_from(r.varint()?).map_err(|_| bad("group id out of range"))?;
        pairs.push((uid, gid));
    }
    if entry_count > r.remaining() {
        return Err(bad("has fewer rows than the archive has entries"));
    }
    let mut owners = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        let at = r.varint()?;
        owners.push(
            *usize::try_from(at)
                .ok()
                .and_then(|at| pairs.get(at))
                .ok_or_else(|| bad("names a missing owner"))?,
        );
    }
    if r.remaining() != 0 {
        return Err(bad("trailing bytes"));
    }
    Ok(owners)
}

/// The extended-attribute table: `(entry, attributes)`, ascending by entry, no empty lists.
pub fn xattr_extension(records: &[(usize, Vec<Xattr>)]) -> Extension {
    let mut w = Writer::default();
    w.varint(records.len() as u64);
    let mut previous: Option<usize> = None;
    for (entry, attrs) in records {
        w.varint((entry - previous.unwrap_or(0)) as u64);
        previous = Some(*entry);
        w.varint(attrs.len() as u64);
        for a in attrs {
            w.varint(a.name.len() as u64);
            w.raw(&a.name);
            w.varint(a.value.len() as u64);
            w.raw(&a.value);
        }
    }
    Extension {
        tag: TAG_XATTR,
        data: w.0,
    }
}

/// Parse an extended-attribute table for an index of `entry_count` entries.
fn parse_xattrs(data: &[u8], entry_count: usize) -> Result<Vec<(usize, Vec<Xattr>)>> {
    let bad = |what: &str| Error::corrupt(format!("extended-attribute table: {what}"));
    let mut r = Reader::new(data);
    let count = r.varint()?;
    if count > (r.remaining() / 4) as u64 {
        return Err(bad("larger than its data"));
    }
    let mut out = Vec::with_capacity(count as usize);
    let mut previous: Option<u64> = None;
    for _ in 0..count {
        let step = r.varint()?;
        let entry = match previous {
            None => step,
            Some(p) => p
                .checked_add(step)
                .filter(|_| step > 0)
                .ok_or_else(|| bad("entries are not ascending"))?,
        };
        if entry >= entry_count as u64 {
            return Err(bad("names a bad entry"));
        }
        let n = r.varint()?;
        if n == 0 || n > MAX_XATTRS_PER_ENTRY as u64 || n > (r.remaining() / 2) as u64 {
            return Err(bad("an entry has an invalid attribute count"));
        }
        let mut attrs = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let name_len = r.varint()?;
            if name_len == 0 || name_len > MAX_XATTR_NAME as u64 || name_len > r.remaining() as u64
            {
                return Err(bad("attribute name has an invalid length"));
            }
            let name = r.take(name_len as usize)?.to_vec();
            if name.contains(&0) {
                return Err(bad("attribute name contains NUL"));
            }
            let value_len = r.varint()?;
            if value_len > MAX_XATTR_VALUE as u64 || value_len > r.remaining() as u64 {
                return Err(bad("attribute value has an invalid length"));
            }
            let value = r.take(value_len as usize)?.to_vec();
            attrs.push(Xattr { name, value });
        }
        previous = Some(entry);
        out.push((entry as usize, attrs));
    }
    if r.remaining() != 0 {
        return Err(bad("trailing bytes"));
    }
    Ok(out)
}

/// How an entry sits in the data stream while the layout is derived.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Own {
    /// A directory or a file with data of its own (possibly zero bytes).
    Data,
    /// A deduplicated copy.
    Dup,
    /// A symbolic link.
    Symlink,
    /// A hard link.
    Hard,
}

fn flags_of(e: &Entry) -> u8 {
    u8::from(e.kind == EntryKind::Dir) | (e.class.id() << 1)
}

impl Index {
    /// Assemble an index from parts and derive its layout (`chunk_offsets`, `entry_starts`, ...).
    /// Fails if the parts are inconsistent (sizes do not add up, arithmetic overflows).
    pub fn build(
        entries: Vec<Entry>,
        chunks: Vec<ChunkInfo>,
        extensions: Vec<Extension>,
    ) -> Result<Self> {
        let mut index = Self {
            entries,
            chunks,
            extensions,
            ..Self::default()
        };
        index.derive_layout()?;
        Ok(index)
    }

    /// Compute every derived field and check that entries and chunks describe the same stream.
    fn derive_layout(&mut self) -> Result<()> {
        let dups = self.dedup_pairs()?;
        let n = self.entries.len();
        let links = match self.single_extension(TAG_LINKS, "link table")? {
            Some(x) => parse_links(&x.data, n)?,
            None => Vec::new(),
        };
        let owners = match self.single_extension(TAG_OWNER, "owner table")? {
            Some(x) => parse_owners(&x.data, n)?,
            None => Vec::new(),
        };
        let xattrs = match self.single_extension(TAG_XATTR, "extended-attribute table")? {
            Some(x) => parse_xattrs(&x.data, n)?,
            None => Vec::new(),
        };
        let empty = empty_hash();
        let mut own = vec![Own::Data; n];
        let mut next_dup = 0usize;
        let mut next_link = 0usize;
        let mut offset = 0u64;
        self.entry_starts = Vec::with_capacity(n);
        for (i, e) in self.entries.iter().enumerate() {
            let dup = match dups.get(next_dup) {
                Some(&(dup, source)) if dup == i => Some(source),
                _ => None,
            };
            let link = match links.get(next_link) {
                Some((at, link)) if *at == i => Some(link),
                _ => None,
            };
            next_dup += usize::from(dup.is_some());
            next_link += usize::from(link.is_some());
            match (dup, link) {
                (Some(_), Some(_)) => {
                    return Err(Error::corrupt(format!(
                        "{}: entry is both deduplicated and a link",
                        e.path
                    )));
                }
                (Some(source), None) => {
                    // A duplicate owns no bytes: it shares its source's region of the stream.
                    let s = &self.entries[source];
                    if e.kind != EntryKind::File
                        || s.kind != EntryKind::File
                        || e.size == 0
                        || e.size != s.size
                        || e.hash != s.hash
                        || own[source] != Own::Data
                    {
                        return Err(Error::corrupt(format!(
                            "{}: invalid deduplication reference",
                            e.path
                        )));
                    }
                    own[i] = Own::Dup;
                    let start = self.entry_starts[source];
                    self.entry_starts.push(start);
                }
                (None, Some(Link::Symlink(_))) => {
                    if e.kind != EntryKind::File
                        || e.size != 0
                        || e.hash != empty
                        || e.class != Class::Unknown
                    {
                        return Err(Error::corrupt(format!(
                            "{}: invalid symbolic link entry",
                            e.path
                        )));
                    }
                    own[i] = Own::Symlink;
                    self.entry_starts.push(offset);
                }
                (None, Some(&Link::HardLink { source })) => {
                    // Like a duplicate, a hard link owns no bytes and shares its source's region.
                    // Its source may itself be a duplicate, but not a link.
                    let s = &self.entries[source];
                    if e.kind != EntryKind::File
                        || s.kind != EntryKind::File
                        || e.size != s.size
                        || e.hash != s.hash
                        || matches!(own[source], Own::Symlink | Own::Hard)
                    {
                        return Err(Error::corrupt(format!(
                            "{}: invalid hard link reference",
                            e.path
                        )));
                    }
                    own[i] = Own::Hard;
                    let start = self.entry_starts[source];
                    self.entry_starts.push(start);
                }
                (None, None) => {
                    self.entry_starts.push(offset);
                    offset = offset
                        .checked_add(e.size)
                        .ok_or_else(|| Error::corrupt("total size overflows"))?;
                }
            }
        }
        // Nothing may be stored below a symbolic link: extraction would have to write through
        // it. (This is what keeps an archive from planting `a -> /etc` and then `a/passwd`.)
        if links.iter().any(|(_, l)| matches!(l, Link::Symlink(_))) {
            let mut names: Vec<&str> = self.entries.iter().map(|e| e.path.as_str()).collect();
            names.sort_unstable();
            for (at, link) in &links {
                if !matches!(link, Link::Symlink(_)) {
                    continue;
                }
                let mut prefix = self.entries[*at].path.clone();
                prefix.push('/');
                let first = names.partition_point(|name| *name < prefix.as_str());
                if names
                    .get(first)
                    .is_some_and(|name| name.starts_with(&prefix))
                {
                    return Err(Error::corrupt(format!(
                        "{}: entries are stored below a symbolic link",
                        self.entries[*at].path
                    )));
                }
            }
        }
        self.links = links;
        self.owners = owners;
        self.xattrs = xattrs;
        self.total_raw = offset;

        let mut raw = 0u64;
        let mut pos = HEADER_LEN as u64;
        self.chunk_starts = Vec::with_capacity(self.chunks.len() + 1);
        self.chunk_offsets = Vec::with_capacity(self.chunks.len());
        for c in &self.chunks {
            self.chunk_starts.push(raw);
            self.chunk_offsets.push(pos);
            raw += u64::from(c.raw_len); // at most 2^32 per chunk and < 2^40 chunks: cannot wrap
            pos = pos
                .checked_add(RECORD_HEADER_LEN as u64 + u64::from(c.comp_len))
                .ok_or_else(|| Error::corrupt("archive size overflows"))?;
        }
        self.chunk_starts.push(raw);
        self.data_end = pos;
        if raw != self.total_raw {
            return Err(Error::corrupt(
                "chunk sizes do not add up to the entry sizes",
            ));
        }
        Ok(())
    }

    /// The `(duplicate, source)` entry pairs of the deduplication table, ascending by
    /// duplicate (empty without one). Only the syntax is checked here; `derive_layout` checks
    /// the pairs against the entries.
    fn dedup_pairs(&self) -> Result<Vec<(usize, usize)>> {
        let mut found = self.extensions.iter().filter(|x| x.tag == TAG_DEDUP);
        let Some(x) = found.next() else {
            return Ok(Vec::new());
        };
        if found.next().is_some() {
            return Err(Error::corrupt("more than one deduplication table"));
        }
        parse_dedup(&x.data, self.entries.len())
    }

    /// The one extension with `tag`; more than one is corrupt.
    fn single_extension(&self, tag: u16, what: &str) -> Result<Option<&Extension>> {
        let mut found = self.extensions.iter().filter(|x| x.tag == tag);
        let first = found.next();
        if found.next().is_some() {
            return Err(Error::corrupt(format!("more than one {what}")));
        }
        Ok(first)
    }

    /// The link recorded for entry `i`, if it is a symbolic or hard link.
    pub fn link(&self, i: usize) -> Option<&Link> {
        self.links
            .binary_search_by_key(&i, |(at, _)| *at)
            .ok()
            .map(|k| &self.links[k].1)
    }

    /// The target of entry `i` if it is a symbolic link.
    pub fn symlink_target(&self, i: usize) -> Option<&[u8]> {
        match self.link(i) {
            Some(Link::Symlink(target)) => Some(target),
            _ => None,
        }
    }

    /// The entry that entry `i` is a hard link to, if it is one.
    pub fn hard_link_source(&self, i: usize) -> Option<usize> {
        match self.link(i) {
            Some(&Link::HardLink { source }) => Some(source),
            _ => None,
        }
    }

    /// True if entry `i` is a symbolic link.
    pub fn is_symlink(&self, i: usize) -> bool {
        self.symlink_target(i).is_some()
    }

    /// The recorded `(uid, gid)` of entry `i`, if the archive stores ownership.
    pub fn owner(&self, i: usize) -> Option<(u32, u32)> {
        self.owners.get(i).copied()
    }

    /// The extended attributes recorded for entry `i` (empty if none).
    pub fn xattrs_of(&self, i: usize) -> &[Xattr] {
        self.xattrs
            .binary_search_by_key(&i, |(at, _)| *at)
            .ok()
            .map_or(&[][..], |k| &self.xattrs[k].1)
    }

    /// The writer's creator string, if the archive carries one.
    pub fn creator(&self) -> Option<&str> {
        self.extensions
            .iter()
            .find(|e| e.tag == TAG_CREATOR)
            .and_then(|e| str::from_utf8(&e.data).ok())
    }

    /// Serialise the index.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.u8(INDEX_VERSION);
        w.varint(self.entries.len() as u64);
        w.varint(self.chunks.len() as u64);
        for e in &self.entries {
            w.u8(flags_of(e));
        }
        for e in &self.entries {
            w.varint(e.path.len() as u64);
        }
        for e in &self.entries {
            w.raw(e.path.as_bytes());
        }
        for e in self.entries.iter().filter(|e| e.kind == EntryKind::File) {
            w.varint(e.size);
        }
        let mut previous = 0i64;
        for e in &self.entries {
            w.svarint(e.mtime_secs.wrapping_sub(previous));
            previous = e.mtime_secs;
        }
        for e in &self.entries {
            w.varint(u64::from(e.mtime_nanos));
        }
        for e in &self.entries {
            w.varint(u64::from(e.mode));
        }
        for e in self.entries.iter().filter(|e| e.kind == EntryKind::File) {
            w.raw(&e.hash);
        }
        for c in &self.chunks {
            w.varint(u64::from(c.raw_len));
        }
        for c in &self.chunks {
            w.varint(u64::from(c.comp_len));
        }
        for c in &self.chunks {
            w.u8(c.method.id());
        }
        for x in &self.extensions {
            w.raw(&x.tag.to_le_bytes());
            w.varint(x.data.len() as u64);
            w.raw(&x.data);
        }
        w.0
    }

    /// Parse and fully validate an index.
    ///
    /// `block_size` is the archive's maximum chunk size and `data_end` the offset where chunk
    /// records must stop (the start of the index record). Every count is checked against the
    /// bytes actually present before anything is allocated, and every cross-reference is
    /// verified, so a hostile index cannot cause large allocations, overflow, or out-of-range
    /// reads later.
    pub fn decode(raw: &[u8], block_size: u64, data_end: u64) -> Result<Self> {
        let mut r = Reader::new(raw);
        let version = r.u8()?;
        if version != INDEX_VERSION {
            return Err(Error::Unsupported(format!(
                "index layout version {version}"
            )));
        }
        let entry_count = r.varint()?;
        let chunk_count = r.varint()?;
        if entry_count > MAX_ENTRIES {
            return Err(Error::corrupt(format!(
                "archive claims {entry_count} entries (limit {MAX_ENTRIES})"
            )));
        }
        let entry_count = entry_count as usize;
        if entry_count > r.remaining() / MIN_ENTRY_BYTES {
            return Err(Error::corrupt("entry count exceeds the index size"));
        }
        if chunk_count > (r.remaining() / MIN_CHUNK_BYTES) as u64 {
            return Err(Error::corrupt("chunk count exceeds the index size"));
        }
        let chunk_count = chunk_count as usize;

        let mut kinds = Vec::with_capacity(entry_count);
        let mut classes = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let f = r.u8()?;
            if f & 0xF0 != 0 {
                return Err(Error::corrupt("reserved entry flag bits are set"));
            }
            kinds.push(if f & 1 == 1 {
                EntryKind::Dir
            } else {
                EntryKind::File
            });
            classes.push(Class::from_id((f >> 1) & 7)?);
        }
        let mut path_lens = Vec::with_capacity(entry_count);
        let mut path_total = 0usize;
        for _ in 0..entry_count {
            let len = r.varint()?;
            if len == 0 || len > MAX_PATH_LEN as u64 {
                return Err(Error::corrupt("entry name has an invalid length"));
            }
            path_total += len as usize; // <= MAX_ENTRIES * 32 KiB: cannot overflow usize
            path_lens.push(len as usize);
        }
        if path_total > r.remaining() {
            return Err(Error::corrupt("entry names extend past the index"));
        }
        let mut paths = Vec::with_capacity(entry_count);
        for &len in &path_lens {
            let name = str::from_utf8(r.take(len)?)
                .map_err(|_| Error::corrupt("entry name is not valid UTF-8"))?;
            validate_archive_path(name)?;
            paths.push(name.to_owned());
        }
        let file_count = kinds.iter().filter(|k| **k == EntryKind::File).count();
        if file_count > r.remaining() / (1 + HASH_LEN) {
            return Err(Error::corrupt("file count exceeds the index size"));
        }
        let mut sizes = Vec::with_capacity(entry_count);
        for &k in &kinds {
            sizes.push(if k == EntryKind::File { r.varint()? } else { 0 });
        }
        let mut mtimes = Vec::with_capacity(entry_count);
        let mut previous = 0i64;
        for _ in 0..entry_count {
            previous = previous.wrapping_add(r.svarint()?);
            mtimes.push(previous);
        }
        let mut nanos = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let n = r.varint()?;
            if n >= 1_000_000_000 {
                return Err(Error::corrupt("invalid timestamp"));
            }
            nanos.push(n as u32);
        }
        let mut modes = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let m = r.varint()?;
            if m > 0o7777 {
                return Err(Error::corrupt("invalid permission bits"));
            }
            modes.push(m as u32);
        }

        let mut entries = Vec::with_capacity(entry_count);
        for (i, path) in paths.into_iter().enumerate() {
            let hash = if kinds[i] == EntryKind::File {
                r.bytes::<HASH_LEN>()?
            } else {
                [0; HASH_LEN]
            };
            if kinds[i] == EntryKind::Dir && classes[i] != Class::Unknown {
                return Err(Error::corrupt(format!("{path}: directory with a class")));
            }
            entries.push(Entry {
                path,
                kind: kinds[i],
                class: classes[i],
                size: sizes[i],
                mtime_secs: mtimes[i],
                mtime_nanos: nanos[i],
                mode: modes[i],
                hash,
            });
        }

        let mut raw_lens = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            let v = r.varint()?;
            if v == 0 || v > block_size {
                return Err(Error::corrupt("chunk size is out of range"));
            }
            raw_lens.push(v as u32);
        }
        let mut comp_lens = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            let v = r.varint()?;
            if v > u64::from(u32::MAX) {
                return Err(Error::corrupt("chunk payload size is out of range"));
            }
            comp_lens.push(v as u32);
        }
        let mut chunks = Vec::with_capacity(chunk_count);
        for i in 0..chunk_count {
            let id = r.u8()?;
            let method = Method::from_id(id)
                .ok_or_else(|| Error::Unsupported(format!("unknown compression method {id}")))?;
            let (raw_len, comp_len) = (raw_lens[i], comp_lens[i]);
            if comp_len == 0 || comp_len > raw_len {
                return Err(Error::corrupt(format!("chunk {i} has an impossible size")));
            }
            if method == Method::Store && comp_len != raw_len {
                return Err(Error::corrupt(format!(
                    "stored chunk {i} has inconsistent lengths"
                )));
            }
            chunks.push(ChunkInfo {
                raw_len,
                comp_len,
                method,
            });
        }

        let mut extensions = Vec::new();
        while r.remaining() > 0 {
            let tag = u16::from_le_bytes(r.bytes::<2>()?);
            let len = r.varint()?;
            if len > r.remaining() as u64 {
                return Err(Error::corrupt("extension extends past the index"));
            }
            let data = r.take(len as usize)?.to_vec();
            match tag {
                TAG_CREATOR => {
                    if data.len() > MAX_CREATOR_LEN || str::from_utf8(&data).is_err() {
                        return Err(Error::corrupt("invalid creator extension"));
                    }
                }
                TAG_DEDUP => {
                    parse_dedup(&data, entry_count)?; // the entries are checked in `derive_layout`
                }
                TAG_LINKS => {
                    parse_links(&data, entry_count)?;
                }
                TAG_OWNER => {
                    parse_owners(&data, entry_count)?;
                }
                TAG_XATTR => {
                    parse_xattrs(&data, entry_count)?;
                }
                t if t & TAG_CRITICAL != 0 => {
                    return Err(Error::Unsupported(format!(
                        "archive needs extension {t:#06x} which this build does not know"
                    )));
                }
                _ => {} // unknown, non-critical: kept, otherwise ignored
            }
            extensions.push(Extension { tag, data });
        }

        let mut names: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        names.sort_unstable();
        if let Some(w) = names.windows(2).find(|w| w[0] == w[1]) {
            return Err(Error::corrupt(format!("duplicate entry {:?}", w[0])));
        }
        let mut index = Self {
            entries,
            chunks,
            extensions,
            ..Self::default()
        };
        index.derive_layout()?;
        if index.data_end != data_end {
            return Err(Error::corrupt(
                "chunk records do not end where the index begins",
            ));
        }
        Ok(index)
    }

    /// Index of the chunk that holds stream byte `offset` (which must be below `total_raw`).
    pub fn chunk_containing(&self, offset: u64) -> usize {
        self.chunk_starts.partition_point(|&s| s <= offset) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity::empty_hash;

    const BS: u64 = 4096;

    fn file(path: &str, size: u64) -> Entry {
        Entry {
            path: path.into(),
            kind: EntryKind::File,
            class: Class::Text,
            size,
            mtime_secs: 1_700_000_000,
            mtime_nanos: 5,
            mode: 0o644,
            hash: [9; HASH_LEN],
        }
    }

    fn dir(path: &str) -> Entry {
        Entry {
            path: path.into(),
            kind: EntryKind::Dir,
            class: Class::Unknown,
            size: 0,
            mtime_secs: -5,
            mtime_nanos: 0,
            mode: 0o755,
            hash: [0; HASH_LEN],
        }
    }

    fn chunk(raw_len: u32, comp_len: u32, method: Method) -> ChunkInfo {
        ChunkInfo {
            raw_len,
            comp_len,
            method,
        }
    }

    /// 5000 + 3000 = 8000 bytes in three uneven chunks.
    fn sample() -> Index {
        Index::build(
            vec![
                dir("d"),
                file("d/a.txt", 5000),
                file("empty", 0),
                file("z", 3000),
            ],
            vec![
                chunk(4096, 1000, Method::Zstd),
                chunk(904, 904, Method::Store),
                chunk(3000, 700, Method::Lz4),
            ],
            vec![Extension {
                tag: TAG_CREATOR,
                data: b"kiln test".to_vec(),
            }],
        )
        .unwrap()
    }

    fn data_end(idx: &Index) -> u64 {
        idx.data_end
    }

    #[test]
    fn round_trip_and_derived_layout() {
        let idx = sample();
        let back = Index::decode(&idx.encode(), BS, data_end(&idx)).unwrap();
        assert_eq!(back, idx);
        assert_eq!(back.entry_starts, vec![0, 0, 5000, 5000]);
        assert_eq!(back.chunk_starts, vec![0, 4096, 5000, 8000]);
        assert_eq!(back.chunk_offsets[0], HEADER_LEN as u64);
        assert_eq!(back.chunk_offsets[1], HEADER_LEN as u64 + 13 + 1000);
        assert_eq!(back.creator(), Some("kiln test"));
        assert_eq!(back.chunk_containing(0), 0);
        assert_eq!(back.chunk_containing(4095), 0);
        assert_eq!(back.chunk_containing(4096), 1);
        assert_eq!(back.chunk_containing(7999), 2);
    }

    #[test]
    fn empty_index_round_trips() {
        let idx = Index::build(vec![], vec![], vec![]).unwrap();
        assert_eq!(
            Index::decode(&idx.encode(), BS, HEADER_LEN as u64).unwrap(),
            idx
        );
        let only_empty = Index::build(
            vec![Entry {
                hash: empty_hash(),
                ..file("e", 0)
            }],
            vec![],
            vec![],
        )
        .unwrap();
        assert_eq!(
            Index::decode(&only_empty.encode(), BS, HEADER_LEN as u64).unwrap(),
            only_empty
        );
    }

    #[test]
    fn every_truncation_is_an_error_not_a_panic() {
        let idx = sample();
        let bytes = idx.encode();
        for cut in 0..bytes.len() {
            // A cut inside the extension area leaves a still-valid index without that
            // extension only if it lands exactly on the boundary before it; every other prefix
            // must fail.
            let r = Index::decode(&bytes[..cut], BS, data_end(&idx));
            if let Ok(i) = r {
                assert!(i.extensions.is_empty(), "prefix of {cut} bytes");
            }
        }
        assert!(Index::decode(&bytes[..bytes.len() - 1], BS, data_end(&idx)).is_err());
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(Index::decode(&extra, BS, data_end(&idx)).is_err());
    }

    #[test]
    fn every_single_byte_corruption_is_survived() {
        // Not all flips are detectable at this layer (e.g. a hash byte), but none may panic.
        let idx = sample();
        let bytes = idx.encode();
        for i in 0..bytes.len() {
            for flip in [0x01u8, 0x80, 0xFF] {
                let mut b = bytes.clone();
                b[i] ^= flip;
                let _ = Index::decode(&b, BS, data_end(&idx));
            }
        }
    }

    #[test]
    fn rejects_bad_semantics() {
        let ok = sample();
        let de = data_end(&ok);
        let enc = |idx: &Index| Index::decode(&idx.encode(), BS, de);
        // Build bypassing the cross-checks so the decoder is what gets tested.
        let raw = |f: &dyn Fn(&mut Index)| {
            let mut i = ok.clone();
            f(&mut i);
            i
        };

        assert!(matches!(
            enc(&raw(&|i| i.entries[1].path = "../evil".into())),
            Err(Error::UnsafePath(_))
        ));
        assert!(matches!(
            enc(&raw(&|i| i.entries[1].path = "/abs".into())),
            Err(Error::UnsafePath(_))
        ));
        assert!(matches!(
            enc(&raw(&|i| i.entries[3].path = "empty".into())),
            Err(Error::Corrupt(ref m)) if m.contains("duplicate")
        ));
        assert!(
            enc(&raw(&|i| i.entries[1].size += 1)).is_err(),
            "sizes must add up"
        );
        assert!(
            enc(&raw(&|i| {
                i.chunks.pop();
            }))
            .is_err(),
            "chunks must cover the stream"
        );
        assert!(
            enc(&raw(&|i| i.chunks[0].raw_len = 5000)).is_err(),
            "chunk above the maximum"
        );
        assert!(
            enc(&raw(&|i| i.chunks[0].raw_len = 0)).is_err(),
            "empty chunk"
        );
        assert!(
            enc(&raw(&|i| i.chunks[0].comp_len = 5000)).is_err(),
            "payload above raw"
        );
        assert!(
            enc(&raw(&|i| i.chunks[1].comp_len = 800)).is_err(),
            "inconsistent stored chunk"
        );
        assert!(
            enc(&raw(&|i| i.entries[0].class = Class::Text)).is_err(),
            "directory with a class"
        );
        assert!(enc(&raw(&|i| i.entries[1].mtime_nanos = 1_000_000_000)).is_err());
        assert!(enc(&raw(&|i| i.entries[1].mode = 0o10000)).is_err());
        // Chunk records must end exactly where the index begins.
        assert!(Index::decode(&ok.encode(), BS, de + 1).is_err());
        assert!(Index::decode(&ok.encode(), BS, de - 1).is_err());
    }

    #[test]
    fn extensions_unknown_non_critical_are_skipped_critical_are_refused() {
        let mut idx = sample();
        idx.extensions.push(Extension {
            tag: 0x0042,
            data: vec![1, 2, 3],
        });
        let de = data_end(&idx);
        let back = Index::decode(&idx.encode(), BS, de).unwrap();
        assert_eq!(
            back.extensions.len(),
            2,
            "unknown non-critical kept, not fatal"
        );
        assert_eq!(back.creator(), Some("kiln test"));

        idx.extensions.push(Extension {
            tag: TAG_CRITICAL | 0x0007,
            data: vec![],
        });
        assert!(matches!(
            Index::decode(&idx.encode(), BS, de),
            Err(Error::Unsupported(_))
        ));
        let mut bad = sample();
        bad.extensions[0].data = vec![0xFF, 0xFE];
        assert!(
            Index::decode(&bad.encode(), BS, de).is_err(),
            "creator must be UTF-8"
        );
    }

    #[test]
    fn lying_counts_do_not_cause_huge_allocations() {
        // 4 billion entries claimed in a handful of bytes.
        let mut w = Writer::default();
        w.u8(INDEX_VERSION);
        w.varint(u64::from(u32::MAX));
        w.varint(0);
        w.raw(&[0; 8]);
        assert!(Index::decode(&w.0, BS, HEADER_LEN as u64).is_err());
        // More entries than the format allows, even if the bytes were there.
        let mut w = Writer::default();
        w.u8(INDEX_VERSION);
        w.varint(MAX_ENTRIES + 1);
        w.varint(0);
        w.raw(&vec![0u8; 8 * (MAX_ENTRIES as usize + 1)]);
        assert!(matches!(
            Index::decode(&w.0, BS, HEADER_LEN as u64),
            Err(Error::Corrupt(ref m)) if m.contains("limit")
        ));
        // Claimed chunk count far beyond the bytes present.
        let mut w = Writer::default();
        w.u8(INDEX_VERSION);
        w.varint(0);
        w.varint(u64::MAX >> 4);
        assert!(Index::decode(&w.0, BS, HEADER_LEN as u64).is_err());
        // A path length of 4 GiB.
        let mut w = Writer::default();
        w.u8(INDEX_VERSION);
        w.varint(1);
        w.varint(0);
        w.u8(0);
        w.varint(u64::from(u32::MAX));
        w.raw(&[0; 40]);
        assert!(Index::decode(&w.0, BS, HEADER_LEN as u64).is_err());
    }

    #[test]
    fn unknown_index_version_is_unsupported() {
        let idx = sample();
        let mut b = idx.encode();
        b[0] = 9;
        assert!(matches!(
            Index::decode(&b, BS, data_end(&idx)),
            Err(Error::Unsupported(_))
        ));
    }
}
