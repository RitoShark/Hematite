//! Combo-bin relocation — re-key legacy `data/<champ>_skins_<slots>.bin`
//! WAD entries to Riot's relocated path.
//!
//! Ported from the Celestial launcher's combo-bin fixer. Old-style mods
//! ship a "combo" BIN naming multiple skins at
//! `data/<champ>_skins_<slots>.bin` (e.g. `data/gragas_skins_0_1.bin`).
//! Riot has since moved this file server-side to
//! `data/characters/<champ>/<champ>_multi_skins_<slots>.bin` — the old
//! path is silently dead in the live client, so the combo BIN a mod ships
//! never actually loads.
//!
//! This module re-keys the WAD entry (hash + path) to the new location
//! without touching its bytes, but only when it's safe to do so:
//!
//! - The mod must not ship any per-skin BINs (`skins/skin<N>.bin`) for the
//!   same champion — if it did, relocating the combo BIN could orphan or
//!   conflict with content the mod already placed correctly (Celestial's
//!   gate).
//! - The live/game-WAD source must confirm the *relocated* path actually
//!   exists — otherwise this would just move the dead reference to a
//!   different dead path.
//! - The mod must not already ship an entry at the relocated hash (no
//!   clobbering existing content).

use hematite_file::wad_adapter::wad_path_hash;
use std::collections::HashSet;

/// A parsed legacy combo-bin path: `data/<champion>_skins_<slots>.bin`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComboBin {
    /// Lower-cased champion name (alphanumeric only — no underscores).
    pub champion: String,
    /// Raw slots segment, e.g. `"0_1_5"` or `"12"`.
    pub slots: String,
}

/// Match `data/<champ>_skins_<slots>.bin` (case-insensitive).
///
/// - Must live directly under the `data/` root (not `assets/`, not any
///   subdirectory — per-skin bins already live under
///   `data/characters/...` and would never match this shape anyway, but
///   this also excludes unrelated roots like `assets/`).
/// - Split at the FIRST `_skins_` occurrence: everything before it is the
///   champion segment, everything after (minus the `.bin` suffix) is the
///   slots segment.
/// - Champion segment: non-empty, `[a-z0-9]+` only (no underscores, no
///   `/`). Rejects the champion segment containing a `/` (i.e. paths with
///   extra directory components before the filename).
/// - Slots segment: non-empty, digits and underscores only (e.g. `"0_1_5"`,
///   `"12"`).
pub fn combo_bin_pattern(path: &str) -> Option<ComboBin> {
    let lower = path.to_lowercase().replace('\\', "/");
    let rest = lower.strip_prefix("data/")?;
    // Reject anything with additional path separators — must be a
    // filename directly under data/.
    if rest.contains('/') {
        return None;
    }
    let stem = rest.strip_suffix(".bin")?;
    let idx = stem.find("_skins_")?;
    let champion = &stem[..idx];
    let slots = &stem[idx + "_skins_".len()..];

    if champion.is_empty() || !champion.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    if slots.is_empty()
        || !slots
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'_')
    {
        return None;
    }

    Some(ComboBin {
        champion: champion.to_string(),
        slots: slots.to_string(),
    })
}

/// Riot's relocated path for a parsed combo bin:
/// `data/characters/{champion}/{champion}_multi_skins_{slots}.bin`
pub fn relocated_path(c: &ComboBin) -> String {
    format!(
        "data/characters/{champ}/{champ}_multi_skins_{slots}.bin",
        champ = c.champion,
        slots = c.slots
    )
}

/// True if the mod ships at least one per-skin BIN
/// (`data/characters/<champ>/skins/skin<N>.bin`), detected via
/// [`hematite_core::seeds::discover_seeds`].
///
/// Relocation is gated on this being false: if the mod already ships
/// proper per-skin BINs, relocating the legacy combo BIN on top risks
/// orphaning or conflicting with content the mod already placed
/// correctly.
pub fn mod_ships_per_skin_bins(all_files: &[(u64, String, Vec<u8>)]) -> bool {
    !hematite_core::seeds::discover_seeds(all_files.iter().map(|(_, p, _)| p.as_str())).is_empty()
}

