use std::collections::{HashMap, HashSet};

use hematite_types::champion::CharacterRelations;
use hematite_types::hash::GameHash;
use hematite_types::repath::RepathReport;

use crate::repath::{
    collect_bin_asset_hashes, collect_bin_asset_paths, is_soundbank, looks_like_bin,
};
use crate::strings::normalize_wad_path;
use crate::traits::{BinProvider, HashProvider};

const RIOT_CATEGORIES: &[&str] = &[
    "characters",
    "maps",
    "perks",
    "items",
    "loadouts",
    "ux",
    "shared",
    "particles",
    "sounds",
    "materials",
    "environments",
    "spells",
    "hud",
    "shaders",
    "lib",
    "menu",
];
const ASSET_EXTS: &[&str] = &[
    ".dds", ".tex", ".skn", ".skl", ".anm", ".wem", ".scb", ".sco", ".scn", ".troybin", ".luaobj",
    ".lua", ".dat", ".png", ".jpg", ".webp", ".mapgeo",
];

#[derive(Debug, PartialEq, Eq)]
enum PathClass {
    Canonical,
    Prefixed,
    Skip,
}

fn classify(path: &str, champions: &CharacterRelations) -> PathClass {
    if is_soundbank(path)
        || path.contains("/wwise2016/vo/")
        || !ASSET_EXTS.iter().any(|ext| path.ends_with(ext))
    {
        return PathClass::Skip;
    }
    let Some(rest) = path
        .strip_prefix("assets/")
        .or_else(|| path.strip_prefix("data/"))
    else {
        return PathClass::Skip;
    };
    let mut segments = rest.split('/');
    match segments.next() {
        Some("") | None => PathClass::Skip,
        Some("characters") => match segments.next() {
            Some(name)
                if !name.is_empty()
                    && (champions.champions.is_empty() || champions.is_riot_character(name)) =>
            {
                PathClass::Canonical
            }
            _ => PathClass::Skip,
        },
        Some(category) if RIOT_CATEGORIES.contains(&category) => PathClass::Canonical,
        Some(_) => PathClass::Prefixed,
    }
}

pub fn check_repath(
    files: &[(u64, String, Vec<u8>)],
    bins: &dyn BinProvider,
    hashes: &dyn HashProvider,
    champions: &CharacterRelations,
) -> RepathReport {
    let shipped: HashSet<u64> = files.iter().map(|(hash, _, _)| *hash).collect();
    let mut names: HashMap<u64, String> = files
        .iter()
        .filter_map(|(hash, path, _)| {
            let path = normalize_wad_path(path);
            (path_hash(&path) == *hash).then_some((*hash, path))
        })
        .collect();
    let mut report = RepathReport::default();
    let mut references = Vec::new();
    for (_, path, bytes) in files {
        if !path.to_lowercase().ends_with(".bin") && !looks_like_bin(bytes) {
            continue;
        }
        let mut tree = match bins.parse_bytes(bytes) {
            Ok(tree) => tree,
            Err(_) => {
                report.bins_failed += 1;
                continue;
            }
        };
        report.bins_scanned += 1;
        tree.objects
            .retain(|_, object| object.class_hash.0 != crate::strings::fnv1a_hash("RitoBinMap"));
        let paths: HashSet<String> = collect_bin_asset_paths(&tree, true).into_iter().collect();
        for path in &paths {
            names.insert(path_hash(path), path.clone());
        }
        for (hash, path) in &tree.recorded_files {
            let path = normalize_wad_path(path);
            if path_hash(&path) == *hash {
                names.insert(*hash, path);
            }
        }
        references.push((paths, collect_bin_asset_hashes(&tree)));
    }
    for (mut paths, file_hashes) in references {
        for hash in file_hashes
            .into_iter()
            .filter(|h| *h != 0)
            .collect::<HashSet<_>>()
        {
            if let Some(path) = names
                .get(&hash)
                .map(String::as_str)
                .or_else(|| hashes.resolve_game_path(GameHash(hash)))
            {
                paths.insert(normalize_wad_path(path));
            } else {
                report.unresolved_hashes += 1;
            }
        }
        for path in paths {
            match classify(&path, champions) {
                PathClass::Canonical if shipped.contains(&path_hash(&path)) => {
                    report.canonical += 1
                }
                PathClass::Prefixed => report.prefixed += 1,
                _ => {}
            }
        }
    }
    report.finish();
    report
}

fn path_hash(path: &str) -> u64 {
    xxhash_rust::xxh64::xxh64(path.as_bytes(), 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::repath::RepathStatus;

    #[test]
    fn categories_and_exclusions_match_repathable_references() {
        let mut champions = CharacterRelations::default();
        champions.champions.insert("azir".into());
        champions
            .subchamp_to_champion
            .insert("tibbers".into(), "annie".into());
        for path in [
            "assets/characters/azir/a.tex",
            "assets/characters/azirsoldier/a.skn",
            "assets/characters/tibbers/a.tex",
            "data/maps/a.scb",
        ] {
            assert_eq!(classify(path, &champions), PathClass::Canonical);
        }
        for path in [
            "assets/custom/characters/azir/a.tex",
            "assets/.azir0_characters/azir/a.tex",
        ] {
            assert_eq!(classify(path, &champions), PathClass::Prefixed);
        }
        for path in [
            "assets/characters/lebron/a.tex",
            "assets/sounds/a.bnk",
            "data/characters/azir/skin0.bin",
            "assets/sounds/wwise2016/vo/a.wem",
            "mod/a.tex",
        ] {
            assert_eq!(classify(path, &champions), PathClass::Skip);
        }
    }

    #[test]
    fn verdicts_preserve_thresholds_without_hiding_remaining_work() {
        for (canonical, prefixed, expected) in [
            (0, 0, RepathStatus::Unknown),
            (1, 0, RepathStatus::NotRepathed),
            (4, 6, RepathStatus::HalflyRepathed),
            (1, 99, RepathStatus::Repathed),
        ] {
            let mut report = RepathReport {
                canonical,
                prefixed,
                bins_scanned: 1,
                ..Default::default()
            };
            report.finish();
            assert_eq!(report.status, expected);
            assert_eq!(report.needs_repath, canonical > 0);
        }
    }
}
