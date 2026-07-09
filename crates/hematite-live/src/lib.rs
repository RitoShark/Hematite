//! Live game file access for League of Legends.
//!
//! Standalone by design: no rs_* or hematite-* dependencies. Reads game WADs
//! TOC-only (never loads whole archives into memory) so it is safe to index
//! very large base-game WADs.

pub mod chunk;
pub mod error;
pub mod toc;

pub use error::LiveError;
pub use toc::{read_toc, read_toc_from, Compression, TocChunk, WadToc};
