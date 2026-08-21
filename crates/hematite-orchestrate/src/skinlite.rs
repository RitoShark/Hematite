//! Pre-applied SkinLite handling — fix skin0 once instead of ~99 clones.
//!
//! Celestial's SkinLite dupes skin0.bin into slots 1..99 (per-slot SCDP /
//! ResourceResolver rekey, see `hematite_core::skinlite`). Running the fix
//! pipeline over every clone wastes minutes doing identical work. Instead:
//!
//! 1. [`detect_and_strip`] — before the BIN pipeline: for each champion
//!    shipping skin0 + 3 or more extra slots, clone the UNFIXED skin0 per
//!    slot with the ported algorithm and byte-compare against the shipped
//!    slot bin (rs_bin's byte-exact round-trip makes raw comparison fair).
//!    Slots that match are provably clones — removed from the working set.
//!    Slots that differ are real chromas and stay in the normal pipeline.
//! 2. [`reclone`] — after every fix (incl. repath + post-repath phase):
//!    regenerate the stripped slots from the now-FIXED skin0.

use hematite_core::skinlite::clone_skin_tree;
use hematite_core::traits::BinProvider;
use hematite_file::wad_adapter::wad_path_hash;

const MIN_EXTRA_SLOTS: usize = 3;

/// One champion's verified SkinLite clone set.
pub struct SkinLiteSet {
    pub champ: String,
    pub skin0_path: String,
    /// `(slot, original chunk path)` for every verified clone.
    pub slots: Vec<(u32, String)>,
}

fn parse_skin_path(path: &str) -> Option<(String, u32)> {
    let lower = path.to_lowercase().replace('\\', "/");
    let rest = lower
        .strip_prefix("data/characters/")
        .or_else(|| lower.strip_prefix("assets/characters/"))?;
    let (champ, tail) = rest.split_once('/')?;
    let slot = tail
        .strip_prefix("skins/skin")?
        .strip_suffix(".bin")?
        .parse::<u32>()
        .ok()?;
    Some((champ.to_string(), slot))
}

/// Detect pre-applied SkinLite sets and remove the verified clone chunks
/// from `all_files`. Returns one record per champion with clones.
pub fn detect_and_strip(
    all_files: &mut Vec<(u64, String, Vec<u8>)>,
    bin_provider: &dyn BinProvider,
) -> Vec<SkinLiteSet> {
    let mut per_champ: std::collections::HashMap<String, Vec<(u32, usize)>> =
        std::collections::HashMap::new();
    for (i, (_, path, _)) in all_files.iter().enumerate() {
        if let Some((champ, slot)) = parse_skin_path(path) {
            per_champ.entry(champ).or_default().push((slot, i));
        }
    }

    let mut sets = Vec::new();
    let mut strip_indices: Vec<usize> = Vec::new();

    for (champ, mut slots) in per_champ {
        slots.sort();
        let Some(&(_, skin0_idx)) = slots.iter().find(|(s, _)| *s == 0) else {
            continue;
        };
        let extra: Vec<(u32, usize)> = slots.into_iter().filter(|(s, _)| *s != 0).collect();
        if extra.len() < MIN_EXTRA_SLOTS {
            continue;
        }

        let (_, skin0_path, skin0_bytes) = &all_files[skin0_idx];
        let Ok(skin0_tree) = bin_provider.parse_bytes(skin0_bytes) else {
            continue;
        };

        let mut verified: Vec<(u32, String)> = Vec::new();
        for (slot, idx) in extra {
            let Some(clone_tree) = clone_skin_tree(&skin0_tree, &champ, slot) else {
                break;
            };
            let Ok(expected) = bin_provider.write_bytes(&clone_tree) else {
                continue;
            };
            if expected == all_files[idx].2 {
                verified.push((slot, all_files[idx].1.clone()));
                strip_indices.push(idx);
            }
        }

        if !verified.is_empty() {
            tracing::info!(
                "SkinLite pre-applied on {}: {} clone slot(s) verified — fixing skin0 once and recloning",
                champ,
                verified.len()
            );
            sets.push(SkinLiteSet {
                champ,
                skin0_path: skin0_path.clone(),
                slots: verified,
            });
        }
    }

    if !strip_indices.is_empty() {
        strip_indices.sort_unstable();
        for idx in strip_indices.into_iter().rev() {
            all_files.swap_remove(idx);
        }
    }

    sets
}

