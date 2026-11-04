//! Opening an existing archive: validate the header and footer, then load the index.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::classify::Class;
use crate::codec::Method;
use crate::error::{Error, IoContext, Result};
use crate::format::index::{Entry, EntryKind, Index};
use crate::format::record::{decode_payload, read_record};
use crate::format::{FOOTER_LEN, Footer, HEADER_LEN, Header, MAGIC, MAX_INDEX_RAW};
use crate::fsutil::read_full;

/// How the data chunks of an archive are encoded (taken from the index; no data is read).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockStats {
    /// Total data chunks.
    pub total: u64,
    /// Chunks stored verbatim (incompressible, classified as such, or a `store` archive).
    pub stored: u64,
    /// Chunks compressed with LZ4.
    pub lz4: u64,
    /// Chunks compressed with Zstandard.
    pub zstd: u64,
    /// Chunks compressed with LZMA2.
    pub lzma: u64,
    /// Bytes of chunk payload as stored (record headers excluded).
    pub payload_bytes: u64,
}

/// Files and bytes of one content class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClassTotals {
    /// Number of files.
    pub files: u64,
    /// Their total size in bytes.
    pub bytes: u64,
}

/// An opened archive. Opening reads only the header, footer and index - never the data chunks -
/// so listing even a huge archive is instant.
#[derive(Debug)]
pub struct Archive {
    pub(crate) path: PathBuf,
    pub(crate) header: Header,
    pub(crate) index: Index,
    pub(crate) file_len: u64,
}

impl Archive {
    /// Open and validate `path`.
    ///
    /// Behaviour for damaged input is part of the format contract (`docs/FORMAT.md`): a file that
    /// does not start with the magic is [`Error::NotAnArchive`]; a newer format version or
    /// unknown required feature is [`Error::Unsupported`]; a file that is too short, has no valid
    /// footer, or whose index fails any check is [`Error::Corrupt`] or [`Error::Checksum`].
    /// Nothing is ever partially accepted.
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path).at(path)?;
        let file_len = file.metadata().at(path)?.len();

        let mut head = [0u8; HEADER_LEN];
        let got = read_full(&mut file, &mut head).at(path)?;
        if got < MAGIC.len() || head[..MAGIC.len()] != MAGIC {
            return Err(Error::NotAnArchive("bad magic bytes"));
        }
        if got < HEADER_LEN || file_len < (HEADER_LEN + FOOTER_LEN) as u64 {
            return Err(Error::corrupt("archive is truncated"));
        }
        let header = Header::decode(&head)?;

        file.seek(SeekFrom::Start(file_len - FOOTER_LEN as u64))
            .at(path)?;
        let mut foot = [0u8; FOOTER_LEN];
        file.read_exact(&mut foot).at(path)?;
        let footer = Footer::decode(&foot)?;

        let data_end = footer.index_offset;
        let footer_start = file_len - FOOTER_LEN as u64;
        if data_end < HEADER_LEN as u64
            || data_end.checked_add(footer.index_record_len) != Some(footer_start)
        {
            return Err(Error::corrupt("footer points outside the archive"));
        }

        file.seek(SeekFrom::Start(data_end)).at(path)?;
        let (record, payload) = read_record(&mut file, MAX_INDEX_RAW, footer.index_record_len)?;
        if record.total_len() != footer.index_record_len {
            return Err(Error::corrupt(
                "index record length disagrees with the footer",
            ));
        }
        let raw = decode_payload(&record, payload)
            .map_err(|e| Error::corrupt(format!("index is damaged: {e}")))?;
        let index = Index::decode(&raw, header.block_size(), data_end)?;
        Ok(Self {
            path: path.to_path_buf(),
            header,
            index,
            file_len,
        })
    }

    /// Count how the chunks are encoded, from the index alone.
    pub fn block_stats(&self) -> BlockStats {
        let mut stats = BlockStats::default();
        for c in &self.index.chunks {
            stats.total += 1;
            stats.payload_bytes += u64::from(c.comp_len);
            match c.method {
                Method::Store => stats.stored += 1,
                Method::Lz4 => stats.lz4 += 1,
                Method::Zstd => stats.zstd += 1,
                Method::Lzma2 => stats.lzma += 1,
            }
        }
        stats
    }

    /// File counts and bytes per content class (all [`Class::Unknown`] for profiles that do not
    /// classify).
    pub fn class_totals(&self) -> Vec<(Class, ClassTotals)> {
        let mut totals = [ClassTotals::default(); 6];
        for e in self
            .index
            .entries
            .iter()
            .filter(|e| e.kind == EntryKind::File)
        {
            let t = &mut totals[e.class.id() as usize];
            t.files += 1;
            t.bytes += e.size;
        }
        (0..6u8)
            .filter_map(|id| {
                let t = totals[id as usize];
                (t.files > 0).then(|| (Class::from_id(id).expect("id below 6"), t))
            })
            .collect()
    }

    /// The method of the chunk that holds the first byte of entry `i` (`None` for empty files and
    /// directories). This is the file's "compression metadata": which codec its data went through.
    pub fn entry_method(&self, i: usize) -> Option<Method> {
        let e = &self.index.entries[i];
        (e.kind == EntryKind::File && e.size > 0).then(|| {
            self.index.chunks[self.index.chunk_containing(self.index.entry_starts[i])].method
        })
    }

    /// Where the archive lives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The archive header (profile and chunk size).
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Every entry, in stream order.
    pub fn entries(&self) -> &[Entry] {
        &self.index.entries
    }

    /// The decoded index.
    pub fn index(&self) -> &Index {
        &self.index
    }

    /// Total uncompressed size of all files.
    pub fn total_raw(&self) -> u64 {
        self.index.total_raw
    }

    /// Number of data chunks.
    pub fn block_count(&self) -> usize {
        self.index.chunks.len()
    }

    /// Size of the archive file in bytes.
    pub fn file_len(&self) -> u64 {
        self.file_len
    }
}
