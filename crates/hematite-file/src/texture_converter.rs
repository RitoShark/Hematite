//! DDS ↔ TEX texture format conversion using rs_tex.
//!
//! Converts between Microsoft's DDS format and League's proprietary TEX format.

use anyhow::{Context, Result};

/// Convert DDS texture data to TEX format.
///
/// Reads a DDS file, decodes it, and re-encodes as TEX with the same
/// compression format and mipmaps.
///
/// ## Supported formats
/// - BC1 (DXT1)
/// - BC3 (DXT5)
/// - BGRA8 (uncompressed)
///
/// ## Process
/// 1. Parse DDS file → extract width/height/format/mipmaps
/// 2. Decode compressed blocks to RGBA
/// 3. Re-encode as TEX with same compression
///
/// **Note**: This is a lossy process for BC formats due to re-compression.
/// For lossless conversion, we'd need to copy raw compressed blocks, but
/// DDS and TEX have different mipmap ordering (DDS: large→small, TEX: small→large).
pub fn dds_to_tex(dds_bytes: &[u8]) -> Result<Vec<u8>> {
    // 1. Try lossless block-swapping first
    if let Some(tex) = dds_to_tex_lossless(dds_bytes) {
        tracing::debug!("Lossless DDS→TEX conversion succeeded");
        return Ok(tex);
    }

    tracing::warn!("Lossless DDS→TEX conversion failed, falling back to rs_tex decoder/encoder");

    use rs_io::Serialize;
    use rs_tex::{read_dds_bytes, TexFormat, Texture};

    // Decode the DDS to an RGBA image. (rs_tex's reader loses the source
    // compression format, so we sniff the DDS header ourselves below.)
    let rgba_image = read_dds_bytes(dds_bytes).context("Failed to decode DDS file")?;

    let width = rgba_image.width();
    let height = rgba_image.height();
    let mip_count = dds_header_mip_count(dds_bytes).unwrap_or(1);
    let has_mipmaps = mip_count > 1;

    tracing::debug!("Converting DDS: {}x{}, {} mipmaps", width, height, mip_count);

    // Determine the output TEX format from the DDS header. rs_tex can only
    // *encode* BC1/BC3/BC5/BC7, so an uncompressed BGRA8 source is built
    // directly instead of going through `Texture::encode`.
    let tex = match detect_dds_format(dds_bytes) {
        DdsFormat::Bc1 => Texture::encode(&rgba_image, TexFormat::Bc1, has_mipmaps)
            .context("Failed to encode TEX (BC1)")?,
        DdsFormat::Bc3 => Texture::encode(&rgba_image, TexFormat::Bc3, has_mipmaps)
            .context("Failed to encode TEX (BC3)")?,
        DdsFormat::Bgra8 => Texture::from_rgba_bgra8(&rgba_image),
        DdsFormat::Unsupported => {
            // Unknown compression — default to BC3, the safe lossy fallback.
            Texture::encode(&rgba_image, TexFormat::Bc3, has_mipmaps)
                .context("Failed to encode TEX (BC3 default)")?
        }
    };

    tracing::debug!("Using TEX format: {:?}", tex.format);

    // Serialize TEX to bytes.
    let output = tex.to_bytes().context("Failed to write TEX data")?;

    tracing::info!(
        "Converted DDS→TEX (fallback): {}x{} ({:?}), {} mipmaps, {} bytes → {} bytes",
        width,
        height,
        tex.format,
        mip_count,
        dds_bytes.len(),
        output.len()
    );

    Ok(output)
}

/// Convert TEX texture data to DDS format.
pub fn tex_to_dds(tex_bytes: &[u8]) -> Result<Vec<u8>> {
    if let Some(dds) = tex_to_dds_lossless(tex_bytes) {
        Ok(dds)
    } else {
        anyhow::bail!("Failed to convert TEX to DDS: format or size mismatch")
    }
}