/// Regenerate every stripped clone slot from the (now fixed) skin0. Returns
/// the number of slots recreated.
pub fn reclone(
    all_files: &mut Vec<(u64, String, Vec<u8>)>,
    sets: &[SkinLiteSet],
    bin_provider: &dyn BinProvider,
) -> u32 {
    let mut recloned = 0u32;
    for set in sets {
        let Some((_, _, skin0_bytes)) = all_files
            .iter()
            .find(|(_, p, _)| p.eq_ignore_ascii_case(&set.skin0_path))
        else {
            tracing::warn!(
                "SkinLite reclone: skin0 for {} disappeared during fixing — slots not recreated",
                set.champ
            );
            continue;
        };
        let Ok(skin0_tree) = bin_provider.parse_bytes(skin0_bytes) else {
            tracing::warn!(
                "SkinLite reclone: fixed skin0 for {} no longer parses — slots not recreated",
                set.champ
            );
            continue;
        };

        for (slot, path) in &set.slots {
            let Some(clone_tree) = clone_skin_tree(&skin0_tree, &set.champ, *slot) else {
                continue;
            };
            match bin_provider.write_bytes(&clone_tree) {
                Ok(bytes) => {
                    all_files.push((wad_path_hash(path), path.clone(), bytes));
                    recloned += 1;
                }
                Err(e) => {
                    tracing::warn!("SkinLite reclone: failed to write skin{slot}: {e}");
                }
            }
        }
    }
    recloned
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_file::bin_adapter::FileBinProvider;
    use hematite_types::bin::{BinObject, BinProperty, BinTree, PropertyValue};
    use hematite_types::hash::{FieldHash, PathHash, TypeHash};
    use indexmap::IndexMap;

    fn fnv(s: &str) -> u32 {
        hematite_core::strings::fnv1a_hash(s)
    }

    fn skin0_bytes(champ: &str, provider: &FileBinProvider) -> Vec<u8> {
        let scdp_key = fnv(&format!("characters/{champ}/skins/skin0"));
        let rr_key = fnv(&format!("characters/{champ}/skins/skin0/resources"));
        let mut properties = IndexMap::new();
        properties.insert(
            fnv("mresourceresolver"),
            BinProperty {
                name_hash: FieldHash(fnv("mresourceresolver")),
                value: PropertyValue::Link(rr_key),
            },
        );
        properties.insert(
            0x22,
            BinProperty {
                name_hash: FieldHash(0x22),
                value: PropertyValue::String("ASSETS/Characters/foo/tex.tex".into()),
            },
        );
        let mut objects = IndexMap::new();
        objects.insert(
            scdp_key,
            BinObject {
                class_hash: TypeHash(fnv("skincharacterdataproperties")),
                path_hash: PathHash(scdp_key),
                properties,
            },
        );
        objects.insert(
            rr_key,
            BinObject {
                class_hash: TypeHash(fnv("resourceresolver")),
                path_hash: PathHash(rr_key),
                properties: IndexMap::new(),
            },
        );
        provider
            .write_bytes(&BinTree {
                objects,
                ..Default::default()
            })
            .unwrap()
    }

    fn cloned_slot_bytes(skin0: &[u8], champ: &str, slot: u32, p: &FileBinProvider) -> Vec<u8> {
        let tree = p.parse_bytes(skin0).unwrap();
        p.write_bytes(&clone_skin_tree(&tree, champ, slot).unwrap())
            .unwrap()
    }

    fn entry(path: &str, bytes: Vec<u8>) -> (u64, String, Vec<u8>) {
        (wad_path_hash(path), path.to_string(), bytes)
    }

    #[test]
    fn detects_strips_and_reclones_verified_clones() {
        let p = FileBinProvider;
        let skin0 = skin0_bytes("kayn", &p);
        let mut all_files = vec![
            entry("data/characters/kayn/skins/skin0.bin", skin0.clone()),
            entry(
                "data/characters/kayn/skins/skin1.bin",
                cloned_slot_bytes(&skin0, "kayn", 1, &p),
            ),
            entry(
                "data/characters/kayn/skins/skin2.bin",
                cloned_slot_bytes(&skin0, "kayn", 2, &p),
            ),
            entry(
                "data/characters/kayn/skins/skin7.bin",
                cloned_slot_bytes(&skin0, "kayn", 7, &p),
            ),
            entry("assets/characters/kayn/other.tex", vec![1, 2, 3]),
        ];

        let sets = detect_and_strip(&mut all_files, &p);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].champ, "kayn");
        assert_eq!(sets[0].slots.len(), 3);
        assert_eq!(all_files.len(), 2, "clone slots stripped");

        let recloned = reclone(&mut all_files, &sets, &p);
        assert_eq!(recloned, 3);
        assert_eq!(all_files.len(), 5);
        let slot7 = all_files
            .iter()
            .find(|(_, p, _)| p == "data/characters/kayn/skins/skin7.bin")
            .unwrap();
        assert_eq!(slot7.2, cloned_slot_bytes(&skin0, "kayn", 7, &p));
    }

    #[test]
    fn real_chroma_slots_are_left_alone() {
        let p = FileBinProvider;
        let skin0 = skin0_bytes("kayn", &p);
        // A genuinely different slot bin (different champ content).
        let unique = skin0_bytes("kaynalt", &p);
        let mut all_files = vec![
            entry("data/characters/kayn/skins/skin0.bin", skin0.clone()),
            entry(
                "data/characters/kayn/skins/skin1.bin",
                cloned_slot_bytes(&skin0, "kayn", 1, &p),
            ),
            entry(
                "data/characters/kayn/skins/skin2.bin",
                cloned_slot_bytes(&skin0, "kayn", 2, &p),
            ),
            entry("data/characters/kayn/skins/skin3.bin", unique.clone()),
        ];

        let sets = detect_and_strip(&mut all_files, &p);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].slots.len(), 2, "only true clones verified");
        assert!(
            all_files
                .iter()
                .any(|(_, p, _)| p == "data/characters/kayn/skins/skin3.bin"),
            "the real chroma stays in the pipeline"
        );
    }

    #[test]
    fn below_threshold_is_untouched() {
        let p = FileBinProvider;
        let skin0 = skin0_bytes("kayn", &p);
        let mut all_files = vec![
            entry("data/characters/kayn/skins/skin0.bin", skin0.clone()),
            entry(
                "data/characters/kayn/skins/skin1.bin",
                cloned_slot_bytes(&skin0, "kayn", 1, &p),
            ),
        ];
        let sets = detect_and_strip(&mut all_files, &p);
        assert!(sets.is_empty());
        assert_eq!(all_files.len(), 2);
    }

    #[test]
    fn reclone_uses_the_fixed_skin0() {
        let p = FileBinProvider;
        let skin0 = skin0_bytes("kayn", &p);
        let mut all_files = vec![
            entry("data/characters/kayn/skins/skin0.bin", skin0.clone()),
            entry(
                "data/characters/kayn/skins/skin1.bin",
                cloned_slot_bytes(&skin0, "kayn", 1, &p),
            ),
            entry(
                "data/characters/kayn/skins/skin2.bin",
                cloned_slot_bytes(&skin0, "kayn", 2, &p),
            ),
            entry(
                "data/characters/kayn/skins/skin3.bin",
                cloned_slot_bytes(&skin0, "kayn", 3, &p),
            ),
        ];

        let sets = detect_and_strip(&mut all_files, &p);
        assert_eq!(sets[0].slots.len(), 3);

        // Simulate a fix mutating skin0 (string→file style change).
        let mut tree = p.parse_bytes(&skin0).unwrap();
        let scdp = fnv("characters/kayn/skins/skin0");
        tree.objects
            .get_mut(&scdp)
            .unwrap()
            .properties
            .get_mut(&0x22)
            .unwrap()
            .value = PropertyValue::WadHash(0xdead_beef);
        let fixed = p.write_bytes(&tree).unwrap();
        all_files[0].2 = fixed.clone();

        reclone(&mut all_files, &sets, &p);
        let slot2 = all_files
            .iter()
            .find(|(_, p, _)| p == "data/characters/kayn/skins/skin2.bin")
            .unwrap();
        assert_eq!(
            slot2.2,
            cloned_slot_bytes(&fixed, "kayn", 2, &p),
            "reclone must derive from the fixed skin0"
        );
    }
}
