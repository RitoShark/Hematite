//! WAD path lookup and chunk extraction using ltk_wad.

use anyhow::{Context, Result};
use hematite_core::traits::{HashProvider, WadProvider};
use hematite_types::hash::GameHash;
use league_toolkit::wad::Wad;
use std::collections::HashSet;
use std::io::{BufReader, Cursor, Read, Seek};
use std::path::Path;
use xxhash_rust::xxh64::xxh64;

/// Compute the WAD chunk hash for a file path.
///
/// League indexes WAD chunks by `xxhash64(path.to_lowercase())`.
/// Use this when inserting a repathed file into a WAD so the game can
/// find it at its new path.
pub fn wad_path_hash(path: &str) -> u64 {
    xxh64(path.to_lowercase().as_bytes(), 0)
}

/// Strip an inner "tag" segment from a filename's stem.
///
/// Riot occasionally ships files under a stripped name in the live game
/// WAD even though the BIN string references a tagged variant — e.g.
/// `attack1.matcha_ambessa.anm` lives in the WAD as `attack1.anm`. Mods
/// authored against the tagged variant lose the reference unless we look
/// the bytes up under both spellings.
///
/// Returns `Some(stripped)` when the filename has the shape
/// `<stem>.<inner>.<ext>` (two dots, three segments). The inner segment is
/// dropped:
///
/// ```text
///   data/c/yone/anim/attack1.matcha_ambessa.anm
/// → data/c/yone/anim/attack1.anm
/// ```
///
/// Returns `None` for filenames that don't carry an inner tag segment
/// (the common case — single-dot filenames pass through untouched).
pub fn strip_inner_suffix(path: &str) -> Option<String> {
    let (dir, file) = match path.rsplit_once('/') {
        Some((d, f)) => (Some(d), f),
        None => (None, path),
    };
    // Require exactly three segments split by '.': stem . inner . ext
    let parts: Vec<&str> = file.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    if parts[0].is_empty() || parts[1].is_empty() || parts[2].is_empty() {
        return None;
    }
    let stripped_file = format!("{}.{}", parts[0], parts[2]);
    Some(match dir {
        Some(d) => format!("{}/{}", d, stripped_file),
        None => stripped_file,
    })
}

/// Look up the WAD-side hash that corresponds to a BIN-referenced path.
///
/// The BIN's path is always the source of truth — the caller writes any
/// bytes returned under `bin_hash`, not the resolved hash. Lookup variants
/// tried in order of specificity:
///
/// 1. The literal `bin_hash` (`xxhash64(bin_path.to_lowercase())`).
/// 2. The suffix-stripped form (see [`strip_inner_suffix`]) — covers Riot
///    patches that strip the inner `.{tag}` segment from filenames.
///
/// Returns the *game-WAD* hash whose bytes to fetch, or `None` if neither
/// variant exists in the provided hash set.
pub fn resolve_wad_hash_for(
    bin_path: &str,
    bin_hash: u64,
    wad_hashes: &HashSet<u64>,
) -> Option<u64> {
    if wad_hashes.contains(&bin_hash) {
        return Some(bin_hash);
    }
    let stripped = strip_inner_suffix(bin_path)?;
    let stripped_hash = wad_path_hash(&stripped);
    if wad_hashes.contains(&stripped_hash) {
        return Some(stripped_hash);
    }
    None
}

/// WAD provider backed by league-toolkit's ltk_wad.
///
/// Stores only the set of path hashes for fast existence checks.
pub struct LtkWadProvider {
    /// Set of xxhash64 path hashes present in the WAD.
    path_hashes: HashSet<u64>,
}

impl LtkWadProvider {
    /// Create empty WAD provider.
    pub fn new() -> Self {
        Self {
            path_hashes: HashSet::new(),
        }
    }

    /// Build from a WAD file on disk.
    pub fn from_file(path: &Path) -> Result<Self> {
        let file =
            std::fs::File::open(path).with_context(|| format!("Failed to open WAD: {:?}", path))?;
        let reader = BufReader::new(file);
        Self::from_reader(reader)
    }

    /// Build from raw WAD bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let cursor = Cursor::new(data);
        Self::from_reader(cursor)
    }

    /// Internal: Build from any Read+Seek source.
    fn from_reader<R: Read + Seek>(reader: R) -> Result<Self> {
        let wad =
            Wad::mount(reader).map_err(|e| anyhow::anyhow!("Failed to parse WAD: {:?}", e))?;

        let mut provider = Self::new();

        for chunk in wad.chunks() {
            provider.path_hashes.insert(chunk.path_hash);
        }

        Ok(provider)
    }

    /// Get total hash count.
    pub fn hash_count(&self) -> usize {
        self.path_hashes.len()
    }
}

