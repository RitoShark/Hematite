//! TOC-only WAD reader. Reads header + chunk table, never chunk payloads.
//! Layout ported from Flint's `wad_jade/{format,reader}.rs` (RW v3.1 / v3.4).

use crate::error::LiveError;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};

pub const WAD_MAGIC: [u8; 2] = *b"RW";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    GZip,
    Satellite,
    Zstd,
    ZstdMulti,
}

impl Compression {
    fn from_type_byte(b: u8) -> Result<Self, LiveError> {
        match b & 0x0F {
            0 => Ok(Self::None),
            1 => Ok(Self::GZip),
            2 => Ok(Self::Satellite),
            3 => Ok(Self::Zstd),
            4 => Ok(Self::ZstdMulti),
            other => Err(LiveError::UnsupportedCompression(other)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TocChunk {
    pub path_hash: u64,
    pub offset: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub compression: Compression,
}

#[derive(Debug)]
pub struct WadToc {
    pub path: PathBuf,
    pub version: (u8, u8),
    pub chunks: Vec<TocChunk>,
}

fn read_u16(r: &mut impl Read) -> Result<u16, LiveError> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}
fn read_u32(r: &mut impl Read) -> Result<u32, LiveError> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn read_u64(r: &mut impl Read) -> Result<u64, LiveError> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

/// Read the TOC from an open reader. `path` is stored for diagnostics only.
pub fn read_toc_from(r: &mut (impl Read + Seek), path: PathBuf) -> Result<WadToc, LiveError> {
    let mut magic = [0u8; 2];
    r.read_exact(&mut magic)?;
    if magic != WAD_MAGIC {
        return Err(LiveError::BadMagic);
    }
    let mut ver = [0u8; 2];
    r.read_exact(&mut ver)?;
    let (major, minor) = (ver[0], ver[1]);
    if major != 3 {
        return Err(LiveError::UnsupportedVersion { major, minor });
    }
    // 256-byte ECDSA signature + 8-byte XXH64 checksum
    let mut skip = [0u8; 264];
    r.read_exact(&mut skip)?;
    let count = read_u32(r)? as usize;
    // Sanity cap: a chunk record is 32 bytes; refuse absurd counts.
    if count > 4_000_000 {
        return Err(LiveError::Decompress(format!(
            "chunk count {count} too large"
        )));
    }
    let mut chunks = Vec::with_capacity(count);
    for _ in 0..count {
        let path_hash = read_u64(r)?;
        let offset = read_u32(r)? as u64;
        let compressed_size = read_u32(r)? as u64;
        let uncompressed_size = read_u32(r)? as u64;
        let mut tail = [0u8; 2]; // type byte + duplicate flag
        r.read_exact(&mut tail)?;
        let compression = Compression::from_type_byte(tail[0])?;
        let _pad_or_subchunk_start = read_u16(r)?;
        let _chunk_checksum = read_u64(r)?;
        chunks.push(TocChunk {
            path_hash,
            offset,
            compressed_size,
            uncompressed_size,
            compression,
        });
    }
    Ok(WadToc {
        path,
        version: (major, minor),
        chunks,
    })
}

/// Read the TOC of a WAD file on disk.
pub fn read_toc(path: &Path) -> Result<WadToc, LiveError> {
    let file = File::open(path)?;
    let mut r = BufReader::new(file);
    read_toc_from(&mut r, path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn synth_wad(major: u8, minor: u8, chunks: &[(u64, u32, u32, u32, u8)]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"RW");
        v.push(major);
        v.push(minor);
        v.extend_from_slice(&[0u8; 256]); // signature
        v.extend_from_slice(&[0u8; 8]); // checksum
        v.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
        for &(hash, off, csz, usz, ty) in chunks {
            v.extend_from_slice(&hash.to_le_bytes());
            v.extend_from_slice(&off.to_le_bytes());
            v.extend_from_slice(&csz.to_le_bytes());
            v.extend_from_slice(&usz.to_le_bytes());
            v.push(ty);
            v.push(0); // duplicate
            v.extend_from_slice(&0u16.to_le_bytes()); // pad / subchunk_start
            v.extend_from_slice(&0u64.to_le_bytes()); // per-chunk checksum
        }
        v
    }

    #[test]
    fn parses_v3_4_toc() {
        let bytes = synth_wad(3, 4, &[(0xDEAD_BEEF, 400, 10, 20, 3)]);
        let toc = read_toc_from(&mut Cursor::new(bytes), "x.wad.client".into()).unwrap();
        assert_eq!(toc.version, (3, 4));
        assert_eq!(toc.chunks.len(), 1);
        let c = &toc.chunks[0];
        assert_eq!(c.path_hash, 0xDEAD_BEEF);
        assert_eq!(c.offset, 400);
        assert_eq!(c.compressed_size, 10);
        assert_eq!(c.uncompressed_size, 20);
        assert_eq!(c.compression, Compression::Zstd);
    }

    #[test]
    fn parses_v3_1_toc() {
        let bytes = synth_wad(3, 1, &[(1, 300, 5, 5, 0), (2, 305, 7, 9, 1)]);
        let toc = read_toc_from(&mut Cursor::new(bytes), "x.wad".into()).unwrap();
        assert_eq!(toc.chunks.len(), 2);
        assert_eq!(toc.chunks[0].compression, Compression::None);
        assert_eq!(toc.chunks[1].compression, Compression::GZip);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = synth_wad(3, 4, &[]);
        bytes[0] = b'X';
        assert!(matches!(
            read_toc_from(&mut Cursor::new(bytes), "x".into()),
            Err(LiveError::BadMagic)
        ));
    }

    #[test]
    fn rejects_unsupported_major() {
        let bytes = synth_wad(2, 0, &[]);
        assert!(matches!(
            read_toc_from(&mut Cursor::new(bytes), "x".into()),
            Err(LiveError::UnsupportedVersion { major: 2, minor: 0 })
        ));
    }

    #[test]
    fn zstd_multi_compression_kind() {
        // type byte low nibble 4 = ZstdMulti (subchunked)
        let bytes = synth_wad(3, 4, &[(9, 0, 1, 1, 4)]);
        let toc = read_toc_from(&mut Cursor::new(bytes), "x".into()).unwrap();
        assert_eq!(toc.chunks[0].compression, Compression::ZstdMulti);
    }
}
