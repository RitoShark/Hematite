//! Loose textures the game will never load.
//!
//! A mod can ship a folder of `.dds` files that nothing points at. The client only loads
//! what a BIN references, and it wants `.tex`, so those files sit in the archive doing
//! nothing and the skin renders with its original art. The mod looks installed and
//! changes nothing, which is worse than an obvious failure: there is no error to chase.
//!
//! ## Why the whole-WAD view
//! This cannot be answered one BIN at a time. The question is what share of the files a
//! mod *ships* are unreachable, so it needs the archive's file list, every BIN's
//! references, and the game's contents together.
//!
//! ## The gate that makes it safe
//! It only runs on a mod with no skin definition at all. A mod that ships a real skin BIN
//! is wiring its own art up, and its loose files are usually intermediates: judging those
//! by this ratio would condemn working mods. Without this gate a perfectly good skin can
//! measure over 90% "unbound" and still render correctly.

use crate::traits::{GameProvider, WadProvider};
use std::collections::HashSet;

/// FNV-1a of `SkinCharacterDataProperties`: the presence of one means the mod defines a
/// skin, and this check does not apply.
pub const SKIN_CHARACTER_DATA_PROPERTIES: u32 = 0x9b67_e9f6;

/// Path fragments whose textures are interface art rather than skin art.
pub const DEFAULT_EXCLUDED_SEGMENTS: &[&str] = &[
    "/hud/",
    "/icons2d/",
    "/icons/",
    "/spells/icons",
    "/loadouts/",
    "/summonericons",
];

/// What the measurement found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LooseTextureVerdict {
    /// Textures that will not load.
    pub lost: usize,
    /// Textures counted, after excluding interface art and mipmap levels.
    pub total: usize,
    /// Subset of `lost` whose `.tex` counterpart exists, so a conversion would fix them.
    pub convertible: usize,
}

impl LooseTextureVerdict {
    /// Whole-percent share that will not load, truncated.
    pub fn percent(&self) -> u32 {
        if self.total == 0 {
            return 0;
        }
        (self.lost as f32 / self.total as f32 * 100.0) as u32
    }

    /// Player-facing summary.
    pub fn describe(&self) -> String {
        format!(
            "{}/{} ({}%) of the skin's loose textures will not display as shipped",
            self.lost,
            self.total,
            self.percent()
        )
    }
}

/// Whether a stem is a mipmap level like `4x_foo`.
///
/// Mipmaps are additional detail levels for a texture that is itself counted, so
/// including them would inflate both sides of the ratio with duplicates.
fn is_mipmap_level(stem: &str) -> bool {
    let digits = stem.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && stem[digits..].starts_with("x_")
}

fn stem_of(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file)
}

