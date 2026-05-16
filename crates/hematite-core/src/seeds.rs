//! Seed discovery — pull every champion/skin pair the mod actually ships
//! out of a WAD's resolved table-of-contents.
//!
//! A "seed" is a `(champion, skin_no)` pair pointing at a root skin BIN
//! the mod intends to override. Most mods carry exactly one (the primary
//! champion's `skin0.bin`), but plenty ship subcharacter forms alongside
//! the main champ — Jinx + jinxmine, Annie + tibbers, Anivia + egg, etc.
//! — and a few package multiple skins for the same champion.
//!
//! ## How
//! Given the resolved paths the WAD ships (typically the strings emitted
//! by the hash dictionary), scan for paths matching:
//!
//! ```text
//!   (data|assets)/characters/{champion}/skins/skin{N}.bin
//! ```
//!
//! Return one [`SkinSeed`] per unique pair. Order is preserved in
//! first-seen order so the CLI's "primary" seed is whatever the WAD
//! lists first (usually the file the user named on the command line).

use std::collections::HashSet;

/// One skin a fix run should treat as a seed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SkinSeed {
    /// Lower-cased champion folder name (e.g. `"yone"`, `"jinxmine"`).
    pub champion: String,
    /// Zero-based skin number parsed from the filename.
    pub skin_no: u32,
}

impl SkinSeed {
    /// Canonical BIN path the seed refers to.
    pub fn bin_path(&self) -> String {
        format!(
            "data/characters/{}/skins/skin{}.bin",
            self.champion, self.skin_no
        )
    }
}

/// Walk a list of WAD-resolved file paths and return every unique
/// `(champion, skin)` seed discovered. The input is expected to be the
/// `path` half of `WadFile::extract_all_files`'s output — paths with no
/// dictionary hit (raw hex hashes) are silently skipped.
pub fn discover_seeds<S, I>(paths: I) -> Vec<SkinSeed>
where
    S: AsRef<str>,
    I: IntoIterator<Item = S>,
{
    let mut seen: HashSet<SkinSeed> = HashSet::new();
    let mut out = Vec::new();
    for raw in paths {
        if let Some(seed) = parse_skin_path(raw.as_ref()) {
            if seen.insert(seed.clone()) {
                out.push(seed);
            }
        }
    }
    out
}

/// Promote `primary` to the front of `seeds` if present; otherwise prepend it.
/// Used by the CLI to keep "the user-named champion is the primary seed"
/// ordering even when subchar entries sort earlier alphabetically.
pub fn order_with_primary(mut seeds: Vec<SkinSeed>, primary: &SkinSeed) -> Vec<SkinSeed> {
    if let Some(idx) = seeds.iter().position(|s| s == primary) {
        seeds.swap(0, idx);
        return seeds;
    }
    let mut prefixed = Vec::with_capacity(seeds.len() + 1);
    prefixed.push(primary.clone());
    prefixed.extend(seeds);
    prefixed
}

fn parse_skin_path(path: &str) -> Option<SkinSeed> {
    let lower = path.to_lowercase();
    let rest = lower
        .strip_prefix("data/characters/")
        .or_else(|| lower.strip_prefix("assets/characters/"))?;
    // rest = "{champion}/skins/skin{N}.bin"
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() != 3 || parts[1] != "skins" {
        return None;
    }
    let file = parts[2];
    if !file.starts_with("skin") || !file.ends_with(".bin") {
        return None;
    }
    let middle = &file[4..file.len() - 4];
    let skin_no: u32 = middle.parse().ok()?;
    if parts[0].is_empty() {
        return None;
    }
    Some(SkinSeed {
        champion: parts[0].to_string(),
        skin_no,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_skin_path() {
        let seed = parse_skin_path("data/characters/yone/skins/skin0.bin").unwrap();
        assert_eq!(seed.champion, "yone");
        assert_eq!(seed.skin_no, 0);
    }

    #[test]
    fn parses_assets_root() {
        let seed = parse_skin_path("assets/characters/jinx/skins/skin27.bin").unwrap();
        assert_eq!(seed.champion, "jinx");
        assert_eq!(seed.skin_no, 27);
    }

    #[test]
    fn rejects_non_skin_paths() {
        assert!(parse_skin_path("data/characters/yone/yone.bin").is_none());
        assert!(parse_skin_path("data/characters/yone/skins/root.bin").is_none());
        assert!(parse_skin_path("data/characters/yone/animations/skin0.bin").is_none());
        assert!(parse_skin_path("data/shared/foo.bin").is_none());
        assert!(parse_skin_path("garbage").is_none());
    }

    #[test]
    fn discover_deduplicates() {
        let paths = vec![
            "data/characters/yone/skins/skin0.bin",
            "DATA/Characters/Yone/Skins/Skin0.bin", // duplicate via case
            "data/characters/jinxmine/skins/skin0.bin",
            "data/characters/yone/skins/skin5.bin",
            "data/characters/yone/yone.bin", // not a skin
        ];
        let seeds = discover_seeds(paths.iter().copied());
        assert_eq!(seeds.len(), 3);
        // First-seen order: yone/0, jinxmine/0, yone/5.
        assert_eq!(seeds[0].champion, "yone");
        assert_eq!(seeds[0].skin_no, 0);
        assert_eq!(seeds[1].champion, "jinxmine");
        assert_eq!(seeds[2].skin_no, 5);
    }

    #[test]
    fn bin_path_round_trips() {
        let seed = SkinSeed {
            champion: "yone".into(),
            skin_no: 0,
        };
        assert_eq!(seed.bin_path(), "data/characters/yone/skins/skin0.bin");
    }

    #[test]
    fn order_with_primary_promotes_existing() {
        let primary = SkinSeed {
            champion: "yone".into(),
            skin_no: 0,
        };
        let seeds = vec![
            SkinSeed {
                champion: "jinxmine".into(),
                skin_no: 0,
            },
            primary.clone(),
            SkinSeed {
                champion: "yonebot".into(),
                skin_no: 0,
            },
        ];
        let ordered = order_with_primary(seeds, &primary);
        assert_eq!(ordered[0], primary);
        assert_eq!(ordered.len(), 3);
    }

    #[test]
    fn order_with_primary_prepends_missing() {
        let primary = SkinSeed {
            champion: "yone".into(),
            skin_no: 0,
        };
        let seeds = vec![SkinSeed {
            champion: "jinxmine".into(),
            skin_no: 0,
        }];
        let ordered = order_with_primary(seeds, &primary);
        assert_eq!(ordered[0], primary);
        assert_eq!(ordered.len(), 2);
    }
}
