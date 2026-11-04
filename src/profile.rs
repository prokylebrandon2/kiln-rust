//! Compression profiles: named, measured presets that replace raw level numbers.
//!
//! The mapping below was chosen from the measurements in `docs/PHASE3.md` (sweep on a 125 MiB
//! source tree and a mixed directory, 2 vCPUs). It is data, not dogma: `-c` and `-b` override the
//! codec and chunk size of any profile.

use crate::codec::CodecSpec;
use crate::error::{Error, Result};
use crate::format::DEFAULT_BLOCK_LOG2;

/// A named preset. The id is stored in the archive header (informational only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Profile {
    /// An explicit codec was chosen with `-c`; no preset applies.
    Custom = 0,
    /// Lowest CPU cost per byte that still compresses well (`zstd:1`).
    Fastest = 1,
    /// The default trade-off (`zstd:3`).
    Balanced = 2,
    /// Smallest output this build can make (`zstd:19`, 16 MiB chunks, content classification).
    Maximum = 3,
    /// No compression at all; chunking, checksums and the index only.
    Store = 4,
    /// `zstd:3`, but each file is classified first and already-compressed or high-entropy files
    /// are stored in their own chunks instead of being run through the compressor.
    Adaptive = 5,
}

/// Everything a profile decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProfileSettings {
    /// Codec and level for compressible data.
    pub codec: CodecSpec,
    /// log2 of the maximum chunk size.
    pub block_log2: u8,
    /// Read a sample of every file first and store incompressible files separately.
    pub classify: bool,
}

impl Profile {
    /// Every profile a user can name on the command line.
    pub const NAMED: [Profile; 5] = [
        Profile::Fastest,
        Profile::Balanced,
        Profile::Maximum,
        Profile::Store,
        Profile::Adaptive,
    ];

    /// The byte written to disk.
    pub fn id(self) -> u8 {
        self as u8
    }

    /// Parse the byte read from disk.
    pub fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Custom),
            1 => Some(Self::Fastest),
            2 => Some(Self::Balanced),
            3 => Some(Self::Maximum),
            4 => Some(Self::Store),
            5 => Some(Self::Adaptive),
            _ => None,
        }
    }

    /// Lower-case name used on the command line and in listings.
    pub fn name(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::Fastest => "fastest",
            Self::Balanced => "balanced",
            Self::Maximum => "maximum",
            Self::Store => "store",
            Self::Adaptive => "adaptive",
        }
    }

    /// Parse a command-line name.
    pub fn parse(text: &str) -> Result<Self> {
        let wanted = text.trim().to_ascii_lowercase();
        Self::NAMED
            .into_iter()
            .find(|p| p.name() == wanted || (wanted == "max" && *p == Self::Maximum))
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "unknown profile {text:?} (choose fastest, balanced, maximum, store or adaptive)"
                ))
            })
    }

    /// The settings this profile stands for. `Custom` behaves like `Balanced` for anything the
    /// caller does not override.
    pub fn settings(self) -> ProfileSettings {
        let zstd = |level| CodecSpec::new(crate::codec::Method::Zstd, level);
        match self {
            Self::Fastest => ProfileSettings {
                codec: zstd(1),
                block_log2: DEFAULT_BLOCK_LOG2,
                classify: false,
            },
            Self::Balanced | Self::Custom => ProfileSettings {
                codec: zstd(3),
                block_log2: DEFAULT_BLOCK_LOG2,
                classify: false,
            },
            Self::Maximum => ProfileSettings {
                codec: zstd(19),
                block_log2: 24,
                classify: true,
            },
            Self::Store => ProfileSettings {
                codec: CodecSpec::new(crate::codec::Method::Store, 0),
                block_log2: DEFAULT_BLOCK_LOG2,
                classify: false,
            },
            Self::Adaptive => ProfileSettings {
                codec: zstd(3),
                block_log2: DEFAULT_BLOCK_LOG2,
                classify: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_names_and_parsing_round_trip() {
        for p in Profile::NAMED.into_iter().chain([Profile::Custom]) {
            assert_eq!(Profile::from_id(p.id()), Some(p));
        }
        for p in Profile::NAMED {
            assert_eq!(Profile::parse(p.name()).unwrap(), p);
            assert_eq!(Profile::parse(&p.name().to_uppercase()).unwrap(), p);
        }
        assert_eq!(Profile::parse("max").unwrap(), Profile::Maximum);
        assert!(Profile::parse("ludicrous").is_err());
        assert!(
            Profile::parse("custom").is_err(),
            "custom is implicit, not named"
        );
        assert_eq!(Profile::from_id(99), None);
    }

    #[test]
    fn every_profile_builds_a_valid_codec() {
        for p in Profile::NAMED.into_iter().chain([Profile::Custom]) {
            let s = p.settings();
            s.codec.build().unwrap();
            assert!((12..=27).contains(&s.block_log2));
        }
    }
}