impl Default for LtkWadProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl WadProvider for LtkWadProvider {
    fn has_path(&self, path: &str) -> bool {
        let normalized = path.to_lowercase().replace('\\', "/");
        let hash = xxh64(normalized.as_bytes(), 0);
        self.path_hashes.contains(&hash)
    }

    fn has_hash(&self, hash: u64) -> bool {
        self.path_hashes.contains(&hash)
    }
}

/// Opened WAD file with chunk extraction capabilities.
///
/// Wraps the LTK `Wad` handle to support both path lookups (via `build_provider`)
/// and reading individual chunks (for BIN extraction).
pub struct WadFile<R: Read + Seek> {
    wad: Wad<R>,
}

impl WadFile<BufReader<std::fs::File>> {
    /// Open a WAD file from disk.
    pub fn open(path: &Path) -> Result<Self> {
        let file =
            std::fs::File::open(path).with_context(|| format!("Failed to open WAD: {:?}", path))?;
        let reader = BufReader::new(file);
        let wad =
            Wad::mount(reader).map_err(|e| anyhow::anyhow!("Failed to parse WAD: {:?}", e))?;
        Ok(Self { wad })
    }
}

impl<R: Read + Seek> WadFile<R> {
    // SECURITY: Limits to prevent resource exhaustion from malicious WAD files
    const MAX_CHUNK_SIZE: u64 = 100 * 1024 * 1024; // 100MB per chunk
    const MAX_TOTAL_EXTRACTED: u64 = 2 * 1024 * 1024 * 1024; // 2GB total

    /// Build an `LtkWadProvider` from this WAD's chunk list.
    pub fn build_provider(&self) -> LtkWadProvider {
        let mut provider = LtkWadProvider::new();
        for chunk in self.wad.chunks() {
            provider.path_hashes.insert(chunk.path_hash);
        }
        provider
    }

    /// Set of every chunk's `path_hash`. Cheap snapshot, useful for
    /// suffix-stripped fallback resolution via [`resolve_wad_hash_for`].
    pub fn chunk_hash_set(&self) -> HashSet<u64> {
        self.wad.chunks().iter().map(|c| c.path_hash).collect()
    }

    /// Extract a single chunk by its xxhash64 path hash.
    ///
    /// Returns `Ok(None)` when the hash isn't present in the WAD. Honors
    /// the per-chunk size limit.
    pub fn extract_chunk_by_hash(&mut self, hash: u64) -> Result<Option<Vec<u8>>> {
        let Some(chunk) = self.wad.chunks().get(hash).copied() else {
            return Ok(None);
        };
        let chunk_size = chunk.uncompressed_size as u64;
        if chunk_size > Self::MAX_CHUNK_SIZE {
            tracing::warn!(
                "Refused chunk {:016x}: {} bytes exceeds {} limit",
                hash,
                chunk_size,
                Self::MAX_CHUNK_SIZE
            );
            return Ok(None);
        }
        match self.wad.load_chunk_decompressed(&chunk) {
            Ok(data) => Ok(Some(data.to_vec())),
            Err(e) => {
                tracing::warn!("Failed to extract chunk {:016x}: {e:?}", hash);
                Ok(None)
            }
        }
    }

    /// Look up a chunk by a BIN-referenced *path*, with suffix-stripped
    /// fallback (see [`resolve_wad_hash_for`] / [`strip_inner_suffix`]).
    ///
    /// The returned `Vec<u8>` is the live game-WAD bytes for the requested
    /// asset — callers should store it under the BIN's hash regardless of
    /// whether the lookup hit the literal or stripped form.
    pub fn extract_chunk_for_path(&mut self, bin_path: &str) -> Result<Option<Vec<u8>>> {
        let bin_hash = wad_path_hash(bin_path);
        let hashes = self.chunk_hash_set();
        let Some(src_hash) = resolve_wad_hash_for(bin_path, bin_hash, &hashes) else {
            return Ok(None);
        };
        if src_hash != bin_hash {
            tracing::debug!(
                bin_path = %bin_path,
                "wad_adapter: resolved chunk via suffix-stripped fallback"
            );
        }
        self.extract_chunk_by_hash(src_hash)
    }

