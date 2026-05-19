//! Mod asset repathing — Topaz `FixPath` semantics, with extras for the
//! cases Topaz misses (binless mods, custom-hashed WAD entries).
//!
//! ## What runs
//! 1. [`repath_path`] — the canonical path transform. Topaz-faithful.
//! 2. [`repath_bin_strings`] — walk every string in a BIN and rewrite the
//!    ones that point at files actually present in the mod (or extension
//!    alternates: `.dds` ↔ `.tex`, `.sco` ↔ `.scb`). Returns the new paths
//!    so the WAD step can re-hash entries to match.
//! 3. [`repath_wad_path`] — compute the new WAD path for any non-root file.
//!    Unlike older versions of this module, **linked BINs are repathed
//!    too** — the only BIN the engine resolves by hard-coded path is the
//!    "root" character / skin BIN (see [`is_root_skin_bin`]).
//! 4. [`missing_invis_placeholders`] — opt-in injection of invisible
//!    placeholder textures for repathed texture refs that have no real file.
//!
//! ## Path layout
//! Topaz's `FixPath_local` is the reference. With the default
//! [`RepathLayout::InFolder`] the prefix is concatenated to the **second**
//! segment with no slash, and the root segment is upper-cased:
//!
//! ```text
//!   data/characters/yone/skins/skin0.bin
//! → DATA/.yone1_characters/yone/skins/skin0.bin
//! ```
//!
//! The alternative [`RepathLayout::Nested`] (LtMAO bumpath) inserts the
//! prefix as its own folder and keeps the original casing:
//!
//! ```text
//!   data/characters/yone/skins/skin0.bin
//! → data/.yone1_/characters/yone/skins/skin0.bin
//! ```

use crate::walk::{walk_tree, PropertyVisitor, VisitResult};
use hematite_types::bin::BinTree;
use hematite_types::hash::FieldHash;
use hematite_types::repath::{RepathLayout, RepathOptions};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Embedded placeholder texture
// ---------------------------------------------------------------------------

/// Bytes of an invisible 1×1 TEX texture used as a placeholder.
pub const INVIS_TEX: &[u8] = include_bytes!("assets/invis.tex");

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Roots that mark a path as a League asset reference. Anything else is
/// left alone.
const ASSET_ROOTS: &[&str] = &["assets/", "data/"];

/// VO path fragment — never repath voice-over audio.
const VO_PATH: &str = "/wwise2016/vo/";

/// Texture extensions that trigger invisible placeholder injection. Both
/// `.dds` and `.tex` references are placeholded as `.tex` since that is
/// the format the engine actually loads.
const TEXTURE_EXTS: &[&str] = &["dds", "tex"];

/// Asset extensions used to recognise "modder-root" paths that don't carry
/// the canonical `assets/` / `data/` prefix — e.g. `reddivinekinggaren/foo.dds`,
/// where the modder uses their handle as a namespace. Only files ending with
/// one of these extensions count as asset references and get the prefix
/// prepended as a new top segment (mirrors Celestial's `ROOT_ASSET_EXTS`).
const MODDER_ROOT_EXTS: &[&str] = &[
    ".dds", ".tex", ".skn", ".skl", ".anm", ".bin", ".bnk", ".wpk", ".wem", ".scb", ".sco", ".scn",
    ".troybin", ".luaobj", ".lua", ".dat", ".png", ".jpg", ".webp", ".mapgeo",
];

/// `true` if a lowercased path looks like a modder-root reference: contains
/// a slash and ends with a known asset extension, but lacks the canonical
/// `assets/` / `data/` prefix. Kept tight on purpose — arbitrary string
/// properties (names, hashes, plain text) must not be misclassified as
/// asset paths.
fn is_modder_root_path(lower: &str) -> bool {
    if !lower.contains('/') {
        return false;
    }
    if ASSET_ROOTS.iter().any(|r| lower.starts_with(r)) {
        return false;
    }
    MODDER_ROOT_EXTS.iter().any(|ext| lower.ends_with(ext))
}

/// Magic numbers that identify League BIN files (PROP/PTCH headers).
const BIN_MAGIC_PROP: &[u8; 4] = b"PROP";
const BIN_MAGIC_PTCH: &[u8; 4] = b"PTCH";

// ---------------------------------------------------------------------------
// Path layout helpers
// ---------------------------------------------------------------------------

/// Returns `(root_lowercased, segments_after_root)` if `path` starts with
/// an asset root, else `None`.
fn split_asset_path(path: &str) -> Option<(&'static str, Vec<&str>)> {
    let lower_head: String = path.chars().take(8).collect::<String>().to_lowercase();
    let stripped = if lower_head.starts_with("assets/") {
        ("assets", &path[7..])
    } else if lower_head.starts_with("data/") {
        ("data", &path[5..])
    } else {
        return None;
    };
    let segs: Vec<&str> = stripped.1.split('/').filter(|s| !s.is_empty()).collect();
    Some((stripped.0, segs))
}

