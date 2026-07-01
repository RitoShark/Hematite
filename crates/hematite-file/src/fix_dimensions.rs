//! Fix non-block-aligned dimensions on Riot TEX textures.
//!
//! Why: BCn-compressed textures encode data as 4×4 pixel blocks. When a TEX
//! ships with a width or height that isn't a multiple of 4, the engine
//! reads garbage past the end of the buffer for the bottom/right edges and
//! the texture renders as noise — or crashes outright on some patches.
//!
//! The fix is the same Python / Celestial path: round each non-conforming
//! dimension **down** to the nearest multiple of 4 and crop the pixel data
//! to match. The header is re-stamped with the new width/height; everything
//! else is preserved. BGRA8 textures bypass this since they have no block
//! alignment constraint.
//!
//! ## API
//! - [`fix_tex_dimensions`] — byte-in / byte-out, single TEX.
//! - [`fix_dimensions_auto`] — sniff + dispatch wrapper (TEX only today;
//!   DDS textures already encode pitch / linear-size in the header so
//!   under-aligned dimensions are typically rejected by the parser long
//!   before they reach the GPU).

use anyhow::Result;

use crate::strip_mipmaps::is_tex;

const TEX_FLAG_HAS_MIPMAPS: u8 = 1;

/// Bytes-per-block and block dimension for a TEX `format` byte. `None` for
/// formats we don't know how to crop. Matches the table in
/// [`strip_mipmaps`] for consistency.
fn tex_format_block_info(format_byte: u8) -> Option<(usize, usize)> {
    match format_byte {
        10 => Some((8, 4)),  // BC1 / DXT1
        12 => Some((16, 4)), // BC3 / DXT5
        20 => Some((4, 1)),  // BGRA8 (no block constraint)
        _ => None,
    }
}

/// Round a dimension *down* to the nearest multiple of `block`.
fn align_down(value: u32, block: u32) -> u32 {
    if block <= 1 {
        return value;
    }
    (value / block) * block
}

/// Sniff the first bytes; for known textures, dispatch to the right fixer.
/// Returns the input unchanged for unknown formats.
pub fn fix_dimensions_auto(bytes: &[u8]) -> Result<Vec<u8>> {
    if is_tex(bytes) {
        return fix_tex_dimensions(bytes);
    }
    Ok(bytes.to_vec())
}

/// Fix TEX dimensions in-place. Returns the (possibly rewritten) bytes.
///
/// The function is a no-op when:
/// * the buffer is too short to be a TEX,
/// * the format byte isn't recognised,
/// * width and height are both already block-aligned,
/// * cropping would overflow the buffer,
/// * the resource type isn't plain 2D (cubemaps / volume textures pack
///   data differently — see the same guard in [`strip_mipmaps_tex`]).
pub fn fix_tex_dimensions(data: &[u8]) -> Result<Vec<u8>> {
    let Some(stripped) = fix_tex_dimensions_inner(data) else {
        return Ok(data.to_vec());
    };
    tracing::debug!(
        "fixed TEX dimensions: {} → {} bytes",
        data.len(),
        stripped.len()
    );
    Ok(stripped)
}

fn fix_tex_dimensions_inner(data: &[u8]) -> Option<Vec<u8>> {
    if !is_tex(data) {
        return None;
    }

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
    let block = block_dim as u32;
    if block <= 1 {
        // BGRA8 (or future uncompressed formats): no alignment requirement.
        return None;
    }

    let new_w = align_down(width, block);
    let new_h = align_down(height, block);
    if new_w == width && new_h == height {
        return None;
    }
    if new_w == 0 || new_h == 0 {
        // Refusing to produce a 0-pixel texture — caller keeps the
        // original (potentially broken) bytes, which is still safer.
        tracing::warn!(
            "fix_tex_dimensions: cropping would yield 0×0 ({}×{}), keeping original",
            width,
            height
        );
        return None;
    }

    let flags = data[11];
    let has_mips = flags & TEX_FLAG_HAS_MIPMAPS != 0;
    let max_dim = width.max(height);
    let mip_count = if has_mips {
        (max_dim as f64).log2().floor() as u32 + 1
    } else {
        1
    };

    // TEX stores mips smallest→largest. The base level is always the last
    // chunk in the file. We crop only the base level — sub-mips can stay
    // garbage because mipmap-strip will toss them next if enabled.
    let mut pre_base = 0usize;
    if has_mips {
        for level in 1..mip_count {
            let mw = (width >> level).max(1);
            let mh = (height >> level).max(1);
            pre_base += mip_byte_size(mw, mh, bytes_per_block, block_dim);
        }
    }

    let base_size_old = mip_byte_size(width, height, bytes_per_block, block_dim);
    let base_size_new = mip_byte_size(new_w, new_h, bytes_per_block, block_dim);
    let pixel_start = 12usize.checked_add(pre_base)?;
    let pixel_end_old = pixel_start.checked_add(base_size_old)?;
    if pixel_end_old > data.len() {
        return None;
    }

    // Row-by-row copy of the cropped base level. Each "row" is a row of
    // blocks of width `blocks_per_row_old`; we keep only the first
    // `blocks_per_row_new` of each row, and only the first
    // `block_rows_new` rows.
    let blocks_per_row_old = (width as usize).div_ceil(block_dim);
    let blocks_per_row_new = (new_w as usize).div_ceil(block_dim);
    let block_rows_new = (new_h as usize).div_ceil(block_dim);

    let mut cropped_base = Vec::with_capacity(base_size_new);
    let base = &data[pixel_start..pixel_end_old];
    let row_bytes_old = blocks_per_row_old * bytes_per_block;
    let row_bytes_new = blocks_per_row_new * bytes_per_block;
    for row_idx in 0..block_rows_new {
        let row_start = row_idx * row_bytes_old;
        let row_end = row_start.checked_add(row_bytes_new)?;
        if row_end > base.len() {
            return None;
        }
        cropped_base.extend_from_slice(&base[row_start..row_end]);
    }

    // Re-emit header with new width / height stamped in. Everything else
    // (resource_type, format, flags) is preserved.
    let mut out = Vec::with_capacity(12 + pre_base + base_size_new);
    out.extend_from_slice(&data[..12]);
    out[4..6].copy_from_slice(&(new_w as u16).to_le_bytes());
    out[6..8].copy_from_slice(&(new_h as u16).to_le_bytes());

    // Preserve sub-mip bytes verbatim. The dimensions in the header refer
    // to the BASE level; the engine derives sub-mip sizes by shifting.
    // Our shifted sizes would be (new_w >> level) etc., but the sub-mip
    // bytes in the original buffer were sized for the OLD width/height.
    // Skipping the sub-mip preservation entirely is safer than copying
    // misaligned bytes — pair with `strip_mipmaps` for the cleanest result.
    if has_mips {
        tracing::debug!(
            "fix_tex_dimensions: textured had mips; dropping mip chain since \
             the base level was cropped — re-run strip_mipmaps_tex if you need \
             a HasMipMaps=0 buffer"
        );
        // Clear HasMipMaps so the engine doesn't try to read non-existent sub-levels.
        out[11] &= !TEX_FLAG_HAS_MIPMAPS;
    }
    out.extend_from_slice(&cropped_base);

    Some(out)
}