    /// Extract all BIN files from the WAD.
    ///
    /// Uses the hash provider to resolve chunk path hashes to file paths,
    /// then extracts chunks whose path ends with `.bin`.
    /// Returns a vec of (resolved_path, decompressed_bytes) pairs.
    pub fn extract_bin_files(
        &mut self,
        hashes: &dyn HashProvider,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        // Collect BIN chunk info first (path_hash + resolved path)
        let bin_chunks: Vec<(u64, String)> = self
            .wad
            .chunks()
            .iter()
            .filter_map(|chunk| {
                let path = hashes.resolve_game_path(GameHash(chunk.path_hash))?;
                if path.to_lowercase().ends_with(".bin") {
                    Some((chunk.path_hash, path.to_string()))
                } else {
                    None
                }
            })
            .collect();

        let mut results = Vec::with_capacity(bin_chunks.len());
        let mut total_extracted: u64 = 0;

        for (path_hash, path) in bin_chunks {
            let Some(chunk) = self.wad.chunks().get(path_hash) else {
                continue;
            };
            let chunk = *chunk;

            // SECURITY: Check chunk size before extraction
            let chunk_size = chunk.uncompressed_size as u64;
            if chunk_size > Self::MAX_CHUNK_SIZE {
                tracing::warn!(
                    "Skipping large BIN chunk {path}: {} bytes exceeds {} bytes limit",
                    chunk_size,
                    Self::MAX_CHUNK_SIZE
                );
                continue;
            }

            // SECURITY: Check total extracted size
            total_extracted = total_extracted.saturating_add(chunk_size);
            if total_extracted > Self::MAX_TOTAL_EXTRACTED {
                anyhow::bail!(
                    "Total extracted BIN size exceeds limit: {} bytes > {} bytes",
                    total_extracted,
                    Self::MAX_TOTAL_EXTRACTED
                );
            }

            match self.wad.load_chunk_decompressed(&chunk) {
                Ok(data) => {
                    results.push((path, data.to_vec()));
                }
                Err(e) => {
                    tracing::warn!("Failed to extract BIN chunk {path}: {e:?}");
                }
            }
        }

        Ok(results)
    }