/// Apply the canonical repath transform to `path`.
///
/// Pure; identical inputs produce identical outputs. Three shapes:
///
/// * **Canonical Riot path** (starts with `assets/` or `data/`) — layout-aware
///   prefix insertion (see [`RepathLayout`]).
/// * **Modder-root path** (`reddivinekinggaren/foo.dds` style — no canonical
///   root, but contains a slash and ends in a known asset extension) — the
///   prefix is prepended as a new top-level segment so the modder's
///   namespace stays intact underneath. Same transform in both layouts.
/// * **Anything else** — returned unchanged.
pub fn repath_path(path: &str, prefix: &str, layout: RepathLayout) -> String {
    let normalized = path.replace('\\', "/");
    if let Some((root, segs)) = split_asset_path(&normalized) {
        if segs.is_empty() {
            // Bare root like "assets/" — nothing to do.
            return normalized;
        }

        return match layout {
            RepathLayout::InFolder => {
                // Topaz: ROOT/{prefix}{segs[0]}/{segs[1..]}
                let root_up = match root {
                    "assets" => "ASSETS",
                    "data" => "DATA",
                    _ => root,
                };
                let head = format!("{}{}", prefix, segs[0]);
                if segs.len() == 1 {
                    format!("{}/{}", root_up, head)
                } else {
                    format!("{}/{}/{}", root_up, head, segs[1..].join("/"))
                }
            }
            RepathLayout::Nested => {
                // LtMAO: root/{prefix}/{segs[..]}
                format!("{}/{}/{}", root, prefix, segs.join("/"))
            }
        };
    }

    // Modder-root path: prepend prefix as new top segment.
    // Layout-agnostic — there's no canonical root to nest under.
    if is_modder_root_path(&normalized.to_lowercase()) {
        return format!("{}/{}", prefix, normalized);
    }

    normalized
}

/// Inverse of [`repath_path`]: given an already-repathed string, recover
/// the original (un-prefixed) form. Idempotent on inputs that don't carry
/// the prefix. Both layouts and the modder-root shape are handled:
///
/// * **Topaz InFolder** `ASSETS/{prefix}characters/...` →
///   `assets/characters/...` (root lower-cased, prefix-prefix stripped from
///   the head segment).
/// * **Nested** `assets/{prefix}/characters/...` → `assets/characters/...`.
/// * **Modder-root** `{prefix}/<rest>` → `<rest>`.
///
/// Used by the game-WAD fallback resolver: given a repathed string ref,
/// figure out the original path so we can look the bytes up in the live
/// game WAD.
pub fn remove_prefix(path: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        return path.to_string();
    }

    // Try canonical layouts first (case-insensitive on the root segment).
    if let Some(slash) = path.find('/') {
        let (root, rest) = path.split_at(slash);
        let rest = &rest[1..]; // drop the slash
        let root_lower = root.to_lowercase();
        if root_lower == "assets" || root_lower == "data" {
            // Nested:  prefix is its own segment immediately after the root.
            if let Some(after) = rest.strip_prefix(&format!("{}/", prefix)) {
                return format!("{}/{}", root_lower, after);
            }
            // InFolder: prefix is concatenated to the head of the next segment.
            if let Some(after) = rest.strip_prefix(prefix) {
                return format!("{}/{}", root_lower, after);
            }
        }
    }

    // Modder-root: prefix is the leading segment.
    if let Some(stripped) = path.strip_prefix(&format!("{}/", prefix)) {
        return stripped.to_string();
    }

    path.to_string()
}

