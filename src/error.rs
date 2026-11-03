//! Error type shared by the whole crate.

use std::path::{Path, PathBuf};

use crate::parallel::PipelineError;

/// Convenience alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong while creating, reading or verifying an archive.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An I/O operation failed on a specific path.
    #[error("{}: {source}", .path.display())]
    Io {
        /// The path being operated on.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// An I/O operation failed with no particular path attached.
    #[error("I/O error: {0}")]
    IoBare(#[from] std::io::Error),
    /// The file is not a Kiln archive at all (wrong magic bytes).
    #[error("not a Kiln archive: {0}")]
    NotAnArchive(&'static str),
    /// The archive is valid but uses a feature or version this build does not understand.
    #[error("unsupported archive: {0}")]
    Unsupported(String),
    /// The archive is structurally damaged or internally inconsistent.
    #[error("archive is corrupt: {0}")]
    Corrupt(String),
    /// A checksum did not match: the data was damaged or tampered with.
    #[error("checksum mismatch: {0}")]
    Checksum(String),
    /// A compression backend reported a failure.
    #[error("{codec} codec error: {message}")]
    Codec {
        /// Backend name, e.g. `zstd`.
        codec: &'static str,
        /// Backend-provided detail.
        message: String,
    },
    /// An archive member name would escape the extraction directory or is otherwise unsafe.
    #[error("unsafe path: {0:?}")]
    UnsafePath(String),
    /// A request that cannot be satisfied (bad option, duplicate names, missing member...).
    #[error("{0}")]
    Invalid(String),
    /// A source file changed size while it was being archived.
    #[error("file changed while being archived: {}", .0.display())]
    FileChanged(PathBuf),
    /// The destination already exists and overwriting was not requested.
    #[error("{} already exists (use --force to overwrite)", .0.display())]
    Exists(PathBuf),
    /// A bug in Kiln itself (for example a worker thread panicked).
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Shorthand for [`Error::Corrupt`].
    pub fn corrupt(msg: impl Into<String>) -> Self {
        Error::Corrupt(msg.into())
    }
}

impl From<PipelineError<Error>> for Error {
    fn from(e: PipelineError<Error>) -> Self {
        match e {
            PipelineError::Task(e) => e,
            PipelineError::Panic(msg) => Error::Internal(msg),
        }
    }
}

/// Attach a path to an [`std::io::Result`].
pub trait IoContext<T> {
    /// Convert the I/O error into [`Error::Io`] naming `path`.
    fn at(self, path: impl AsRef<Path>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: impl AsRef<Path>) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.as_ref().to_path_buf(),
            source,
        })
    }
}
