//! Error type for hematite-live.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a WAD file (bad magic)")]
    BadMagic,
    #[error("unsupported WAD version {major}.{minor}")]
    UnsupportedVersion { major: u8, minor: u8 },
    #[error("unsupported compression kind {0}")]
    UnsupportedCompression(u8),
    #[error("chunk not found: {0:016x}")]
    NotFound(u64),
    #[error("League installation not found or invalid: {0}")]
    InvalidInstall(PathBuf),
    #[error("decompression failed: {0}")]
    Decompress(String),
}