/// Returns `true` if this string value is an asset path that *could* be
/// repathed (modulo VO skip and existence checks).
fn is_repath_candidate(value: &str, skip_vo: bool) -> bool {
    let lower = value.to_lowercase().replace('\\', "/");
    let canonical = ASSET_ROOTS.iter().any(|p| lower.starts_with(p));
    let modder_root = !canonical && is_modder_root_path(&lower);
    if !canonical && !modder_root {
        return false;
    }
    if skip_vo && lower.contains(VO_PATH) {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Existence checks
// ---------------------------------------------------------------------------

/// Lowercased-path lookup keyed by both the textual path and its
/// `xxhash64`. Used to answer "does this asset string actually point at a
/// file the mod ships?", including the case where the mod's WAD entry
/// has no resolved path (custom-hashed mods).
#[derive(Default)]
pub struct WadIndex {
    /// Lowercased file paths that the WAD ships.
    pub paths: HashSet<String>,
    /// All path-hashes present in the WAD, including hex-only (unresolved) entries.
    pub hashes: HashSet<u64>,
}

impl WadIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from `(hash, path)` pairs (typically from `extract_all_files`).
    pub fn from_entries<I: IntoIterator<Item = (u64, String)>>(it: I) -> Self {
        let mut idx = Self::new();
        for (h, p) in it {
            idx.hashes.insert(h);
            idx.paths.insert(p.to_lowercase().replace('\\', "/"));
        }
        idx
    }

    /// Does this asset path (or an ext-alternate) exist in the WAD?
    ///
    /// Checks `.dds` ↔ `.tex` and `.sco` ↔ `.scb` swaps so that a BIN
    /// string `"foo.dds"` still matches when the mod ships `"foo.tex"`,
    /// and vice-versa. Also matches by `xxhash64(lowercased)` so custom-
    /// hashed entries (no resolved path) are still recognised.
    pub fn has(&self, path: &str, hash_fn: impl Fn(&str) -> u64) -> bool {
        self.get_actual_path(path, hash_fn).is_some()
    }

    /// Returns the path actually present in the WAD (resolving alternates).
    pub fn get_actual_path(&self, path: &str, hash_fn: impl Fn(&str) -> u64) -> Option<String> {
        let lower = path.to_lowercase().replace('\\', "/");
        if self.paths.contains(&lower) {
            return Some(lower);
        }
        if self.hashes.contains(&hash_fn(&lower)) {
            return Some(lower);
        }
        // Extension alternates.
        let alt: Option<String> = if let Some(stem) = lower.strip_suffix(".dds") {
            Some(format!("{}.tex", stem))
        } else if let Some(stem) = lower.strip_suffix(".tex") {
            Some(format!("{}.dds", stem))
        } else if let Some(stem) = lower.strip_suffix(".sco") {
            Some(format!("{}.scb", stem))
        } else {
            lower
                .strip_suffix(".scb")
                .map(|stem| format!("{}.sco", stem))
        };
        if let Some(ref a) = alt {
            if self.paths.contains(a) {
                return Some(a.clone());
            }
            if self.hashes.contains(&hash_fn(a)) {
                return Some(a.clone());
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Root-BIN detection (these must NOT be moved)
// ---------------------------------------------------------------------------

/// Returns `true` for character / skin BINs that the engine resolves at a
/// hard-coded path. Repathing these would make the game stop loading the
/// mod entirely.
///
/// Conservative pattern: matches the standard skin definition file and the
/// champion-root data file. Anything else (linked / animation BINs) is
/// fair game for repathing.
pub fn is_root_skin_bin(path: &str) -> bool {
    let lower = path.to_lowercase().replace('\\', "/");
    if !lower.ends_with(".bin") {
        return false;
    }
    // data/characters/{champ}/{champ}.bin
    if let Some(rest) = lower.strip_prefix("data/characters/") {
        let parts: Vec<&str> = rest.trim_end_matches(".bin").split('/').collect();
        if parts.len() == 2 && parts[0] == parts[1] {
            return true;
        }
        // data/characters/{champ}/skins/skinNN.bin (root skin)
        if parts.len() == 3 && parts[1] == "skins" && parts[2].starts_with("skin") {
            return true;
        }
        // data/characters/{champ}/skins/root.bin (rare but exists)
        if parts.len() == 3 && parts[1] == "skins" && parts[2] == "root" {
            return true;
        }
    }
    false
}

/// Detects a League BIN file purely by content magic. Used so we can repath
/// strings inside BINs whose path hashes weren't in the dictionary.
pub fn looks_like_bin(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && (&bytes[..4] == BIN_MAGIC_PROP || &bytes[..4] == BIN_MAGIC_PTCH)
}

// ---------------------------------------------------------------------------
// BIN asset path collection (read-only)
// ---------------------------------------------------------------------------

/// Collect every asset-path string referenced inside a BIN tree. Used by
/// the `--game-wad` flow to figure out which game files need to be pulled.
pub fn collect_bin_asset_paths(tree: &BinTree, skip_vo: bool) -> Vec<String> {
    struct Collector {
        skip_vo: bool,
        paths: Vec<String>,
    }
    impl PropertyVisitor for Collector {
        fn visit_string(&mut self, value: &str, _f: FieldHash) -> VisitResult {
            if is_repath_candidate(value, self.skip_vo) {
                self.paths.push(value.to_lowercase().replace('\\', "/"));
            }
            VisitResult::Skip
        }
    }
    let mut v = Collector {
        skip_vo,
        paths: Vec::new(),
    };
    let mut clone = tree.clone();
    walk_tree(&mut clone, &mut v);
    // Also surface linked-BIN paths as references — the consumer usually
    // wants those too.
    for link in &tree.linked {
        if is_repath_candidate(link, skip_vo) {
            v.paths.push(link.to_lowercase().replace('\\', "/"));
        }
    }
    v.paths
}

// ---------------------------------------------------------------------------
// BIN repathing
// ---------------------------------------------------------------------------

/// Outcome of [`repath_bin_strings`].
pub struct RepathBinResult {
    /// Number of strings rewritten (asset strings + linked deps).
    pub strings_repathed: u32,
    /// Map of `original_lower` → `new` for every rewritten path. Used so
    /// the WAD step can rename / re-hash entries by their original path.
    pub mapping: HashMap<String, String>,
}

/// Rewrite asset references inside a single BIN tree.
///
/// * Walks string properties via [`walk_tree`] and the dedicated `tree.linked`
///   list (which the walker doesn't traverse).
/// * Repaths only strings that point at files actually present in `index`
///   (or an extension alternate, or a custom hash). Strings referencing
///   base-game-only assets are left untouched: repathing them would break
///   the reference because the new path would have no file.
/// * Records the original→new mapping so the caller can rename WAD entries.
pub fn repath_bin_strings(
    tree: &mut BinTree,
    opts: &RepathOptions,
    index: &WadIndex,
    hash_fn: impl Fn(&str) -> u64 + Copy,
) -> RepathBinResult {
    struct Repather<'a, F> {
        prefix: &'a str,
        layout: RepathLayout,
        skip_vo: bool,
        index: &'a WadIndex,
        hash_fn: F,
        mapping: HashMap<String, String>,
    }

    impl<'a, F> PropertyVisitor for Repather<'a, F>
    where
        F: Fn(&str) -> u64 + Copy,
    {
        fn visit_string(&mut self, value: &str, _f: FieldHash) -> VisitResult {
            if !is_repath_candidate(value, self.skip_vo) {
                return VisitResult::Skip;
            }
            // Resolve the actual path that exists in the mod's WAD index.
            let actual_path = match self.index.get_actual_path(value, self.hash_fn) {
                Some(p) => p,
                None => return VisitResult::Skip,
            };

            let mut new_path = repath_path(value, self.prefix, self.layout);

            // Align extensions if the reference differs from the actual file in WAD
            let lower_val = value.to_lowercase().replace('\\', "/");
            if lower_val != actual_path {
                if let Some(ext) = actual_path.split('.').last() {
                    if let Some(dot) = new_path.rfind('.') {
                        new_path = format!("{}.{}", &new_path[..dot], ext);
                    }
                }
            }

            if new_path == value {
                return VisitResult::Skip;
            }

            // Map both the original reference and the actual path to the new repathed path
            self.mapping
                .entry(lower_val)
                .or_insert_with(|| new_path.clone());
            self.mapping
                .entry(actual_path)
                .or_insert_with(|| new_path.clone());

            VisitResult::Mutate(new_path)
        }
    }

    let mut visitor = Repather {
        prefix: &opts.prefix,
        layout: opts.layout,
        skip_vo: opts.skip_vo,
        index,
        hash_fn,
        mapping: HashMap::new(),
    };

    let mut count = walk_tree(tree, &mut visitor);

    // Repath linked dependencies too (the walker doesn't visit them).
    // Same rule: only repath when present in mod's WAD.
    for link in tree.linked.iter_mut() {
        if !is_repath_candidate(link, opts.skip_vo) {
            continue;
        }
        // Root skin/champion BINs are loaded by the engine at fixed paths.
        if is_root_skin_bin(link) {
            continue;
        }
        let actual_path = match index.get_actual_path(link, hash_fn) {
            Some(p) => p,
            None => continue,
        };

        let mut new_path = repath_path(link, &opts.prefix, opts.layout);

        // Align extensions if the reference differs from the actual file in WAD
        let lower_link = link.to_lowercase().replace('\\', "/");
        if lower_link != actual_path {
            if let Some(ext) = actual_path.split('.').last() {
                if let Some(dot) = new_path.rfind('.') {
                    new_path = format!("{}.{}", &new_path[..dot], ext);
                }
            }
        }

        if new_path == *link {
            continue;
        }

        // Map both the original reference and the actual path to the new repathed path
        visitor
            .mapping
            .entry(lower_link)
            .or_insert_with(|| new_path.clone());
        visitor
            .mapping
            .entry(actual_path)
            .or_insert_with(|| new_path.clone());

        *link = new_path;
        count += 1;
    }

    RepathBinResult {
        strings_repathed: count,
        mapping: visitor.mapping,
    }
}

// ---------------------------------------------------------------------------
// WAD entry repathing
// ---------------------------------------------------------------------------

/// Compute the new WAD path for an entry, or `None` if the entry should
/// keep its original path.
///
/// **Root skin BINs are NEVER repathed** — the engine references them by
/// hard-coded path. Linked / animation / data BINs *are* repathed so the
/// repathed string references inside other BINs still resolve.
/// Modder-root entries (`<handle>/file.ext`) are repathed too — their
/// prefix gets prepended as a new top segment so the entry hash matches
/// what `repath_path` produces for BIN string refs to that file.
pub fn repath_wad_path(path: &str, prefix: &str, layout: RepathLayout) -> Option<String> {
    let lower = path.to_lowercase().replace('\\', "/");
    let canonical = ASSET_ROOTS.iter().any(|p| lower.starts_with(p));
    let modder_root = !canonical && is_modder_root_path(&lower);
    if !canonical && !modder_root {
        // Custom hex hashes, plain VO entries, etc. — leave alone.
        return None;
    }
    // Root skin / champion BINs are loaded by the engine at fixed paths.
    if canonical && is_root_skin_bin(&lower) {
        return None;
    }
    let new = repath_path(path, prefix, layout);
    if new == path {
        None
    } else {
        Some(new)
    }
}

// ---------------------------------------------------------------------------
// Invisible placeholder injection
// ---------------------------------------------------------------------------

/// Build a list of `(path, bytes)` placeholder entries for every texture
/// path referenced by BIN files but absent from the WAD after repathing.
///
/// Both `.dds` and `.tex` references are normalised to `.tex` (the native
/// League format) and filled with [`INVIS_TEX`].
pub fn missing_invis_placeholders(
    existing_paths: &HashSet<String>,
    referenced_paths: &[String],
) -> Vec<(String, Vec<u8>)> {
    let mut result = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for raw in referenced_paths {
        let path = raw.to_lowercase();
        let is_tex = TEXTURE_EXTS
            .iter()
            .any(|ext| path.ends_with(&format!(".{}", ext)));
        if !is_tex {
            continue;
        }
        let tex_path = if let Some(stem) = path.strip_suffix(".dds") {
            format!("{}.tex", stem)
        } else {
            path
        };
        if !seen.insert(tex_path.clone()) {
            continue;
        }
        if existing_paths.contains(&tex_path) {
            continue;
        }
        result.push((tex_path, INVIS_TEX.to_vec()));
    }
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::bin::{BinObject, BinProperty, BinTree, PropertyValue};
    use hematite_types::hash::{PathHash, TypeHash};
    use indexmap::IndexMap;

    fn dummy_hash(s: &str) -> u64 {
        // Deterministic, not the real xxhash64 — fine for tests because
        // we use the same fn on both sides of the index check.
        s.bytes()
            .fold(0u64, |a, b| a.wrapping_mul(131).wrapping_add(b as u64))
    }

    fn opts(prefix: &str, layout: RepathLayout) -> RepathOptions {
        let mut o = RepathOptions::new(prefix);
        o.layout = layout;
        o
    }

    // -- repath_path: Topaz layout --------------------------------------

    #[test]
    fn topaz_layout_assets() {
        assert_eq!(
            repath_path(
                "assets/characters/yone/skins/skin0/yone_base.skn",
                ".yone1_",
                RepathLayout::InFolder
            ),
            "ASSETS/.yone1_characters/yone/skins/skin0/yone_base.skn"
        );
    }

    #[test]
    fn topaz_layout_data() {
        assert_eq!(
            repath_path(
                "data/characters/yone/animations/skin0.bin",
                ".yone1_",
                RepathLayout::InFolder
            ),
            "DATA/.yone1_characters/yone/animations/skin0.bin"
        );
    }

    #[test]
    fn topaz_layout_short_path() {
        // Path with only the root + one segment.
        assert_eq!(
            repath_path(
                "assets/spells/yone_q.bin",
                ".yone1_",
                RepathLayout::InFolder
            ),
            "ASSETS/.yone1_spells/yone_q.bin"
        );
    }

    #[test]
    fn topaz_layout_root_only() {
        // Just "assets/" — nothing meaningful to do.
        assert_eq!(
            repath_path("assets/", ".yone1_", RepathLayout::InFolder),
            "assets/"
        );
    }

    #[test]
    fn topaz_layout_case_insensitive_root() {
        assert_eq!(
            repath_path(
                "ASSETS/Characters/Yone/skin.dds",
                ".yone1_",
                RepathLayout::InFolder
            ),
            "ASSETS/.yone1_Characters/Yone/skin.dds"
        );
    }

    #[test]
    fn non_asset_path_passes_through() {
        // Non-asset extension + no canonical root → passthrough.
        assert_eq!(
            repath_path(
                "sounds/effects/explosion.bnk",
                ".x_",
                RepathLayout::InFolder
            ),
            // .bnk is in MODDER_ROOT_EXTS so this actually *does* get
            // treated as a modder-root path. That's the documented behavior:
            // any string ending in a known asset extension with a slash is
            // a candidate.
            ".x_/sounds/effects/explosion.bnk"
        );
        // Plain string with no slash → passthrough.
        assert_eq!(
            repath_path("just_a_name", ".x_", RepathLayout::InFolder),
            "just_a_name"
        );
        // Slash but non-asset extension → passthrough.
        assert_eq!(
            repath_path("some/random/path.txt", ".x_", RepathLayout::InFolder),
            "some/random/path.txt"
        );
    }

    // -- repath_path: modder-root layout --------------------------------

    #[test]
    fn modder_root_path_gets_prefix_prepended() {
        // Reddivinekinggaren-style modder handle with no canonical root.
        // Layout shouldn't matter — modder-root paths use a single shape.
        assert_eq!(
            repath_path("reddivinekinggaren/foo.dds", "bum", RepathLayout::InFolder),
            "bum/reddivinekinggaren/foo.dds"
        );
        assert_eq!(
            repath_path("reddivinekinggaren/foo.dds", "bum", RepathLayout::Nested),
            "bum/reddivinekinggaren/foo.dds"
        );
        // Custom extension like .scb also recognised.
        assert_eq!(
            repath_path("modder/effects/sparkle.scb", "bum", RepathLayout::InFolder),
            "bum/modder/effects/sparkle.scb"
        );
    }

    // -- repath_path: nested layout -------------------------------------

    #[test]
    fn nested_layout_assets() {
        assert_eq!(
            repath_path(
                "assets/characters/yone/skin.dds",
                "yone1",
                RepathLayout::Nested
            ),
            "assets/yone1/characters/yone/skin.dds"
        );
    }

    #[test]
    fn repath_path_normalizes_backslashes() {
        assert_eq!(
            repath_path(
                "assets\\characters\\yone\\skin.dds",
                "yone1",
                RepathLayout::Nested
            ),
            "assets/yone1/characters/yone/skin.dds"
        );
        assert_eq!(
            repath_path(
                "assets\\characters\\yone\\skins\\skin0\\yone_base.skn",
                ".yone1_",
                RepathLayout::InFolder
            ),
            "ASSETS/.yone1_characters/yone/skins/skin0/yone_base.skn"
        );
    }

    // -- root skin detection --------------------------------------------

    #[test]
    fn root_skin_detection() {
        assert!(is_root_skin_bin("data/characters/yone/yone.bin"));
        assert!(is_root_skin_bin("DATA/Characters/Yone/Yone.bin"));
        assert!(is_root_skin_bin("data/characters/yone/skins/skin0.bin"));
        assert!(is_root_skin_bin("data/characters/yone/skins/skin27.bin"));
        assert!(is_root_skin_bin("data/characters/yone/skins/root.bin"));

        assert!(!is_root_skin_bin(
            "data/characters/yone/animations/skin0.bin"
        ));
        assert!(!is_root_skin_bin(
            "data/characters/yone/skins/skin0/effects.bin"
        ));
        assert!(!is_root_skin_bin("data/shared/foo.bin"));
        assert!(!is_root_skin_bin("assets/characters/yone/yone.bin"));
        assert!(!is_root_skin_bin("data/characters/yone/yoneother.bin"));
    }

    #[test]
    fn repath_wad_path_skips_root_skin() {
        assert!(repath_wad_path(
            "data/characters/yone/skins/skin0.bin",
            ".yone1_",
            RepathLayout::InFolder
        )
        .is_none());
    }

    #[test]
    fn repath_wad_path_repaths_linked_bin() {
        // Animation BINs (linked from the root skin BIN) MUST be repathed
        // because the string ref inside the root BIN is repathed.
        assert_eq!(
            repath_wad_path(
                "data/characters/yone/animations/skin0.bin",
                ".yone1_",
                RepathLayout::InFolder
            ),
            Some("DATA/.yone1_characters/yone/animations/skin0.bin".to_string())
        );
    }

    // -- BIN content repathing -----------------------------------------

    fn make_tree_with_string(s: &str) -> BinTree {
        let mut tree = BinTree::default();
        let mut obj = BinObject {
            class_hash: TypeHash(0x1234),
            path_hash: PathHash(0x5678),
            properties: IndexMap::new(),
        };
        obj.properties.insert(
            0x1,
            BinProperty {
                name_hash: FieldHash(0x1),
                value: PropertyValue::String(s.to_string()),
            },
        );
        tree.objects.insert(0x5678, obj);
        tree
    }

    #[test]
    fn bin_string_repathed_when_file_present() {
        let mut tree = make_tree_with_string("assets/characters/yone/skins/skin0/yone.dds");
        let idx = WadIndex::from_entries(vec![(
            0,
            "assets/characters/yone/skins/skin0/yone.dds".to_string(),
        )]);

        let r = repath_bin_strings(
            &mut tree,
            &opts(".yone1_", RepathLayout::InFolder),
            &idx,
            dummy_hash,
        );
        assert_eq!(r.strings_repathed, 1);
        assert_eq!(r.mapping.len(), 1);
        let new = r
            .mapping
            .get("assets/characters/yone/skins/skin0/yone.dds")
            .unwrap();
        assert_eq!(new, "ASSETS/.yone1_characters/yone/skins/skin0/yone.dds");
    }

    #[test]
    fn bin_string_skipped_when_file_absent_from_mod() {
        // Now that selective repathing is restored, asset strings are only repathed
        // if they are present in the mod's WAD index.
        let mut tree = make_tree_with_string("assets/characters/yone/base/yone.skn");
        let idx = WadIndex::from_entries(vec![(
            0,
            "assets/characters/yone/skins/skin0/yone.dds".to_string(),
        )]);

        let r = repath_bin_strings(
            &mut tree,
            &opts(".yone1_", RepathLayout::InFolder),
            &idx,
            dummy_hash,
        );
        assert_eq!(r.strings_repathed, 0);
    }

    #[test]
    fn bin_string_matched_via_xxhash_alternate() {
        // Verifies that a canonical string is repathed when matched via hash in the index.
        let mut tree = make_tree_with_string("assets/characters/yone/skins/skin0/yone.dds");
        let h = dummy_hash("assets/characters/yone/skins/skin0/yone.dds");
        let mut idx = WadIndex::new();
        idx.hashes.insert(h);

        let r = repath_bin_strings(
            &mut tree,
            &opts(".yone1_", RepathLayout::InFolder),
            &idx,
            dummy_hash,
        );
        assert_eq!(r.strings_repathed, 1);
    }

    #[test]
    fn bin_string_dds_tex_alternate() {
        // BIN says .dds — still gets repathed when the mod ships the alternate .tex.
        let mut tree = make_tree_with_string("assets/characters/yone/skins/skin0/yone.dds");
        let idx = WadIndex::from_entries(vec![(
            0,
            "assets/characters/yone/skins/skin0/yone.tex".to_string(),
        )]);

        let r = repath_bin_strings(
            &mut tree,
            &opts(".yone1_", RepathLayout::InFolder),
            &idx,
            dummy_hash,
        );
        assert_eq!(r.strings_repathed, 1);
    }

    #[test]
    fn bin_string_extension_alignment() {
        // BIN references .sco — but mod contains converted .scb in WAD.
        // String should be repathed and renamed to .scb, mapping should contain both keys.
        let mut tree = make_tree_with_string("assets/characters/yone/skins/skin0/yone.sco");
        let idx = WadIndex::from_entries(vec![(
            0,
            "assets/characters/yone/skins/skin0/yone.scb".to_string(),
        )]);

        let r = repath_bin_strings(
            &mut tree,
            &opts(".yone1_", RepathLayout::InFolder),
            &idx,
            dummy_hash,
        );
        assert_eq!(r.strings_repathed, 1);
        
        let expected_repathed = "ASSETS/.yone1_characters/yone/skins/skin0/yone.scb";
        
        // The value in the tree should be mutated to the aligned new extension
        let obj = tree.objects.get(&0x5678).unwrap();
        let prop = obj.properties.get(&0x1).unwrap();
        if let PropertyValue::String(s) = &prop.value {
            assert_eq!(s, expected_repathed);
        } else {
            panic!("Expected String property");
        }

        // Both original and actual keys should map to the repathed path
        assert_eq!(
            r.mapping.get("assets/characters/yone/skins/skin0/yone.sco").unwrap(),
            expected_repathed
        );
        assert_eq!(
            r.mapping.get("assets/characters/yone/skins/skin0/yone.scb").unwrap(),
            expected_repathed
        );
    }

    #[test]
    fn bin_string_skips_vo() {
        let mut tree = make_tree_with_string("assets/sounds/wwise2016/vo/yone/en_us/yone_vo.bnk");
        let idx = WadIndex::from_entries(vec![(
            0,
            "assets/sounds/wwise2016/vo/yone/en_us/yone_vo.bnk".to_string(),
        )]);

        let r = repath_bin_strings(
            &mut tree,
            &opts(".yone1_", RepathLayout::InFolder),
            &idx,
            dummy_hash,
        );
        assert_eq!(r.strings_repathed, 0);
    }

    #[test]
    fn linked_deps_repathed() {
        let mut tree = make_tree_with_string("dummy");
        tree.linked = vec![
            "data/characters/yone/yone.bin".to_string(), // root — skip
            "data/characters/yone/animations/skin0.bin".to_string(), // linked — repath
        ];
        let idx = WadIndex::from_entries(vec![
            (0, "data/characters/yone/yone.bin".to_string()),
            (0, "data/characters/yone/animations/skin0.bin".to_string()),
        ]);

        let r = repath_bin_strings(
            &mut tree,
            &opts(".yone1_", RepathLayout::InFolder),
            &idx,
            dummy_hash,
        );
        assert_eq!(r.strings_repathed, 1);
        assert_eq!(tree.linked[0], "data/characters/yone/yone.bin");
        assert_eq!(
            tree.linked[1],
            "DATA/.yone1_characters/yone/animations/skin0.bin"
        );
    }

    #[test]
    fn linked_deps_skipped_when_file_absent() {
        let mut tree = make_tree_with_string("dummy");
        tree.linked = vec![
            "data/characters/yone/animations/skin0.bin".to_string(),
        ];
        let idx = WadIndex::new(); // Empty index

        let r = repath_bin_strings(
            &mut tree,
            &opts(".yone1_", RepathLayout::InFolder),
            &idx,
            dummy_hash,
        );
        assert_eq!(r.strings_repathed, 0);
        assert_eq!(tree.linked[0], "data/characters/yone/animations/skin0.bin");
    }

    // -- placeholders --------------------------------------------------

    #[test]
    fn placeholder_dedup_and_normalise() {
        let existing: HashSet<String> = vec!["assets/x/existing.tex".to_string()]
            .into_iter()
            .collect();
        let referenced = vec![
            "assets/x/existing.dds".to_string(), // → existing.tex (skip, already there)
            "assets/x/missing.dds".to_string(),  // → missing.tex (add)
            "assets/x/missing.tex".to_string(),  // dup of above
            "assets/x/other.tex".to_string(),    // add
            "assets/x/model.skn".to_string(),    // not a tex
        ];
        let p = missing_invis_placeholders(&existing, &referenced);
        assert_eq!(p.len(), 2);
        assert!(p.iter().any(|(p, _)| p == "assets/x/missing.tex"));
        assert!(p.iter().any(|(p, _)| p == "assets/x/other.tex"));
        for (_, bytes) in &p {
            assert_eq!(bytes.as_slice(), INVIS_TEX);
        }
    }

    // -- magic detection -----------------------------------------------

    #[test]
    fn looks_like_bin_detects_prop_and_ptch() {
        assert!(looks_like_bin(b"PROP\x00\x00\x00\x00"));
        assert!(looks_like_bin(b"PTCH\x00\x00\x00\x00"));
        assert!(!looks_like_bin(b"DDS \x00\x00\x00\x00"));
        assert!(!looks_like_bin(b"PRO"));
    }

    // -- prefix derivation ---------------------------------------------

    #[test]
    fn derive_prefix_topaz_style() {
        assert_eq!(RepathOptions::derive_prefix("Yone", 1), ".yone1_");
        assert_eq!(RepathOptions::derive_prefix("Aatrox", 27), ".aatr27_");
        assert_eq!(RepathOptions::derive_prefix("MissFortune", 10), ".miss10_");
        assert_eq!(RepathOptions::derive_prefix("Kha'Zix", 0), ".khaz0_");
        assert_eq!(RepathOptions::derive_prefix("", 5), "bum");
    }

    // -- remove_prefix (inverse) ---------------------------------------

    #[test]
    fn remove_prefix_topaz_in_folder() {
        assert_eq!(
            remove_prefix("ASSETS/.yone1_characters/yone/skin.dds", ".yone1_"),
            "assets/characters/yone/skin.dds"
        );
        assert_eq!(
            remove_prefix(
                "DATA/.yone1_characters/yone/animations/skin0.bin",
                ".yone1_"
            ),
            "data/characters/yone/animations/skin0.bin"
        );
    }

    #[test]
    fn remove_prefix_nested() {
        assert_eq!(
            remove_prefix("assets/bum/characters/yone/skin.dds", "bum"),
            "assets/characters/yone/skin.dds"
        );
    }

    #[test]
    fn remove_prefix_modder_root() {
        assert_eq!(
            remove_prefix("bum/reddivinekinggaren/foo.dds", "bum"),
            "reddivinekinggaren/foo.dds"
        );
    }

    #[test]
    fn remove_prefix_idempotent_on_unprefixed() {
        // Paths without the prefix are returned unchanged.
        assert_eq!(
            remove_prefix("assets/characters/yone/skin.dds", ".yone1_"),
            "assets/characters/yone/skin.dds"
        );
    }

    #[test]
    fn remove_prefix_round_trip_in_folder() {
        let original = "assets/characters/yone/skin.dds";
        let repathed = repath_path(original, ".yone1_", RepathLayout::InFolder);
        // Path round-trip strips back to lowercased canonical root form.
        assert_eq!(remove_prefix(&repathed, ".yone1_"), original);
    }

    #[test]
    fn remove_prefix_round_trip_modder_root() {
        let original = "reddivinekinggaren/foo.dds";
        let repathed = repath_path(original, "bum", RepathLayout::InFolder);
        assert_eq!(remove_prefix(&repathed, "bum"), original);
    }

    // -- WAD path repathing covers modder-root entries ------------------

    #[test]
    fn repath_wad_path_modder_root_entry() {
        // Mod ships a file under its own handle root; engine references
        // it via repathed BIN string, so the WAD entry must also be
        // renamed to keep the hash in sync.
        assert_eq!(
            repath_wad_path("reddivinekinggaren/foo.dds", "bum", RepathLayout::InFolder),
            Some("bum/reddivinekinggaren/foo.dds".to_string())
        );
    }

    #[test]
    fn repath_wad_path_ignores_unknown_extension() {
        // .txt isn't in MODDER_ROOT_EXTS — leave the entry alone.
        assert!(repath_wad_path("modder/notes.txt", "bum", RepathLayout::InFolder).is_none());
    }

    #[test]
    fn bin_string_backslash_normalization() {
        let mut tree = make_tree_with_string("assets\\characters\\yone\\skins\\skin0\\yone.dds");
        let idx = WadIndex::from_entries(vec![(
            0,
            "assets/characters/yone/skins/skin0/yone.dds".to_string(),
        )]);

        let r = repath_bin_strings(
            &mut tree,
            &opts(".yone1_", RepathLayout::InFolder),
            &idx,
            dummy_hash,
        );
        assert_eq!(r.strings_repathed, 1);
        assert_eq!(r.mapping.len(), 1);
        let new = r
            .mapping
            .get("assets/characters/yone/skins/skin0/yone.dds")
            .unwrap();
        assert_eq!(new, "ASSETS/.yone1_characters/yone/skins/skin0/yone.dds");

        // Verify the string inside the tree was updated to the repathed path with forward slashes
        let strings = crate::walk::extract_strings(&tree);
        assert_eq!(strings[0], "ASSETS/.yone1_characters/yone/skins/skin0/yone.dds");
    }
}
