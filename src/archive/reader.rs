//! Opening an existing archive: validate the header and footer, then load the index.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::codec::Method;
use crate::error::{Error, IoContext, Result};
use crate::format::index::{Entry, Index};
use crate::format::record::{RECORD_HEADER_LEN, RecordHeader, decode_payload, read_record};
use crate::format::{FOOTER_LEN, Footer, HEADER_LEN, Header, MAGIC, MAX_INDEX_RAW};
use crate::fsutil::read_full;

/// How the data blocks of an archive are encoded (from their 13-byte headers only).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockStats {
    /// Total data blocks.
    pub total: u64,
    /// Blocks stored verbatim (incompressible, or a `store` archive).
    pub stored: u64,
    /// Blocks compressed with LZ4.
    pub lz4: u64,
    /// Blocks compressed with Zstandard.
    pub zstd: u64,
    /// Blocks compressed with LZMA2.
    pub lzma: u64,
    /// Bytes of block payload as stored (headers excluded).
    pub payload_bytes: u64,
}

/// An opened archive. Opening reads only the header, footer and index - never the data blocks -
/// so listing even a huge archive is instant.
#[derive(Debug)]
pub struct Archive {
    pub(crate) path: PathBuf,
    pub(crate) header: Header,
    pub(crate) index: Index,
    pub(crate) file_len: u64,
    /// Offset where data blocks end and the index record begins.
    pub(crate) data_end: u64,
}

impl Archive {
    /// Open and validate `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path).at(path)?;
        let file_len = file.metadata().at(path)?.len();

        let mut head = [0u8; HEADER_LEN];
        let got = read_full(&mut file, &mut head).at(path)?;
        if got >= MAGIC.len() && head[..MAGIC.len()] != MAGIC || got < MAGIC.len() {
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
            data_end,
        })
    }

    /// Read every block's 13-byte header (one seek per block; no payload is read) and count how
    /// each block is encoded. Also checks that the blocks tile the data area exactly.
    pub fn block_stats(&self) -> Result<BlockStats> {
        let mut file = File::open(&self.path).at(&self.path)?;
        let offsets = &self.index.block_offsets;
        let mut stats = BlockStats::default();
        for (i, &offset) in offsets.iter().enumerate() {
            let next = offsets.get(i + 1).copied().unwrap_or(self.data_end);
            file.seek(SeekFrom::Start(offset)).at(&self.path)?;
            let mut raw = [0u8; RECORD_HEADER_LEN];
            file.read_exact(&mut raw).at(&self.path)?;
            let header = RecordHeader::decode(&raw, self.header.block_size() as u32)?;
            if offset + header.total_len() != next {
                return Err(Error::corrupt(format!(
                    "block {i} does not end where the next block begins"
                )));
            }
            stats.total += 1;
            stats.payload_bytes += u64::from(header.comp_len);
            match header.method {
                Method::Store => stats.stored += 1,
                Method::Lz4 => stats.lz4 += 1,
                Method::Zstd => stats.zstd += 1,
                Method::Lzma2 => stats.lzma += 1,
            }
        }
        Ok(stats)
    }

    /// Where the archive lives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The archive header (codec and block size).
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Every entry, in stream order.
    pub fn entries(&self) -> &[Entry] {
        &self.index.entries
    }

    /// Total uncompressed size of all files.
    pub fn total_raw(&self) -> u64 {
        self.index.total_raw
    }

    /// Number of data blocks.
    pub fn block_count(&self) -> usize {
        self.index.block_offsets.len()
    }

    /// Size of the archive file in bytes.
    pub fn file_len(&self) -> u64 {
        self.file_len
    }
}