/// Lossless block-swapping and mipmap-reversing DDS to TEX converter.
pub fn dds_to_tex_lossless(dds_data: &[u8]) -> Option<Vec<u8>> {
    if dds_data.len() < 128 || &dds_data[..4] != b"DDS " {
        return None;
    }

    let height = u32::from_le_bytes(dds_data[12..16].try_into().ok()?) as u16;
    let width = u32::from_le_bytes(dds_data[16..20].try_into().ok()?) as u16;
    let mip_count = u32::from_le_bytes(dds_data[28..32].try_into().ok()?);

    let pf_flags = u32::from_le_bytes(dds_data[80..84].try_into().ok()?);
    let four_cc = &dds_data[84..88];
    let rgb_bit_count = u32::from_le_bytes(dds_data[88..92].try_into().ok()?);

    let (tex_format, bytes_per_block, block_dim): (u8, usize, usize) = if pf_flags & 0x4 != 0 {
        if four_cc == b"DX10" {
            return None;
        }
        match four_cc {
            b"DXT1" => (10, 8, 4),  // Bc1
            b"DXT5" => (12, 16, 4), // Bc3
            _ => return None,
        }
    } else if pf_flags & 0x40 != 0 && rgb_bit_count == 32 {
        (20, 4, 1) // Bgra8
    } else {
        return None;
    };

    let pixel_data = &dds_data[128..];
    let actual_mips = mip_count.max(1);
    let has_mipmaps = actual_mips > 1;

    // Calculate mip offsets (DDS order: largest → smallest)
    let mut mip_slices: Vec<(usize, usize)> = Vec::new();
    let mut offset = 0usize;
    for i in 0..actual_mips {
        let mw = ((width as u32) >> i).max(1) as usize;
        let mh = ((height as u32) >> i).max(1) as usize;
        let blocks_x = mw.div_ceil(block_dim);
        let blocks_y = mh.div_ceil(block_dim);
        let mip_size = blocks_x * blocks_y * bytes_per_block;
        mip_slices.push((offset, mip_size));
        offset += mip_size;
    }

    let mut tex = Vec::new();
    tex.extend_from_slice(&[b'T', b'E', b'X', 0x00]); // magic
    tex.extend_from_slice(&width.to_le_bytes());
    tex.extend_from_slice(&height.to_le_bytes());
    tex.push(0); // is_extended_format
    tex.push(tex_format);
    tex.push(0); // resource_type
    tex.push(if has_mipmaps { 1 } else { 0 }); // flags

    // TEX stores mips smallest→largest (reversed from DDS)
    for &(mip_offset, mip_size) in mip_slices.iter().rev() {
        let end = mip_offset + mip_size;
        if end <= pixel_data.len() {
            tex.extend_from_slice(&pixel_data[mip_offset..end]);
        } else {
            return None;
        }
    }

    Some(tex)
}

