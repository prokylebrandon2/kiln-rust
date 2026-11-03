//! Block records: the unit of storage for both data blocks and the index.
//!
//! ```text
//! method u8 | raw_len u32 | comp_len u32 | crc32 u32 | payload (comp_len bytes)
//! ```
//!
//! Invariants enforced on read: `raw_len <= max_raw`, `comp_len <= raw_len` (a block that does
//! not shrink is stored, so this bounds every allocation), and stored blocks have
//! `comp_len == raw_len`.

use std::io::{self, Read, Write};

use crate::codec::{BlockCodec, Method, decoder_for};
use crate::error::{Error, Result};
use crate::integrity::crc32;

/// Size of a record header in bytes.
pub const RECORD_HEADER_LEN: usize = 13;

/// The fixed-size header in front of every payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordHeader {
    /// How the payload is encoded.
    pub method: Method,
    /// Length of the data after decoding.
    pub raw_len: u32,
    /// Length of the payload as stored.
    pub comp_len: u32,
    /// CRC-32 of the payload as stored.
    pub crc: u32,
}

impl RecordHeader {
    /// Serialise to the 13 on-disk bytes.
    pub fn encode(&self) -> [u8; RECORD_HEADER_LEN] {
        let mut out = [0u8; RECORD_HEADER_LEN];
        out[0] = self.method.id();
        out[1..5].copy_from_slice(&self.raw_len.to_le_bytes());
        out[5..9].copy_from_slice(&self.comp_len.to_le_bytes());
        out[9..13].copy_from_slice(&self.crc.to_le_bytes());
        out
    }

    /// Parse and validate; `max_raw` is the largest `raw_len` the caller is willing to allocate.
    pub fn decode(bytes: &[u8; RECORD_HEADER_LEN], max_raw: u32) -> Result<Self> {
        let method = Method::from_id(bytes[0]).ok_or_else(|| {
            Error::Unsupported(format!("unknown compression method {}", bytes[0]))
        })?;
        let word = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().expect("4 bytes"));
        let header = Self {
            method,
            raw_len: word(1),
            comp_len: word(5),
            crc: word(9),
        };
        if header.raw_len > max_raw {
            return Err(Error::corrupt(format!(
                "record claims {} bytes, more than the {max_raw} allowed",
                header.raw_len
            )));
        }
        if header.comp_len > header.raw_len {
            return Err(Error::corrupt(
                "record payload is larger than its decoded size",
            ));
        }
        if method == Method::Store && header.comp_len != header.raw_len {
            return Err(Error::corrupt("stored record has inconsistent lengths"));
        }
        Ok(header)
    }

    /// Bytes this record occupies in the archive.
    pub fn total_len(&self) -> u64 {
        RECORD_HEADER_LEN as u64 + u64::from(self.comp_len)
    }
}

/// A fully compressed block, ready to be written.
#[derive(Debug)]
pub struct EncodedBlock {
    /// The header describing the payload.
    pub header: RecordHeader,
    /// The (possibly compressed) bytes.
    pub payload: Vec<u8>,
}

impl EncodedBlock {
    /// Compress `raw` with `codec`, falling back to storing it when compression does not help.
    pub fn from_raw(codec: &dyn BlockCodec, raw: Vec<u8>) -> Result<Self> {
        let raw_len = u32::try_from(raw.len())
            .map_err(|_| Error::Internal("block larger than 4 GiB".into()))?;
        let (method, payload) = if codec.method() == Method::Store || raw.is_empty() {
            (Method::Store, raw)
        } else {
            let packed = codec.compress(&raw)?;
            if packed.len() < raw.len() {
                (codec.method(), packed)
            } else {
                (Method::Store, raw)
            }
        };
        let comp_len = payload.len() as u32;
        let crc = crc32(&payload);
        Ok(Self {
            header: RecordHeader {
                method,
                raw_len,
                comp_len,
                crc,
            },
            payload,
        })
    }

    /// Write header and payload; returns the bytes written.
    pub fn write_to<W: Write>(&self, out: &mut W) -> io::Result<u64> {
        out.write_all(&self.header.encode())?;
        out.write_all(&self.payload)?;
        Ok(self.header.total_len())
    }
}

/// Read one record. `bytes_available` is how many bytes remain in the archive from the current
/// position; a record that claims more is rejected *before* anything is allocated.
pub fn read_record<R: Read>(
    reader: &mut R,
    max_raw: u32,
    bytes_available: u64,
) -> Result<(RecordHeader, Vec<u8>)> {
    let mut head = [0u8; RECORD_HEADER_LEN];
    reader.read_exact(&mut head).map_err(truncated)?;
    let header = RecordHeader::decode(&head, max_raw)?;
    if header.total_len() > bytes_available {
        return Err(Error::corrupt("record extends past the end of the archive"));
    }
    let mut payload = vec![0u8; header.comp_len as usize];
    reader.read_exact(&mut payload).map_err(truncated)?;
    Ok((header, payload))
}

