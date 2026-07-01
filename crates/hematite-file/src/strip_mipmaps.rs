//! Strip mipmaps from DDS and TEX texture bytes.
//!
//! Why: certain Riot patches regressed mipmap handling for some champion
//! textures — DDS/TEX files with a full mip chain stop rendering in game
//! while the same image with only the base level renders fine. Stripping
//! mips at fix time is a non-invasive workaround until Riot fixes the
//! engine path.
//!
//! Logic:
//! - DDS: edit the 128-byte header (zero out MIPMAPCOUNT + MIPMAP caps, set
//!   mip count to 1) and truncate to header + base level.
//! - TEX: TEX stores mips smallest→largest, so the base level is at the END.
//!   Compute the offset to skip past every smaller mip, slice out the
//!   base, and rewrite the 12-byte header with HasMipMaps cleared.
//!
//! Public entry points (`strip_mipmaps_dds`, `strip_mipmaps_tex`,
//! `strip_mipmaps_auto`) return the input bytes unchanged when there's
//! nothing to strip or the format isn't supported.

use anyhow::Result;

/// Returns `true` when `data` looks like a DDS file (magic + minimum header).
pub fn is_dds(data: &[u8]) -> bool {
    data.len() >= 128 && &data[..4] == b"DDS "
}

/// Returns `true` when `data` looks like a Riot TEX file (magic + minimum header).
pub fn is_tex(data: &[u8]) -> bool {
    data.len() >= 12 && &data[..4] == b"TEX\0"
}

/// Strip mipmaps from a DDS byte buffer. If no mips are present, the format
/// is DX10, or the pixel format isn't recognised, returns the input bytes
/// verbatim.
pub fn strip_mipmaps_dds(dds_bytes: &[u8]) -> Result<Vec<u8>> {
    match strip_dds_mipmaps_inner(dds_bytes) {
        Some(stripped) => {
            tracing::debug!(
                "stripped DDS mipmaps: {} → {} bytes",
                dds_bytes.len(),
                stripped.len()
            );
            Ok(stripped)
        }
        None => Ok(dds_bytes.to_vec()),
    }
}

/// Strip mipmaps from a Riot TEX byte buffer. If the HasMipMaps flag is
/// already clear, the resource type isn't plain 2D, or the format byte
/// isn't recognised, returns the input bytes verbatim.
pub fn strip_mipmaps_tex(tex_bytes: &[u8]) -> Result<Vec<u8>> {
    match strip_tex_mipmaps_inner(tex_bytes) {
        Some(stripped) => {
            tracing::debug!(
                "stripped TEX mipmaps: {} → {} bytes",
                tex_bytes.len(),
                stripped.len()
            );
            Ok(stripped)
        }
        None => Ok(tex_bytes.to_vec()),
    }
}

/// Sniff the magic bytes and dispatch to the DDS or TEX stripper. Returns
/// the input unchanged for any other byte stream.
pub fn strip_mipmaps_auto(bytes: &[u8]) -> Result<Vec<u8>> {
    if is_dds(bytes) {
        return strip_mipmaps_dds(bytes);
    }
    if is_tex(bytes) {
        return strip_mipmaps_tex(bytes);
    }
    Ok(bytes.to_vec())
}

// ── DDS ──────────────────────────────────────────────────────────────

const DDSD_MIPMAPCOUNT: u32 = 0x0002_0000;
const DDSCAPS_COMPLEX: u32 = 0x0000_0008;
const DDSCAPS_MIPMAP: u32 = 0x0040_0000;

fn strip_dds_mipmaps_inner(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 128 {
        return None;
    }

    let mip_count = read_u32_le(data, 0x1C);
    if mip_count <= 1 {
        return None;
    }

    let width = read_u32_le(data, 0x10);
    let height = read_u32_le(data, 0x0C);
    if width == 0 || height == 0 {
        return None;
    }

    // FourCC at 0x54. DX10 means a 20-byte extension header follows the
    // standard 128 — skip rather than rewriting that path.
    let fourcc = &data[0x54..0x58];
    if fourcc == b"DX10" {
        return None;
    }

    let base_size = dds_base_level_size(width, height, fourcc, &data[0x58..0x6C])?;
    let total_required = 128 + base_size;
    if total_required > data.len() {
        return None;
    }

    let mut out = Vec::with_capacity(total_required);
    out.extend_from_slice(&data[..total_required]);

    // Header rewrites (all little-endian u32):
    //   flags        -= DDSD_MIPMAPCOUNT
    //   mipmap_count  = 1
    //   caps         -= DDSCAPS_MIPMAP | DDSCAPS_COMPLEX
    let flags = read_u32_le(&out, 0x08) & !DDSD_MIPMAPCOUNT;
    write_u32_le(&mut out, 0x08, flags);
    write_u32_le(&mut out, 0x1C, 1);
    let caps = read_u32_le(&out, 0x6C) & !(DDSCAPS_MIPMAP | DDSCAPS_COMPLEX);
    write_u32_le(&mut out, 0x6C, caps);

    Some(out)
}

