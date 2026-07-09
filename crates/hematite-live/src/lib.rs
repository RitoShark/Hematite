//! Live game file access for League of Legends.
//!
//! Standalone by design: no rs_* or hematite-* dependencies. Reads game WADs
//! TOC-only (never loads whole archives into memory) so it is safe to index
//! very large base-game WADs.

pub mod chunk;
pub mod detect;
pub mod error;
pub mod index;
pub mod toc;
pub mod wads;

pub use detect::{detect_league, LeagueInstall};
pub use error::LiveError;
pub use index::{wad_path_hash, GameIndex};
pub use toc::{read_toc, read_toc_from, Compression, TocChunk, WadToc};
pub use wads::{champion_wad, enumerate_wads, GameWadInfo};