fn truncated(e: io::Error) -> Error {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        Error::corrupt("archive is truncated")
    } else {
        Error::IoBare(e)
    }
}

/// Verify a payload's CRC-32 and decode it to exactly `header.raw_len` bytes.
pub fn decode_payload(header: &RecordHeader, payload: Vec<u8>) -> Result<Vec<u8>> {
    if crc32(&payload) != header.crc {
        return Err(Error::Checksum(
            "block CRC-32 does not match its stored value".into(),
        ));
    }
    match header.method {
        Method::Store => Ok(payload),
        method => decoder_for(method).decompress(&payload, header.raw_len as usize),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::codec::CodecSpec;

    fn sample() -> Vec<u8> {
        (0..50_000u32).map(|i| (i / 40) as u8).collect()
    }

    #[test]
    fn compressible_block_round_trips_through_a_record() {
        let codec = CodecSpec::parse("zstd").unwrap().build().unwrap();
        let block = EncodedBlock::from_raw(codec.as_ref(), sample()).unwrap();
        assert_eq!(block.header.method, Method::Zstd);
        assert!(block.header.comp_len < block.header.raw_len);

        let mut bytes = Vec::new();
        let n = block.write_to(&mut bytes).unwrap();
        assert_eq!(n as usize, bytes.len());

        let (header, payload) = read_record(&mut Cursor::new(&bytes), 1 << 20, n).unwrap();
        assert_eq!(header, block.header);
        assert_eq!(decode_payload(&header, payload).unwrap(), sample());
    }

    #[test]
    fn incompressible_block_falls_back_to_store() {
        let mut x = 0x1234_5678_9abc_def0u64;
        let noise: Vec<u8> = (0..20_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 32) as u8
            })
            .collect();
        for spec in ["lz4", "zstd", "lzma"] {
            let codec = CodecSpec::parse(spec).unwrap().build().unwrap();
            let block = EncodedBlock::from_raw(codec.as_ref(), noise.clone()).unwrap();
            assert_eq!(block.header.method, Method::Store, "{spec}");
            assert_eq!(block.header.comp_len, block.header.raw_len);
        }
    }

    #[test]
    fn empty_block_is_stored() {
        let codec = CodecSpec::parse("zstd").unwrap().build().unwrap();
        let block = EncodedBlock::from_raw(codec.as_ref(), Vec::new()).unwrap();
        assert_eq!(
            (block.header.method, block.header.raw_len),
            (Method::Store, 0)
        );
    }

    #[test]
    fn header_validation_rejects_hostile_values() {
        let ok = RecordHeader {
            method: Method::Zstd,
            raw_len: 100,
            comp_len: 50,
            crc: 0,
        };
        assert_eq!(RecordHeader::decode(&ok.encode(), 100).unwrap(), ok);
        assert!(
            RecordHeader::decode(&ok.encode(), 99).is_err(),
            "raw_len above the cap"
        );
        let mut h = ok;
        h.comp_len = 101;
        assert!(
            RecordHeader::decode(&h.encode(), 1000).is_err(),
            "payload larger than raw"
        );
        let stored = RecordHeader {
            method: Method::Store,
            raw_len: 100,
            comp_len: 99,
            crc: 0,
        };
        assert!(RecordHeader::decode(&stored.encode(), 1000).is_err());
        let mut raw = ok.encode();
        raw[0] = 77;
        assert!(matches!(
            RecordHeader::decode(&raw, 1000),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn reading_never_allocates_beyond_what_the_archive_holds() {
        let h = RecordHeader {
            method: Method::Zstd,
            raw_len: 100 << 20,
            comp_len: 100 << 20,
            crc: 0,
        };
        let mut bytes = h.encode().to_vec();
        bytes.extend_from_slice(&[0; 10]);
        let avail = bytes.len() as u64;
        let err = read_record(&mut Cursor::new(&bytes), 128 << 20, avail).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt(ref m) if m.contains("past the end")),
            "{err}"
        );
        // And a short read with a lying `bytes_available` is a clean truncation error.
        let err = read_record(&mut Cursor::new(&bytes), 128 << 20, u64::MAX).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt(ref m) if m.contains("truncated")),
            "{err}"
        );
    }

    #[test]
    fn crc_mismatch_is_caught_before_decoding() {
        let codec = CodecSpec::parse("lz4").unwrap().build().unwrap();
        let mut block = EncodedBlock::from_raw(codec.as_ref(), sample()).unwrap();
        block.payload[5] ^= 0x40;
        let err = decode_payload(&block.header, block.payload).unwrap_err();
        assert!(matches!(err, Error::Checksum(_)));
    }
}