fn dds_base_level_size(width: u32, height: u32, fourcc: &[u8], pf_tail: &[u8]) -> Option<usize> {
    let block_bytes = match fourcc {
        b"DXT1" | b"BC1U" => Some(8usize),
        b"DXT5" | b"BC3U" | b"DXT3" => Some(16usize),
        b"BC4U" | b"ATI1" => Some(8usize),
        b"BC5U" | b"ATI2" => Some(16usize),
        _ => None,
    };
    if let Some(bytes_per_block) = block_bytes {
        let blocks_w = (width as usize).div_ceil(4);
        let blocks_h = (height as usize).div_ceil(4);
        return Some(blocks_w * blocks_h * bytes_per_block);
    }

    // Uncompressed: rgbBitCount lives at offset 0x58 in the full header,
    // which is offset 0 in the pf_tail slice (caller passes [0x58..0x6C]).
    if pf_tail.len() >= 4 {
        let bit_count = read_u32_le(pf_tail, 0);
        if bit_count > 0 && bit_count.is_multiple_of(8) {
            let bytes_per_pixel = (bit_count / 8) as usize;
            return Some(width as usize * height as usize * bytes_per_pixel);
        }
    }

    None
}

// ── TEX ──────────────────────────────────────────────────────────────

const TEX_FLAG_HAS_MIPMAPS: u8 = 1;

fn strip_tex_mipmaps_inner(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 12 || &data[..4] != b"TEX\0" {
        return None;
    }

    let flags = data[11];
    if flags & TEX_FLAG_HAS_MIPMAPS == 0 {
        return None;
    }

    // resource_type at offset 10: 0=texture, 1=cubemap, 2=surface, 3=volume.
    // Cubemaps and volume textures store multiple faces / depth slices per
    // mip level, so the 2D-only mip-byte math underestimates the skip and
    // corrupts the file. Bail on anything other than plain 2D.
    let resource_type = data[10];
    if resource_type != 0 {
        return None;
    }

    let width = u16::from_le_bytes(data[4..6].try_into().ok()?) as u32;
    let height = u16::from_le_bytes(data[6..8].try_into().ok()?) as u32;
    if width == 0 || height == 0 {
        return None;
    }

    let tex_format = data[9];
    let (bytes_per_block, block_dim) = tex_format_block_info(tex_format)?;

    let max_dim = width.max(height);
    if max_dim == 0 {
        return None;
    }
    let mip_count = (max_dim as f64).log2().floor() as u32 + 1;
    if mip_count <= 1 {
        return None;
    }

    // Sum the byte-size of every NON-base mip (levels 1..mip_count-1) —
    // these sit BEFORE the base mip in the file, smallest first.
    let mut skip = 0usize;
    for level in 1..mip_count {
        let mw = (width >> level).max(1);
        let mh = (height >> level).max(1);
        skip += mip_byte_size(mw, mh, bytes_per_block, block_dim);
    }

    let base_size = mip_byte_size(width, height, bytes_per_block, block_dim);
    let pixel_start = 12usize.checked_add(skip)?;
    let pixel_end = pixel_start.checked_add(base_size)?;
    if pixel_end > data.len() {
        return None;
    }

    let mut out = Vec::with_capacity(12 + base_size);
    out.extend_from_slice(&data[..12]);
    out[11] &= !TEX_FLAG_HAS_MIPMAPS;
    out.extend_from_slice(&data[pixel_start..pixel_end]);

    Some(out)
}

/// Returns `(bytes_per_block, block_dim)` — `block_dim` is 4 for BCn, 1 for
/// uncompressed BGRA8. Matches Riot's TEX format byte values.
fn tex_format_block_info(format_byte: u8) -> Option<(usize, usize)> {
    match format_byte {
        10 => Some((8, 4)),  // BC1 / DXT1
        12 => Some((16, 4)), // BC3 / DXT5
        20 => Some((4, 1)),  // BGRA8
        _ => None,
    }
}

fn mip_byte_size(width: u32, height: u32, bytes_per_block: usize, block_dim: usize) -> usize {
    let blocks_w = (width as usize).div_ceil(block_dim);
    let blocks_h = (height as usize).div_ceil(block_dim);
    blocks_w * blocks_h * bytes_per_block
}

// ── tiny helpers ─────────────────────────────────────────────────────

fn read_u32_le(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
}

