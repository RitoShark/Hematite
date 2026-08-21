//! `--restore-anm` — pull dead `.anm` animation references from the live
//! game instead of deleting them.
//!
//! Ported from the Celestial launcher's `anm_restore` step. Where
//! `anm_remover` deletes BIN references to missing animation files (to
//! avoid a hard crash), `--restore-anm` instead tries to make the
//! reference valid again by pulling the missing `.anm` bytes out of the
//! base game (via [`crate::deep_repair::GamePullSource`]) and shipping them
//! alongside the mod.
//!
//! This module deliberately does NOT depend on `--repath` being active —
//! it runs as its own pipeline step so animation restoration works for
//! mods that never repath their assets at all.

use crate::deep_repair::{self, GamePullSource};
use hematite_core::repath as repath_core;
use hematite_core::traits::BinProvider;
use std::collections::HashSet;

/// Outcome of a [`restore_missing_anms`] run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AnmRestoreStats {
    /// Distinct `.anm` paths referenced by the mod's BINs.
    pub refs_found: u32,
    /// `.anm` files successfully pulled from the game source and added to
    /// the mod.
    pub restored: u32,
    /// Referenced `.anm` paths that could not be located in the game
    /// source (missing there too).
    pub still_missing: u32,
}

/// Collect every distinct `.anm` path referenced by the mod's own BINs.
///
/// Scans `all_files` for entries that look like BINs (path ends `.bin` or
/// content starts with the `PROP`/`PTCH` magic), parses each, and reuses
/// [`repath_core::collect_bin_asset_paths`] to pull every asset-path string
/// out of the tree (including `.linked` dependency paths) — then filters
/// that list down to `.anm` references (case-insensitive). BINs that fail
/// to parse are skipped (same tolerance as the deep-repair closure loop).
pub fn collect_anm_refs(
    all_files: &[(u64, String, Vec<u8>)],
    bin_provider: &dyn BinProvider,
) -> HashSet<String> {
    let mut refs = HashSet::new();
    for (_, path, bytes) in all_files {
        let is_bin = path.to_lowercase().ends_with(".bin") || repath_core::looks_like_bin(bytes);
        if !is_bin {
            continue;
        }
        let tree = match bin_provider.parse_bytes(bytes) {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!("Skipping BIN at {} while collecting .anm refs: {}", path, e);
                continue;
            }
        };
        // skip_vo=true: VO paths are never .anm references, and this
        // matches the default the rest of the pipeline uses.
        for p in repath_core::collect_bin_asset_paths(&tree, true) {
            if p.to_lowercase().ends_with(".anm") {
                refs.insert(p);
            }
        }
    }
    refs
}

