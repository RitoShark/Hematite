//! On-demand chunk payload read + decompress.

use crate::error::LiveError;
use crate::toc::{Compression, TocChunk};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// Per-chunk sanity limit (matches hematite-file's WadFile guard).
pub const MAX_CHUNK_SIZE: u64 = 1024 * 1024 * 1024;

/// Read and decompress a single chunk from an open WAD file handle.
pub fn read_chunk(file: &mut File, chunk: &TocChunk) -> Result<Vec<u8>, LiveError> {
    if chunk.uncompressed_size > MAX_CHUNK_SIZE || chunk.compressed_size > MAX_CHUNK_SIZE {
        return Err(LiveError::Decompress(format!(
            "chunk {:016x} exceeds size limit",
            chunk.path_hash
        )));
    }
    file.seek(SeekFrom::Start(chunk.offset))?;
    let mut raw = vec![0u8; chunk.compressed_size as usize];
    file.read_exact(&mut raw)?;
    decompress(&raw, chunk)
}

pub(crate) fn decompress(raw: &[u8], chunk: &TocChunk) -> Result<Vec<u8>, LiveError> {
    match chunk.compression {
        Compression::None => Ok(raw.to_vec()),
        Compression::GZip => {
            let mut out = Vec::with_capacity(chunk.uncompressed_size as usize);
            flate2::read::GzDecoder::new(raw)
                .read_to_end(&mut out)
                .map_err(|e| LiveError::Decompress(e.to_string()))?;
            Ok(out)
        }
        Compression::Zstd | Compression::ZstdMulti => zstd::stream::decode_all(raw)
            .map_err(|e| LiveError::Decompress(e.to_string())),
        Compression::Satellite => Err(LiveError::UnsupportedCompression(2)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn chunk(comp: Compression, csz: usize, usz: usize) -> TocChunk {
        TocChunk {
            path_hash: 1,
            offset: 0,
            compressed_size: csz as u64,
            uncompressed_size: usz as u64,
            compression: comp,
        }
    }

    #[test]
    fn decompress_none_passthrough() {
        let data = b"hello".to_vec();
        let out = decompress(&data, &chunk(Compression::None, 5, 5)).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn decompress_gzip_roundtrip() {
        let plain = b"league of legends".repeat(10);
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&plain).unwrap();
        let comp = enc.finish().unwrap();
        let out = decompress(&comp, &chunk(Compression::GZip, comp.len(), plain.len())).unwrap();
        assert_eq!(out, plain);
    }

    #[test]
    fn decompress_zstd_roundtrip() {
        let plain = b"wad chunk payload".repeat(100);
        let comp = zstd::stream::encode_all(&plain[..], 3).unwrap();
        let out = decompress(&comp, &chunk(Compression::Zstd, comp.len(), plain.len())).unwrap();
        assert_eq!(out, plain);
    }

    #[test]
    fn satellite_is_unsupported() {
        assert!(decompress(b"x", &chunk(Compression::Satellite, 1, 1)).is_err());
    }
}
