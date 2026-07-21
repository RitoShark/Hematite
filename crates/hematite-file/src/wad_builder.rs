//! WAD file building — wraps rs_wad's WadBuilder for use by the CLI.
//!
//! rs_wad's builder writes a v3.4 archive: it zstd-compresses each chunk,
//! deduplicates identical contents by an xxh3-128 fingerprint, and lays out a
//! table of contents sorted by path hash. The output is a valid archive that
//! round-trips losslessly, but is not byte-identical to other tools.

use anyhow::Result;
use rs_wad::WadBuilder;
use std::io::{Seek, Write};

/// Build a WAD file from an extracted file list, skipping removed paths.
///
/// `files` — all chunks as `(original_hash, resolved_path, bytes)`.
/// `files_to_remove` — paths to exclude from the output WAD.
/// `writer` — destination (file, cursor, etc.).
///
/// Returns the number of chunks written.
pub fn build_wad<W: Write + Seek>(
    files: &[(u64, String, Vec<u8>)],
    files_to_remove: &[String],
    writer: &mut W,
) -> Result<usize> {
    let mut builder = WadBuilder::new();
    let mut count = 0;

    for (hash, path, _) in files {
        if !files_to_remove.contains(path) {
            builder = builder.with_chunk_hash(*hash);
            count += 1;
        } else {
            tracing::debug!("Excluding removed file: {}", path);
        }
    }

    builder
        .build_to_writer(writer, |path_hash, out| {
            let (_, path, bytes) =
                files
                    .iter()
                    .find(|(h, _, _)| *h == path_hash)
                    .ok_or_else(|| {
                        rs_wad::Error::Build(format!("Missing file for hash {:016X}", path_hash))
                    })?;

            tracing::trace!("Writing chunk: {} ({} bytes)", path, bytes.len());
            out.write_all(bytes).map_err(rs_io::Error::from)?;
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("Failed to build WAD: {:?}", e))?;

    Ok(count)
}