/// Pull every `.anm` reference the mod's BINs point at but doesn't ship,
/// out of `source` (the live game index or an explicit `--game-wad`).
///
/// Algorithm:
/// 1. Collect distinct `.anm` refs via [`collect_anm_refs`].
/// 2. Build a [`repath_core::WadIndex`] of what the mod currently ships.
/// 3. For each ref not already present in the mod, pull it via
///    [`deep_repair::pull_one`] (shares the suffix-strip + canonical
///    resolution ladder with deep repair — no logic duplicated here).
///
/// Newly pulled files are appended to `all_files` under the *requested*
/// path's hash, exactly like `deep_repair::pull_one`'s normal contract.
pub fn restore_missing_anms(
    all_files: &mut Vec<(u64, String, Vec<u8>)>,
    bin_provider: &dyn BinProvider,
    source: &mut dyn GamePullSource,
) -> AnmRestoreStats {
    let mut stats = AnmRestoreStats::default();

    let refs = collect_anm_refs(all_files, bin_provider);
    stats.refs_found = refs.len() as u32;
    if refs.is_empty() {
        return stats;
    }

    let index =
        repath_core::WadIndex::from_entries(all_files.iter().map(|(h, p, _)| (*h, p.clone())));

    // Sort for deterministic ordering (HashSet iteration order isn't
    // stable, and stable output makes logs/tests reproducible).
    let mut sorted_refs: Vec<&String> = refs.iter().collect();
    sorted_refs.sort();

    for anm_ref in sorted_refs {
        if index.has(anm_ref, hematite_file::wad_adapter::wad_path_hash) {
            // Already shipped by the mod — nothing to restore.
            continue;
        }
        match deep_repair::pull_one(anm_ref, source, all_files) {
            Ok(Some((pulled_path, _))) => {
                stats.restored += 1;
                tracing::info!("Restored animation from game: {}", pulled_path);
            }
            Ok(None) => {
                stats.still_missing += 1;
                tracing::debug!("Animation not found in game source: {}", anm_ref);
            }
            Err(e) => {
                stats.still_missing += 1;
                tracing::warn!("Failed to pull animation {}: {}", anm_ref, e);
            }
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deep_repair::WadFileSource;
    use hematite_file::wad_adapter::wad_path_hash;

    /// In-memory BIN provider mirroring `deep_repair`'s test fixture:
    /// bytes are the BIN magic `PROP` followed by a UTF-8 list of
    /// dependency paths separated by `\n`, returned as `.linked` entries
    /// so `collect_bin_asset_paths` (which also surfaces `.linked`) picks
    /// them up without needing a real property tree.
    struct FakeBinProvider;

    impl BinProvider for FakeBinProvider {
        fn parse_bytes(&self, data: &[u8]) -> anyhow::Result<hematite_types::bin::BinTree> {
            anyhow::ensure!(data.len() >= 4 && &data[..4] == b"PROP", "not a fake BIN");
            let body = std::str::from_utf8(&data[4..]).unwrap_or("");
            let linked: Vec<String> = body
                .split('\n')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            Ok(hematite_types::bin::BinTree {
                linked,
                ..Default::default()
            })
        }
        fn write_bytes(&self, _tree: &hematite_types::bin::BinTree) -> anyhow::Result<Vec<u8>> {
            unimplemented!("not needed for restore-anm tests")
        }
    }

    fn fake_bin(deps: &[&str]) -> Vec<u8> {
        let mut v = b"PROP".to_vec();
        v.extend(deps.join("\n").into_bytes());
        v
    }

    /// Minimal fixture `.wad.client` writer, duplicated from
    /// `deep_repair`'s test helper (not exported across the crate's
    /// module boundary as pub).
    fn write_fixture_wad(path: &std::path::Path, entries: &[(&str, &[u8])]) {
        let mut payloads: Vec<Vec<u8>> = Vec::new();
        for (_, data) in entries {
            payloads.push(zstd::stream::encode_all(&data[..], 3).unwrap());
        }
        let header_len = 2 + 2 + 256 + 8 + 4 + 32 * entries.len();
        let mut out = Vec::new();
        out.extend_from_slice(b"RW");
        out.push(3);
        out.push(4);
        out.extend_from_slice(&[0u8; 264]);
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        let mut offset = header_len as u32;
        for ((p, data), comp) in entries.iter().zip(&payloads) {
            out.extend_from_slice(&wad_path_hash(p).to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.push(3); // zstd
            out.push(0);
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u64.to_le_bytes());
            offset += comp.len() as u32;
        }
        for comp in &payloads {
            out.extend_from_slice(comp);
        }
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn collect_anm_refs_finds_anm_strings() {
        let all_files = vec![(
            0u64,
            "data/characters/yone/skins/skin0.bin".to_string(),
            fake_bin(&[
                "data/characters/yone/animations/attack1.anm",
                "data/characters/yone/skins/skin0.dds",
                "ASSETS/Characters/Yone/Animations/Taunt.ANM",
            ]),
        )];

        let refs = collect_anm_refs(&all_files, &FakeBinProvider);

        assert_eq!(refs.len(), 2);
        assert!(refs.contains("data/characters/yone/animations/attack1.anm"));
        // Case is lower-cased by collect_bin_asset_paths.
        assert!(refs.contains("assets/characters/yone/animations/taunt.anm"));
        // Non-.anm refs are filtered out.
        assert!(!refs.iter().any(|r| r.ends_with(".dds")));
    }

    #[test]
    fn collect_anm_refs_ignores_non_bin_entries() {
        let all_files = vec![(
            0u64,
            "data/characters/yone/skins/skin0.dds".to_string(),
            b"not a bin, just texture bytes".to_vec(),
        )];
        let refs = collect_anm_refs(&all_files, &FakeBinProvider);
        assert!(refs.is_empty());
    }

    #[test]
    fn restore_pulls_missing_anm() {
        let dir = tempfile::tempdir().unwrap();
        let wad_path = dir.path().join("Yone.wad.client");
        let anm_bytes = b"ANM DATA HERE".to_vec();
        write_fixture_wad(
            &wad_path,
            &[("data/characters/yone/animations/attack1.anm", &anm_bytes)],
        );
        let mut source = WadFileSource::open(&wad_path).unwrap();

        let mut all_files = vec![(
            0u64,
            "data/characters/yone/skins/skin0.bin".to_string(),
            fake_bin(&["data/characters/yone/animations/attack1.anm"]),
        )];

        let stats = restore_missing_anms(&mut all_files, &FakeBinProvider, &mut source);

        assert_eq!(stats.refs_found, 1);
        assert_eq!(stats.restored, 1);
        assert_eq!(stats.still_missing, 0);

        let requested_hash = wad_path_hash("data/characters/yone/animations/attack1.anm");
        let pulled = all_files
            .iter()
            .find(|(h, _, _)| *h == requested_hash)
            .expect("restored .anm must be present under the requested hash");
        assert_eq!(pulled.1, "data/characters/yone/animations/attack1.anm");
        assert_eq!(pulled.2, anm_bytes);
    }

    #[test]
    fn restore_skips_shipped_anm() {
        // The mod already ships the referenced .anm — restore must not
        // touch the game source or duplicate the entry.
        let dir = tempfile::tempdir().unwrap();
        let wad_path = dir.path().join("Yone.wad.client");
        // Game source deliberately empty — if restore tried to pull, it
        // would report still_missing instead of leaving the count at 0.
        write_fixture_wad(&wad_path, &[]);
        let mut source = WadFileSource::open(&wad_path).unwrap();

        let shipped_path = "data/characters/yone/animations/attack1.anm";
        let mut all_files = vec![
            (
                0u64,
                "data/characters/yone/skins/skin0.bin".to_string(),
                fake_bin(&[shipped_path]),
            ),
            (
                wad_path_hash(shipped_path),
                shipped_path.to_string(),
                b"already shipped bytes".to_vec(),
            ),
        ];

        let stats = restore_missing_anms(&mut all_files, &FakeBinProvider, &mut source);

        assert_eq!(stats.refs_found, 1);
        assert_eq!(stats.restored, 0);
        assert_eq!(stats.still_missing, 0);
        assert_eq!(all_files.len(), 2, "no duplicate entry should be added");
    }

    #[test]
    fn restore_counts_unresolvable() {
        let dir = tempfile::tempdir().unwrap();
        let wad_path = dir.path().join("Yone.wad.client");
        // Game source has nothing — the reference cannot be resolved.
        write_fixture_wad(&wad_path, &[]);
        let mut source = WadFileSource::open(&wad_path).unwrap();

        let mut all_files = vec![(
            0u64,
            "data/characters/yone/skins/skin0.bin".to_string(),
            fake_bin(&["data/characters/yone/animations/gone.anm"]),
        )];

        let stats = restore_missing_anms(&mut all_files, &FakeBinProvider, &mut source);

        assert_eq!(stats.refs_found, 1);
        assert_eq!(stats.restored, 0);
        assert_eq!(stats.still_missing, 1);
        assert_eq!(all_files.len(), 1, "unresolved ref must not be added");
    }

    #[test]
    fn restore_noop_when_no_anm_refs() {
        let dir = tempfile::tempdir().unwrap();
        let wad_path = dir.path().join("Yone.wad.client");
        write_fixture_wad(&wad_path, &[]);
        let mut source = WadFileSource::open(&wad_path).unwrap();

        let mut all_files = vec![(
            0u64,
            "data/characters/yone/skins/skin0.bin".to_string(),
            fake_bin(&["data/characters/yone/skins/skin0.dds"]),
        )];

        let stats = restore_missing_anms(&mut all_files, &FakeBinProvider, &mut source);

        assert_eq!(stats, AnmRestoreStats::default());
        assert_eq!(all_files.len(), 1);
    }
}