fn write_u32_le(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_dds_dxt5(width: u32, height: u32, mip_count: u32) -> Vec<u8> {
        let mut data = vec![0u8; 128];
        data[..4].copy_from_slice(b"DDS ");
        write_u32_le(&mut data, 0x04, 124);
        write_u32_le(&mut data, 0x08, 0x000A_1007);
        write_u32_le(&mut data, 0x0C, height);
        write_u32_le(&mut data, 0x10, width);
        write_u32_le(&mut data, 0x1C, mip_count);
        write_u32_le(&mut data, 0x4C, 32);
        write_u32_le(&mut data, 0x50, 4);
        data[0x54..0x58].copy_from_slice(b"DXT5");
        write_u32_le(&mut data, 0x6C, DDSCAPS_MIPMAP | DDSCAPS_COMPLEX | 0x1000);

        for level in 0..mip_count {
            let w = (width >> level).max(1);
            let h = (height >> level).max(1);
            let bytes = (w as usize).div_ceil(4) * (h as usize).div_ceil(4) * 16;
            data.extend(std::iter::repeat(level as u8).take(bytes));
        }
        data
    }

    fn make_tex(width: u32, height: u32, has_mipmaps: bool) -> Vec<u8> {
        let mut data = vec![0u8; 12];
        data[..4].copy_from_slice(b"TEX\0");
        data[4..6].copy_from_slice(&(width as u16).to_le_bytes());
        data[6..8].copy_from_slice(&(height as u16).to_le_bytes());
        data[9] = 12;
        data[11] = if has_mipmaps { 1 } else { 0 };

        if has_mipmaps {
            let max_dim = width.max(height);
            let mip_count = (max_dim as f64).log2().floor() as u32 + 1;
            for level in (0..mip_count).rev() {
                let w = (width >> level).max(1);
                let h = (height >> level).max(1);
                let bytes = (w as usize).div_ceil(4) * (h as usize).div_ceil(4) * 16;
                data.extend(std::iter::repeat(level as u8).take(bytes));
            }
        } else {
            let bytes = (width as usize).div_ceil(4) * (height as usize).div_ceil(4) * 16;
            data.extend(std::iter::repeat(0xAA).take(bytes));
        }
        data
    }

    #[test]
    fn sniffer_detects_dds_magic() {
        let dds = make_dds_dxt5(64, 64, 1);
        assert!(is_dds(&dds));
        assert!(!is_tex(&dds));
    }

    #[test]
    fn sniffer_detects_tex_magic() {
        let tex = make_tex(64, 64, false);
        assert!(is_tex(&tex));
        assert!(!is_dds(&tex));
    }

    #[test]
    fn sniffer_rejects_random_bytes() {
        let junk = b"NOT A TEXTURE FILE AT ALL".to_vec();
        assert!(!is_dds(&junk));
        assert!(!is_tex(&junk));
    }

    #[test]
    fn sniffer_rejects_short_buffers() {
        assert!(!is_dds(b"DDS "));
        assert!(!is_tex(b"TEX"));
    }

    #[test]
    fn auto_passthrough_for_unknown_bytes() {
        let junk = b"hello world, definitely not a texture".to_vec();
        let out = strip_mipmaps_auto(&junk).unwrap();
        assert_eq!(out, junk);
    }

    #[test]
    fn auto_dispatches_to_dds_path() {
        let original = make_dds_dxt5(256, 256, 9);
        let stripped = strip_mipmaps_auto(&original).unwrap();
        assert_eq!(stripped.len(), 128 + 65_536);
        assert_eq!(read_u32_le(&stripped, 0x1C), 1);
    }

    #[test]
    fn auto_dispatches_to_tex_path() {
        let original = make_tex(256, 256, true);
        let stripped = strip_mipmaps_auto(&original).unwrap();
        assert_eq!(stripped.len(), 12 + 65_536);
        assert_eq!(stripped[11] & TEX_FLAG_HAS_MIPMAPS, 0);
    }

    #[test]
    fn dds_strips_to_base_level() {
        let original = make_dds_dxt5(256, 256, 9);
        let stripped = strip_mipmaps_dds(&original).unwrap();
        assert_eq!(stripped.len(), 128 + 65_536);
        assert_eq!(read_u32_le(&stripped, 0x1C), 1);
        assert_eq!(read_u32_le(&stripped, 0x08) & DDSD_MIPMAPCOUNT, 0);
        assert_eq!(read_u32_le(&stripped, 0x6C) & DDSCAPS_MIPMAP, 0);
    }

    #[test]
    fn dds_no_mips_returns_input_unchanged() {
        let original = make_dds_dxt5(256, 256, 1);
        let out = strip_mipmaps_dds(&original).unwrap();
        assert_eq!(out, original);
    }

    #[test]
    fn tex_strips_to_base_level_largest_byte() {
        let original = make_tex(256, 256, true);
        let stripped = strip_mipmaps_tex(&original).unwrap();
        assert_eq!(stripped.len(), 12 + 65_536);
        assert_eq!(stripped[11] & TEX_FLAG_HAS_MIPMAPS, 0);
        assert!(stripped[12..].iter().all(|&b| b == 0));
    }

    #[test]
    fn tex_no_mips_returns_input_unchanged() {
        let original = make_tex(256, 256, false);
        let out = strip_mipmaps_tex(&original).unwrap();
        assert_eq!(out, original);
    }
}
