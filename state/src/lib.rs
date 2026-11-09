//! Kiln's UI-independent state.
//!
//! Nothing here knows about Slint, Android or the engine. A folder is a [`Listing`]: typed
//! [`Entry`] values, a sorted and filtered *view* of them, and a selection that survives
//! re-sorting. The UI layer maps rows of the view to what is drawn, one visible row at a time.
//!
//! Listing a folder is two-phase (see [`scan`]): names and kinds first, which is fast and lets
//! the first rows appear at once, then sizes, dates and item counts, which are slow on a phone's
//! shared storage and are fetched for the rows in view first.

#![forbid(unsafe_code)]

pub mod entry;
pub mod fmt;
pub mod listing;
pub mod order;
pub mod scan;

pub use entry::{Entry, Kind, Meta};
pub use listing::{Change, Listing};
pub use order::{Order, SortKey};
