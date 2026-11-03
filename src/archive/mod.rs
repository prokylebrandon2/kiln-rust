//! High-level archive operations: [`pack`], [`Archive::open`], [`extract`] and [`verify`].

mod extract;
mod reader;
mod writer;

pub use extract::{ExtractOptions, ExtractStats, extract, verify};
pub use reader::Archive;
pub use writer::{PackOptions, PackStats, pack};
