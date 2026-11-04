//! Kiln (Rust rewrite) - a multicore archiver built on proven compression libraries.
//!
//! # Architecture in one paragraph
//!
//! File contents are concatenated into a single stream cut into fixed-size blocks. Every block
//! is compressed independently by a pluggable [`codec::BlockCodec`] (Zstandard, LZ4, LZMA2 or
//! stored) on a pool of worker threads; an ordered, bounded pipeline ([`parallel`]) keeps memory
//! flat and output deterministic. An index at the end of the archive lists every entry and block,
//! so listing is instant and single files can be extracted by decoding only the blocks they
//! touch. CRC-32 guards each block and BLAKE3 guards each file. See `docs/ARCHITECTURE.md`.
//!
//! # Example
//!
//! ```no_run
//! use std::path::{Path, PathBuf};
//! use kiln::{Archive, ExtractOptions, PackOptions, extract, pack};
//!
//! let stats = pack(Path::new("out.kiln"), &[PathBuf::from("my_dir")], &PackOptions::default())?;
//! println!("{} files -> {} bytes", stats.files, stats.archive_bytes);
//!
//! let archive = Archive::open(Path::new("out.kiln"))?;
//! extract(&archive, &ExtractOptions::new("restored"))?;
//! # Ok::<(), kiln::Error>(())
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod archive;
pub mod bench;
pub mod cancel;
pub mod classify;
pub mod codec;
pub mod error;
pub mod format;
pub mod fsutil;
pub mod integrity;
pub mod parallel;
pub mod path;
pub mod profile;
pub mod util;
pub mod walk;

pub use archive::{
    Archive, BlockStats, ClassTotals, ExtractOptions, ExtractStats, PackOptions, PackStats,
    extract, extract_with_progress, pack, pack_with_progress, verify, verify_with,
    verify_with_progress,
};
pub use error::{Error, Result};
pub use profile::Profile;
