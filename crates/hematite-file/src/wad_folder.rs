//! WAD *folder* output — the unpacked form cslol/Celestial load directly.
//!
//! Chunks whose path the hash dictionary resolved are written at that path.
//! Chunks it couldn't name keep the 16-hex-digit filename the extractor gave
//! them, at the folder root — the same convention Quartz and cslol use, and
//! the reason [`hex_chunk_hash`] exists: reading such a folder back must
//! restore the ORIGINAL hash instead of hashing the hex string.

use anyhow::{Context, Result};
use std::path::Path;

/// The literal path hash encoded by a root-level 16-hex-digit chunk name
/// (`654b389f7bb124ad`, or `654b389f7bb124ad.bin`), else `None`.
///
/// Restricted to the folder root because that is where the extractor puts
/// unresolved chunks; a nested `assets/x/0123456789abcdef.tex` is a real
/// path that must keep hashing normally.
pub fn hex_chunk_hash(rel_path: &str) -> Option<u64> {
    let normalized = rel_path.replace('\\', "/");
    if normalized.contains('/') {
        return None;
    }
    let stem = normalized.split('.').next()?;
    if stem.len() != 16 || !stem.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(stem, 16).ok()
}

/// Relative-path length past which a chunk is written under its flat hash
/// name instead. Combo BINs (`data/yone_skins_skin0_skins_skin1_…bin`) run to
/// ~700 characters — no Windows filesystem accepts that as one component.
const MAX_REL_LEN: usize = 200;

/// Full-path length past which the same fallback applies, so a deep output
/// directory can't push an otherwise-fine path over MAX_PATH.
const MAX_FULL_LEN: usize = 240;

/// Flat name for a chunk whose real path can't exist on disk: the path hash,
/// keeping the original extension so the file stays identifiable.
fn flat_hash_name(hash: u64, rel: &str) -> String {
    match Path::new(rel).extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{hash:016x}.{ext}"),
        None => format!("{hash:016x}"),
    }
}

/// Write an extracted file list into `dir` as a WAD folder, skipping removed
/// paths. Any existing file or directory at `dir` is replaced.
///
/// Paths too long for the filesystem fall back to their flat hash name —
/// [`hex_chunk_hash`] restores the original hash when the folder is read
/// back, so the chunk still lands correctly even though its name doesn't.
///
/// Returns the number of files written.
pub fn write_wad_folder(
    dir: &Path,
    files: &[(u64, String, Vec<u8>)],
    files_to_remove: &[String],
) -> Result<usize> {
    if dir.is_file() {
        std::fs::remove_file(dir).context("Failed to replace stale WAD file with a folder")?;
    } else if dir.is_dir() {
        std::fs::remove_dir_all(dir).context("Failed to clear existing WAD folder")?;
    }
    std::fs::create_dir_all(dir).context("Failed to create output WAD folder")?;

    let mut written = 0;
    for (hash, path, bytes) in files {
        if files_to_remove.contains(path) {
            tracing::debug!("Excluding removed file: {}", path);
            continue;
        }
        let mut rel = path.replace('\\', "/");
        if rel.split('/').any(|c| c == "..") || Path::new(&rel).is_absolute() {
            anyhow::bail!("Refusing to write chunk outside the WAD folder: {rel}");
        }
        if rel.len() > MAX_REL_LEN || dir.join(&rel).to_string_lossy().len() > MAX_FULL_LEN {
            let flat = flat_hash_name(*hash, &rel);
            tracing::debug!("Path too long for disk, writing as {}: {}", flat, rel);
            rel = flat;
        }
        let dest = dir.join(&rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .context("Failed to create parent directory in WAD folder")?;
        }
        std::fs::write(&dest, bytes).context("Failed to write file in WAD folder")?;
        written += 1;
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_chunk_hash_only_matches_root_level_hex_names() {
        assert_eq!(hex_chunk_hash("654b389f7bb124ad"), Some(0x654b389f7bb124ad));
        assert_eq!(
            hex_chunk_hash("654B389F7BB124AD.bin"),
            Some(0x654b389f7bb124ad)
        );
        // Nested paths are real paths, even when the stem looks like a hash.
        assert_eq!(hex_chunk_hash("assets/x/0123456789abcdef.tex"), None);
        // Wrong length / non-hex.
        assert_eq!(hex_chunk_hash("654b389f7bb124a"), None);
        assert_eq!(hex_chunk_hash("zzzzzzzzzzzzzzzz"), None);
        assert_eq!(hex_chunk_hash("skin0.bin"), None);
    }

    #[test]
    fn write_wad_folder_lays_out_paths_and_skips_removed() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("Rengar.wad.client");
        let files = vec![
            (
                1u64,
                "data/characters/rengar/skins/skin0.bin".to_string(),
                b"PROPdata".to_vec(),
            ),
            (
                0x654b389f7bb124ad,
                "654b389f7bb124ad".to_string(),
                b"raw".to_vec(),
            ),
            (3u64, "assets/gone.tex".to_string(), b"bye".to_vec()),
        ];

        let written = write_wad_folder(&out, &files, &["assets/gone.tex".to_string()]).unwrap();

        assert_eq!(written, 2);
        assert_eq!(
            std::fs::read(out.join("data/characters/rengar/skins/skin0.bin")).unwrap(),
            b"PROPdata"
        );
        assert_eq!(std::fs::read(out.join("654b389f7bb124ad")).unwrap(), b"raw");
        assert!(!out.join("assets/gone.tex").exists());
    }

    #[test]
    fn overlong_paths_fall_back_to_their_flat_hash_name() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("Yone.wad.client");
        // The real shape: a combo BIN naming every skin slot in one segment.
        let combo = format!(
            "data/yone_{}.bin",
            (0..60)
                .map(|n| format!("skins_skin{n}"))
                .collect::<Vec<_>>()
                .join("_")
        );
        assert!(combo.len() > MAX_REL_LEN);
        let hash = 0xabcdef0123456789u64;

        write_wad_folder(&out, &[(hash, combo.clone(), b"PROP".to_vec())], &[]).unwrap();

        let flat = out.join("abcdef0123456789.bin");
        assert!(flat.is_file(), "overlong path must land under its hash");
        // And reading it back restores the original chunk hash.
        assert_eq!(hex_chunk_hash("abcdef0123456789.bin"), Some(hash));
    }

    #[test]
    fn write_wad_folder_replaces_a_stale_packed_wad() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("Rengar.wad.client");
        std::fs::write(&out, b"stale packed wad").unwrap();

        write_wad_folder(&out, &[(1, "a.tex".to_string(), b"x".to_vec())], &[]).unwrap();

        assert!(out.is_dir());
        assert_eq!(std::fs::read(out.join("a.tex")).unwrap(), b"x");
    }
}