    /// Extract ALL files from the WAD, preserving original hashes.
    ///
    /// Returns a vec of (path_hash, resolved_path, decompressed_bytes) for ALL chunks.
    /// Custom files without resolved paths use hex format as path but keep original hash.
    pub fn extract_all_files(
        &mut self,
        hashes: &dyn HashProvider,
    ) -> Result<Vec<(u64, String, Vec<u8>)>> {
        // Collect all chunk info (path_hash + resolved path)
        let all_chunks: Vec<(u64, String)> = self
            .wad
            .chunks()
            .iter()
            .map(|chunk| {
                // Try to resolve path from hash database
                let path = hashes
                    .resolve_game_path(GameHash(chunk.path_hash))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        // Custom file not in hash DB - use hex format (will be preserved)
                        tracing::debug!(
                            "Preserving custom file {:016x} (not in hash database)",
                            chunk.path_hash
                        );
                        format!("{:016x}", chunk.path_hash)
                    });
                (chunk.path_hash, path)
            })
            .collect();

        let mut results = Vec::with_capacity(all_chunks.len());
        let mut total_extracted: u64 = 0;

        for (path_hash, path) in all_chunks {
            let Some(chunk) = self.wad.chunks().get(path_hash) else {
                continue;
            };
            let chunk = *chunk;

            // SECURITY: Check chunk size before extraction
            let chunk_size = chunk.uncompressed_size as u64;
            if chunk_size > Self::MAX_CHUNK_SIZE {
                tracing::warn!(
                    "Skipping large chunk {path}: {} bytes exceeds {} bytes limit",
                    chunk_size,
                    Self::MAX_CHUNK_SIZE
                );
                continue;
            }

            // SECURITY: Check total extracted size
            total_extracted = total_extracted.saturating_add(chunk_size);
            if total_extracted > Self::MAX_TOTAL_EXTRACTED {
                anyhow::bail!(
                    "Total extracted size from WAD exceeds limit: {} bytes > {} bytes",
                    total_extracted,
                    Self::MAX_TOTAL_EXTRACTED
                );
            }

            match self.wad.load_chunk_decompressed(&chunk) {
                Ok(data) => {
                    // Store original hash + path + bytes
                    results.push((path_hash, path, data.to_vec()));
                }
                Err(e) => {
                    tracing::debug!("Failed to extract chunk {path}: {e:?}");
                }
            }
        }

        Ok(results)
    }

    /// Extract all BNK files from the WAD.
    ///
    /// Uses the hash provider to resolve chunk path hashes to file paths,
    /// then extracts chunks whose path ends with `.bnk`.
    /// Returns a vec of (resolved_path, decompressed_bytes) pairs.
    pub fn extract_bnk_files(
        &mut self,
        hashes: &dyn HashProvider,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        // Collect BNK chunk info first (path_hash + resolved path)
        let bnk_chunks: Vec<(u64, String)> = self
            .wad
            .chunks()
            .iter()
            .filter_map(|chunk| {
                let path = hashes.resolve_game_path(GameHash(chunk.path_hash))?;
                if path.to_lowercase().ends_with(".bnk") {
                    Some((chunk.path_hash, path.to_string()))
                } else {
                    None
                }
            })
            .collect();

        let mut results = Vec::with_capacity(bnk_chunks.len());
        let mut total_extracted: u64 = 0;

        for (path_hash, path) in bnk_chunks {
            let Some(chunk) = self.wad.chunks().get(path_hash) else {
                continue;
            };
            let chunk = *chunk;

            // SECURITY: Check chunk size before extraction
            let chunk_size = chunk.uncompressed_size as u64;
            if chunk_size > Self::MAX_CHUNK_SIZE {
                tracing::warn!(
                    "Skipping large BNK chunk {path}: {} bytes exceeds {} bytes limit",
                    chunk_size,
                    Self::MAX_CHUNK_SIZE
                );
                continue;
            }

            // SECURITY: Check total extracted size
            total_extracted = total_extracted.saturating_add(chunk_size);
            if total_extracted > Self::MAX_TOTAL_EXTRACTED {
                anyhow::bail!(
                    "Total extracted BNK size exceeds limit: {} bytes > {} bytes",
                    total_extracted,
                    Self::MAX_TOTAL_EXTRACTED
                );
            }

            match self.wad.load_chunk_decompressed(&chunk) {
                Ok(data) => {
                    results.push((path, data.to_vec()));
                }
                Err(e) => {
                    tracing::warn!("Failed to extract BNK chunk {path}: {e:?}");
                }
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_inner_suffix_three_segment_filename() {
        // The Riot rename: drop the inner tag segment.
        assert_eq!(
            strip_inner_suffix("data/characters/ambessa/anim/attack1.matcha_ambessa.anm"),
            Some("data/characters/ambessa/anim/attack1.anm".to_string())
        );
        // Works without a directory.
        assert_eq!(
            strip_inner_suffix("attack1.matcha_ambessa.anm"),
            Some("attack1.anm".to_string())
        );
    }

    #[test]
    fn strip_inner_suffix_passes_through_single_dot() {
        // Normal one-extension filenames have nothing to strip.
        assert_eq!(strip_inner_suffix("data/characters/yone/yone.bin"), None);
        assert_eq!(strip_inner_suffix("attack1.anm"), None);
    }

    #[test]
    fn strip_inner_suffix_rejects_more_than_three_segments() {
        // Don't make decisions about deeply-dotted filenames.
        assert_eq!(strip_inner_suffix("a.b.c.d"), None);
    }

    #[test]
    fn resolve_wad_hash_for_prefers_literal() {
        let mut hashes = HashSet::new();
        let path = "data/characters/yone/anim/attack1.matcha.anm";
        let h = wad_path_hash(path);
        hashes.insert(h);

        // Literal in set → return it.
        assert_eq!(resolve_wad_hash_for(path, h, &hashes), Some(h));
    }

    #[test]
    fn resolve_wad_hash_for_falls_back_to_stripped() {
        let mut hashes = HashSet::new();
        let stripped = "data/characters/yone/anim/attack1.anm";
        hashes.insert(wad_path_hash(stripped));

        // BIN references the tagged form; WAD only has stripped.
        let tagged = "data/characters/yone/anim/attack1.matcha.anm";
        let tagged_hash = wad_path_hash(tagged);
        assert_eq!(
            resolve_wad_hash_for(tagged, tagged_hash, &hashes),
            Some(wad_path_hash(stripped))
        );
    }

    #[test]
    fn resolve_wad_hash_for_returns_none_when_neither_present() {
        let hashes = HashSet::new();
        let path = "data/characters/yone/anim/missing.matcha.anm";
        assert_eq!(
            resolve_wad_hash_for(path, wad_path_hash(path), &hashes),
            None
        );
    }
}