/// Lossless block-swapping and mipmap-reversing TEX to DDS converter.
pub fn tex_to_dds_lossless(tex_data: &[u8]) -> Option<Vec<u8>> {
    if tex_data.len() < 12 || &tex_data[..4] != b"TEX\0" {
        return None;
    }

    let width = u16::from_le_bytes(tex_data[4..6].try_into().ok()?) as u32;
    let height = u16::from_le_bytes(tex_data[6..8].try_into().ok()?) as u32;
    let tex_format = tex_data[9];
    let has_mipmaps = tex_data[11] & 1 != 0;

    let (four_cc, bytes_per_block, block_dim, is_compressed): (&[u8; 4], usize, usize, bool) =
        match tex_format {
            10 => (b"DXT1", 8, 4, true),  // Bc1
            12 => (b"DXT5", 16, 4, true), // Bc3
            20 => (b"\0\0\0\0", 4, 1, false), // Bgra8
            _ => return None,
        };

    // Calculate mip count
    let mip_count = if has_mipmaps {
        let max_dim = width.max(height);
        (max_dim as f64).log2().floor() as u32 + 1
    } else {
        1
    };

    // Calculate mip sizes (from largest to smallest, DDS order)
    let mut mip_sizes: Vec<usize> = Vec::new();
    for i in 0..mip_count {
        let mw = (width >> i).max(1) as usize;
        let mh = (height >> i).max(1) as usize;
        let blocks_x = mw.div_ceil(block_dim);
        let blocks_y = mh.div_ceil(block_dim);
        mip_sizes.push(blocks_x * blocks_y * bytes_per_block);
    }

    let pixel_data = &tex_data[12..];
    let total_pixel_size: usize = mip_sizes.iter().sum();
    if pixel_data.len() < total_pixel_size {
        return None;
    }

    // TEX stores smallest→largest, compute offsets in TEX order (reversed)
    let mut tex_offsets: Vec<(usize, usize)> = Vec::new();
    let mut off = 0usize;
    for &size in mip_sizes.iter().rev() {
        tex_offsets.push((off, size));
        off += size;
    }

    // Build DDS header
    let pitch_or_linear = mip_sizes[0]; // largest mip size
    let mut dds = Vec::with_capacity(128 + total_pixel_size);

    dds.extend_from_slice(b"DDS "); // magic
    dds.extend_from_slice(&124u32.to_le_bytes()); // dwSize
    let flags: u32 = 0x1 | 0x2 | 0x4 | 0x1000 | if has_mipmaps { 0x20000 } else { 0 }
        | if is_compressed { 0x80000 } else { 0x8 };
    dds.extend_from_slice(&flags.to_le_bytes()); // dwFlags
    dds.extend_from_slice(&height.to_le_bytes()); // dwHeight
    dds.extend_from_slice(&width.to_le_bytes()); // dwWidth
    dds.extend_from_slice(&(pitch_or_linear as u32).to_le_bytes()); // dwPitchOrLinearSize
    dds.extend_from_slice(&0u32.to_le_bytes()); // dwDepth
    dds.extend_from_slice(&mip_count.to_le_bytes()); // dwMipMapCount
    dds.extend_from_slice(&[0u8; 44]); // dwReserved1[11]

    // DDS_PIXELFORMAT
    dds.extend_from_slice(&32u32.to_le_bytes()); // dwSize
    if is_compressed {
        dds.extend_from_slice(&0x4u32.to_le_bytes()); // dwFlags: FOURCC
        dds.extend_from_slice(four_cc); // dwFourCC
        dds.extend_from_slice(&0u32.to_le_bytes()); // dwRGBBitCount
        dds.extend_from_slice(&0u32.to_le_bytes()); // dwRBitMask
        dds.extend_from_slice(&0u32.to_le_bytes()); // dwGBitMask
        dds.extend_from_slice(&0u32.to_le_bytes()); // dwBBitMask
        dds.extend_from_slice(&0u32.to_le_bytes()); // dwABitMask
    } else {
        dds.extend_from_slice(&0x41u32.to_le_bytes()); // dwFlags: ALPHAPIXELS|RGB
        dds.extend_from_slice(&0u32.to_le_bytes()); // dwFourCC
        dds.extend_from_slice(&32u32.to_le_bytes()); // dwRGBBitCount
        dds.extend_from_slice(&0x00FF_0000u32.to_le_bytes()); // dwRBitMask
        dds.extend_from_slice(&0x0000_FF00u32.to_le_bytes()); // dwGBitMask
        dds.extend_from_slice(&0x0000_00FFu32.to_le_bytes()); // dwBBitMask
        dds.extend_from_slice(&0xFF00_0000u32.to_le_bytes()); // dwABitMask
    }

    let caps: u32 = 0x1000 | if has_mipmaps { 0x8 | 0x400000 } else { 0 };
    dds.extend_from_slice(&caps.to_le_bytes()); // dwCaps
    dds.extend_from_slice(&[0u8; 16]); // dwCaps2..dwReserved2

    // Write pixel data: DDS order is largest→smallest (reverse of TEX)
    for &(tex_off, size) in tex_offsets.iter().rev() {
        let end = tex_off + size;
        if end <= pixel_data.len() {
            dds.extend_from_slice(&pixel_data[tex_off..end]);
        } else {
            return None;
        }
    }

    Some(dds)
}

/// Detected DDS compression format.
#[derive(Debug, Clone, Copy)]
enum DdsFormat {
    Bc1,
    Bc3,
    Bgra8,
    Unsupported,
}

/// Read the mip count from a DDS header, defaulting to `1` when absent.
fn dds_header_mip_count(dds: &[u8]) -> Option<u32> {
    if dds.len() < 128 || &dds[..4] != b"DDS " {
        return None;
    }
    Some(u32::from_le_bytes(dds[28..32].try_into().ok()?).max(1))
}

/// Detect the DDS compression format directly from the DDS pixel-format header.
///
/// Parses the FourCC / RGB-bit-count fields (the same span the lossless path
/// reads) to choose the output TEX format. Unknown encodings fall back to
/// [`DdsFormat::Unsupported`] (the caller then defaults to BC3).
fn detect_dds_format(dds: &[u8]) -> DdsFormat {
    if dds.len() < 128 || &dds[..4] != b"DDS " {
        return DdsFormat::Unsupported;
    }

    let pf_flags = match dds[80..84].try_into() {
        Ok(b) => u32::from_le_bytes(b),
        Err(_) => return DdsFormat::Unsupported,
    };
    let four_cc = &dds[84..88];
    let rgb_bit_count = match dds[88..92].try_into() {
        Ok(b) => u32::from_le_bytes(b),
        Err(_) => return DdsFormat::Unsupported,
    };

    if pf_flags & 0x4 != 0 {
        // FOURCC compressed
        match four_cc {
            b"DXT1" => DdsFormat::Bc1,
            b"DXT5" => DdsFormat::Bc3,
            _ => DdsFormat::Unsupported,
        }
    } else if pf_flags & 0x40 != 0 && rgb_bit_count == 32 {
        DdsFormat::Bgra8
    } else {
        DdsFormat::Unsupported
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore] // Requires actual DDS file
    fn test_dds_to_tex_conversion() {
        // Requires a real DDS file — tested manually with sample files
    }
}