fn mip_byte_size(width: u32, height: u32, bytes_per_block: usize, block_dim: usize) -> usize {
    let blocks_w = (width as usize).div_ceil(block_dim);
    let blocks_h = (height as usize).div_ceil(block_dim);
    blocks_w * blocks_h * bytes_per_block
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic 1-mip BC1 TEX with the given dimensions and a
    /// pixel buffer filled with a recognisable pattern.
    fn synth_bc1_tex(width: u16, height: u16) -> Vec<u8> {
        let block_dim = 4usize;
        let bytes_per_block = 8usize;
        let blocks_w = (width as usize).div_ceil(block_dim);
        let blocks_h = (height as usize).div_ceil(block_dim);
        let size = blocks_w * blocks_h * bytes_per_block;

        let mut buf = Vec::with_capacity(12 + size);
        buf.extend_from_slice(b"TEX\0");
        buf.extend_from_slice(&width.to_le_bytes());
        buf.extend_from_slice(&height.to_le_bytes());
        // unk, format=10 (BC1), resource_type=0, flags=0
        buf.push(0);
        buf.push(10);
        buf.push(0);
        buf.push(0);
        // Pixel data: block index modulo 0xFF as a poor-man's checksum.
        for i in 0..size {
            buf.push((i % 0xFF) as u8);
        }
        buf
    }

    #[test]
    fn aligned_tex_is_passthrough() {
        let buf = synth_bc1_tex(8, 8);
        let out = fix_tex_dimensions(&buf).unwrap();
        assert_eq!(buf, out);
    }

    #[test]
    fn misaligned_width_is_cropped_down() {
        // 6×4: width rounds down to 4. Result should be 4×4 BC1 = 8 bytes.
        let buf = synth_bc1_tex(6, 4);
        let out = fix_tex_dimensions(&buf).unwrap();
        assert_ne!(buf, out);
        assert_eq!(out.len(), 12 + 8);
        assert_eq!(&out[..4], b"TEX\0");
        assert_eq!(u16::from_le_bytes([out[4], out[5]]), 4);
        assert_eq!(u16::from_le_bytes([out[6], out[7]]), 4);
    }

    #[test]
    fn misaligned_height_is_cropped_down() {
        // 4×6: height rounds down to 4.
        let buf = synth_bc1_tex(4, 6);
        let out = fix_tex_dimensions(&buf).unwrap();
        assert_eq!(out.len(), 12 + 8);
        assert_eq!(u16::from_le_bytes([out[6], out[7]]), 4);
    }

    #[test]
    fn would_crop_to_zero_keeps_original() {
        // 2×2 BC1: aligning to 4 would give 0×0 — keep original bytes.
        let buf = synth_bc1_tex(2, 2);
        let out = fix_tex_dimensions(&buf).unwrap();
        assert_eq!(buf, out);
    }

    #[test]
    fn bgra8_has_no_block_constraint() {
        // BGRA8 = format byte 20, 1×1 byte block.
        let mut buf = Vec::new();
        buf.extend_from_slice(b"TEX\0");
        buf.extend_from_slice(&3u16.to_le_bytes());
        buf.extend_from_slice(&5u16.to_le_bytes());
        buf.push(0);
        buf.push(20);
        buf.push(0);
        buf.push(0);
        buf.extend_from_slice(&[0u8; 3 * 5 * 4]);
        let out = fix_tex_dimensions(&buf).unwrap();
        assert_eq!(buf, out);
    }

    #[test]
    fn non_tex_is_passthrough() {
        let buf = vec![0u8; 32];
        let out = fix_tex_dimensions(&buf).unwrap();
        assert_eq!(buf, out);
    }

    #[test]
    fn auto_routes_tex_only() {
        let buf = synth_bc1_tex(6, 4);
        let out = fix_dimensions_auto(&buf).unwrap();
        assert_eq!(out.len(), 12 + 8);
    }
}