/// Measure a mod's loose textures.
///
/// `files` is every path the archive ships. `defines_skin` reports whether any BIN
/// declares a skin, which disables the check. `references` answers whether any BIN points
/// at a path.
///
/// Returns `None` when the check does not apply or the share is below `threshold`.
/// `min_textures` suppresses tiny samples, where one stray file would read as a large
/// percentage.
#[allow(clippy::too_many_arguments)]
pub fn evaluate(
    files: &[String],
    defines_skin: bool,
    references: impl Fn(&str) -> bool,
    wad: &dyn WadProvider,
    game: Option<&dyn GameProvider>,
    excluded_segments: &[String],
    min_textures: usize,
    threshold: f32,
) -> Option<LooseTextureVerdict> {
    if defines_skin {
        return None;
    }

    let normalized: Vec<String> = files
        .iter()
        .map(|p| p.to_lowercase().replace('\\', "/"))
        .collect();

    let shipped_tex: HashSet<&str> = normalized
        .iter()
        .filter(|p| p.ends_with(".tex"))
        .map(String::as_str)
        .collect();

    let excluded: Vec<String> = if excluded_segments.is_empty() {
        DEFAULT_EXCLUDED_SEGMENTS
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        excluded_segments.iter().map(|s| s.to_lowercase()).collect()
    };

    let candidates: Vec<&String> = normalized
        .iter()
        .filter(|p| p.ends_with(".dds"))
        .filter(|p| !excluded.iter().any(|seg| p.contains(seg.as_str())))
        .filter(|p| !is_mipmap_level(stem_of(p)))
        .collect();

    if candidates.len() < min_textures {
        return None;
    }

    let resolves = |path: &str| wad.has_path(path) || game.is_some_and(|g| g.has_path(path));

    let mut lost = 0usize;
    let mut convertible = 0usize;
    for dds in &candidates {
        let twin = format!("{}.tex", &dds[..dds.len() - 4]);

        // Bound, in any of three ways: the mod ships the converted form, a BIN points at
        // the file directly, or a BIN points at the converted form and that form exists.
        if shipped_tex.contains(twin.as_str()) || references(dds) || (references(&twin) && resolves(&twin))
        {
            continue;
        }

        lost += 1;
        if resolves(&twin) {
            convertible += 1;
        }
    }

    let share = lost as f32 / candidates.len() as f32;
    if share <= threshold {
        return None;
    }

    Some(LooseTextureVerdict {
        lost,
        total: candidates.len(),
        convertible,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Nothing;
    impl WadProvider for Nothing {
        fn has_path(&self, _p: &str) -> bool {
            false
        }
        fn has_hash(&self, _h: u64) -> bool {
            false
        }
    }

    fn paths(n: usize) -> Vec<String> {
        (0..n)
            .map(|i| format!("assets/characters/gragas/skins/skin1/tex{i}.dds"))
            .collect()
    }

    #[test]
    fn unreferenced_textures_are_lost() {
        let v = evaluate(&paths(10), false, |_| false, &Nothing, None, &[], 6, 0.80).unwrap();
        assert_eq!((v.lost, v.total), (10, 10));
        assert_eq!(v.percent(), 100);
    }

    /// A mod that defines its own skin is wiring its art up; its loose files are
    /// intermediates and must not be judged by this ratio.
    #[test]
    fn a_mod_defining_a_skin_is_exempt() {
        assert!(evaluate(&paths(10), true, |_| false, &Nothing, None, &[], 6, 0.80).is_none());
    }

    #[test]
    fn a_referenced_texture_is_not_lost() {
        let v = evaluate(&paths(10), false, |p| p.ends_with("tex0.dds"), &Nothing, None, &[], 6, 0.80)
            .unwrap();
        assert_eq!(v.lost, 9);
    }

    /// Shipping the converted form means the art loads, so the source file being
    /// unreferenced is irrelevant.
    #[test]
    fn shipping_the_converted_form_binds_it() {
        let mut files = paths(10);
        files.push("assets/characters/gragas/skins/skin1/tex0.tex".into());
        let v = evaluate(&files, false, |_| false, &Nothing, None, &[], 6, 0.80).unwrap();
        assert_eq!(v.lost, 9);
    }

    /// Exactly at the threshold does not fire: the bar is "more than".
    #[test]
    fn the_threshold_is_exclusive() {
        let files = paths(10);
        let bound = ["tex0.dds", "tex1.dds"];
        assert!(evaluate(
            &files,
            false,
            |p| bound.iter().any(|b| p.ends_with(b)),
            &Nothing,
            None,
            &[],
            6,
            0.80
        )
        .is_none());
    }

    /// A handful of files is not a measurement; one stray would read as a huge share.
    #[test]
    fn small_samples_are_ignored() {
        assert!(evaluate(&paths(5), false, |_| false, &Nothing, None, &[], 6, 0.80).is_none());
    }

    #[test]
    fn interface_art_is_excluded() {
        let files = vec!["assets/characters/gragas/hud/icon.dds".to_string()];
        assert!(evaluate(&files, false, |_| false, &Nothing, None, &[], 1, 0.80).is_none());
    }

    #[test]
    fn mipmap_levels_are_excluded() {
        assert!(is_mipmap_level("4x_body"));
        assert!(is_mipmap_level("16x_body"));
        assert!(!is_mipmap_level("body"));
        assert!(!is_mipmap_level("x_body"));
    }

    #[test]
    fn percent_truncates_rather_than_rounds() {
        let v = LooseTextureVerdict {
            lost: 799,
            total: 1000,
            convertible: 0,
        };
        assert_eq!(v.percent(), 79);
    }
}