/// Re-key legacy combo-bin WAD entries to Riot's relocated path.
///
/// For each entry whose path matches [`combo_bin_pattern`]:
/// - Gate: skip entirely (all combo bins, return 0) if
///   [`mod_ships_per_skin_bins`] is true.
/// - Compute the relocated path. If `game_hashes` contains its hash AND
///   the mod doesn't already ship an entry at that hash, re-key the entry
///   in place (hash + path updated, bytes untouched) and count it.
/// - Otherwise (game doesn't have the relocated path, or the mod already
///   ships it), leave the entry untouched.
///
/// Returns the number of entries relocated.
pub fn relocate_combo_bins(
    all_files: &mut [(u64, String, Vec<u8>)],
    game_hashes: &HashSet<u64>,
) -> u32 {
    if mod_ships_per_skin_bins(all_files) {
        tracing::debug!(
            "combo-bin relocation: mod ships per-skin BIN(s) — skipping (gate blocks relocation)"
        );
        return 0;
    }

    // Snapshot hashes the mod already ships, so a combo bin never gets
    // re-keyed onto a hash the mod already has an entry for.
    let existing_hashes: HashSet<u64> = all_files.iter().map(|(h, _, _)| *h).collect();

    let mut count = 0u32;
    for (hash, path, _bytes) in all_files.iter_mut() {
        let Some(combo) = combo_bin_pattern(path) else {
            continue;
        };
        let new_path = relocated_path(&combo);
        let new_hash = wad_path_hash(&new_path);

        if !game_hashes.contains(&new_hash) {
            tracing::debug!(
                "combo-bin relocation: game lacks relocated path for {} ({} -> {}) — leaving untouched",
                path,
                path,
                new_path
            );
            continue;
        }
        if existing_hashes.contains(&new_hash) {
            tracing::debug!(
                "combo-bin relocation: mod already ships an entry at {} — leaving {} untouched",
                new_path,
                path
            );
            continue;
        }

        tracing::info!("combo-bin relocation: {} -> {}", path, new_path);
        *hash = new_hash;
        *path = new_path;
        count += 1;
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_matches_basic() {
        let c = combo_bin_pattern("data/gragas_skins_0_1.bin").unwrap();
        assert_eq!(c.champion, "gragas");
        assert_eq!(c.slots, "0_1");
    }

    #[test]
    fn pattern_matches_single_slot_and_case_insensitive() {
        let c = combo_bin_pattern("DATA/Gragas_Skins_12.BIN").unwrap();
        assert_eq!(c.champion, "gragas");
        assert_eq!(c.slots, "12");
    }

    #[test]
    fn pattern_rejects_per_skin_and_noise() {
        assert!(combo_bin_pattern("data/characters/gragas/skins/skin0.bin").is_none());
        assert!(combo_bin_pattern("data/gragas.bin").is_none());
        // assets/ root must NOT match per brief.
        assert!(combo_bin_pattern("assets/gragas_skins_0.bin").is_none());
    }

    #[test]
    fn pattern_rejects_missing_skins_marker() {
        assert!(combo_bin_pattern("data/gragas_0_1.bin").is_none());
    }

    #[test]
    fn pattern_rejects_non_bin_extension() {
        assert!(combo_bin_pattern("data/gragas_skins_0_1.dds").is_none());
    }

    #[test]
    fn pattern_rejects_empty_slots() {
        assert!(combo_bin_pattern("data/gragas_skins_.bin").is_none());
    }

    #[test]
    fn pattern_splits_at_first_skins_marker() {
        // Split at the FIRST "_skins_" occurrence. Since the slots segment
        // after that first marker ("bar_skins_0") contains letters, it's
        // not a valid slots segment and the whole path must not match.
        assert!(combo_bin_pattern("data/foo_skins_bar_skins_0.bin").is_none());
    }

    #[test]
    fn relocated_path_format() {
        let c = ComboBin {
            champion: "gragas".to_string(),
            slots: "0_1".to_string(),
        };
        assert_eq!(
            relocated_path(&c),
            "data/characters/gragas/gragas_multi_skins_0_1.bin"
        );
    }

    #[test]
    fn gate_blocks_when_per_skin_bins_present() {
        let mut all_files = vec![
            (
                1u64,
                "data/gragas_skins_0_1.bin".to_string(),
                b"bytes".to_vec(),
            ),
            (
                2u64,
                "data/characters/gragas/skins/skin0.bin".to_string(),
                b"bytes".to_vec(),
            ),
        ];
        let new_path = "data/characters/gragas/gragas_multi_skins_0_1.bin";
        let mut game_hashes = HashSet::new();
        game_hashes.insert(wad_path_hash(new_path));

        let count = relocate_combo_bins(&mut all_files, &game_hashes);

        assert_eq!(count, 0);
        assert_eq!(all_files[0].1, "data/gragas_skins_0_1.bin");
    }

    #[test]
    fn relocate_rekeys_when_game_has_target() {
        let mut all_files = vec![(
            1u64,
            "data/gragas_skins_0_1.bin".to_string(),
            b"bytes".to_vec(),
        )];
        let new_path = "data/characters/gragas/gragas_multi_skins_0_1.bin";
        let new_hash = wad_path_hash(new_path);
        let mut game_hashes = HashSet::new();
        game_hashes.insert(new_hash);

        let count = relocate_combo_bins(&mut all_files, &game_hashes);

        assert_eq!(count, 1);
        assert_eq!(all_files[0].0, new_hash);
        assert_eq!(all_files[0].1, new_path);
        assert_eq!(all_files[0].2, b"bytes".to_vec());
    }

    #[test]
    fn relocate_skips_when_game_lacks_target() {
        let mut all_files = vec![(
            1u64,
            "data/gragas_skins_0_1.bin".to_string(),
            b"bytes".to_vec(),
        )];
        let game_hashes = HashSet::new();

        let count = relocate_combo_bins(&mut all_files, &game_hashes);

        assert_eq!(count, 0);
        assert_eq!(all_files[0].0, 1u64);
        assert_eq!(all_files[0].1, "data/gragas_skins_0_1.bin");
    }

    #[test]
    fn relocate_skips_when_mod_ships_target_hash() {
        let new_path = "data/characters/gragas/gragas_multi_skins_0_1.bin";
        let new_hash = wad_path_hash(new_path);
        let mut all_files = vec![
            (
                1u64,
                "data/gragas_skins_0_1.bin".to_string(),
                b"combo bytes".to_vec(),
            ),
            (
                new_hash,
                new_path.to_string(),
                b"already shipped bytes".to_vec(),
            ),
        ];
        let mut game_hashes = HashSet::new();
        game_hashes.insert(new_hash);

        let count = relocate_combo_bins(&mut all_files, &game_hashes);

        assert_eq!(count, 0);
        assert_eq!(all_files[0].1, "data/gragas_skins_0_1.bin");
        assert_eq!(all_files[1].2, b"already shipped bytes".to_vec());
    }

    #[test]
    fn non_combo_names_untouched() {
        let mut all_files = vec![(
            1u64,
            "data/characters/gragas/gragas.bin".to_string(),
            b"bytes".to_vec(),
        )];
        let mut game_hashes = HashSet::new();
        game_hashes.insert(wad_path_hash(
            "data/characters/gragas/gragas_multi_skins_0.bin",
        ));

        let count = relocate_combo_bins(&mut all_files, &game_hashes);

        assert_eq!(count, 0);
        assert_eq!(all_files[0].0, 1u64);
    }
}
