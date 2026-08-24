//! Textures a render pass binds without checking.
//!
//! Most missing textures are cosmetic. The client loads them asynchronously, and a handle
//! that never resolves falls back to a default: the surface renders wrong, the game keeps
//! running. That is why a dead texture reference is a warning nearly everywhere.
//!
//! A few are not like that. A reflection probe's texture is bound straight into the map's
//! lighting pass, with no fallback and no null check, so an absent file leaves the handle's
//! GPU data null and the pass dereferences it at bind time. The map does not render wrong;
//! it does not load.
//!
//! The distinction is the field the path is stored in, not the file, so this matches on
//! field hashes rather than on extensions. `TerrainPaintTexturePath` looks like it belongs
//! here and does not: a missing terrain paint texture falls back like any other.
//!
//! ## Only map mods
//! These fields exist only in map data, so the caller gates on the mod being a map mod.
//! Walking a skin mod's BINs looking for them finds nothing, every time, and most mods are
//! skins.
//!
//! Fail-open. An unparsable BIN or an unreadable archive means no finding.

use hematite_types::diagnostic::{Diagnostic, ReasonCatalog};
use std::collections::HashSet;

/// A field whose texture a render pass binds without a fallback.
#[derive(Debug, Clone)]
pub struct RequiredTextureField {
    /// FNV-1a of the lowercased field name.
    pub field_hash: u32,
    pub reason: String,
}

impl RequiredTextureField {
    /// Build the runtime list from config, hashing each field name.
    ///
    /// The hash is derived rather than written down: a field hash in a config file is a
    /// number nobody can check by eye, and a wrong one fails silently forever.
    pub fn from_config(entries: &[hematite_types::config::RequiredTextureConfig]) -> Vec<Self> {
        entries
            .iter()
            .filter(|e| e.enabled)
            .filter_map(|e| {
                Some(Self {
                    field_hash: crate::strings::fnv1a_hash(&e.field.to_ascii_lowercase()),
                    reason: e.reason.clone()?,
                })
            })
            .collect()
    }
}

/// Whether this mod is a map mod.
///
/// The fields above exist only in map data, so a skin mod is skipped without walking
/// anything. Matched on the mod's own file paths: map content lives under `maps/`, and
/// nothing else does.
pub fn is_map_mod<'a>(mut paths: impl Iterator<Item = &'a str>) -> bool {
    paths.any(|p| {
        let lower = p.to_ascii_lowercase().replace('\\', "/");
        lower.contains("/maps/") || lower.starts_with("maps/")
    })
}

/// Whether a value is a texture path worth resolving.
fn is_texture_path(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.ends_with(".dds") || lower.ends_with(".tex")
}

/// Every required-render texture path a BIN names, for the given fields.
///
/// These live inside map placeables, several container levels down, so the whole tree is
/// walked rather than just the top-level properties.
pub fn collect<'a>(
    tree: &'a hematite_types::bin::BinTree,
    fields: &[RequiredTextureField],
) -> Vec<(&'a str, u32)> {
    let mut out = Vec::new();
    for (hash, value) in crate::walk::string_fields(tree) {
        if !fields.iter().any(|f| f.field_hash == hash) {
            continue;
        }
        if is_texture_path(value) {
            out.push((value, hash));
        }
    }
    out
}

/// Report every required-render texture that resolves nowhere.
///
/// `resolves` answers whether a path exists in the mod or the game. The caller gates on
/// [`is_map_mod`]; these fields do not appear anywhere else.
pub fn run<'a>(
    fields: &[RequiredTextureField],
    catalog: &ReasonCatalog,
    bins: impl Iterator<Item = &'a hematite_types::bin::BinTree>,
    resolves: impl Fn(&str) -> bool,
) -> Vec<Diagnostic> {
    if fields.is_empty() {
        return Vec::new();
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for tree in bins {
        for (path, field_hash) in collect(tree, fields) {
            let lower = path.to_ascii_lowercase();
            if !seen.insert(lower) || resolves(path) {
                continue;
            }
            let Some(field) = fields.iter().find(|f| f.field_hash == field_hash) else {
                continue;
            };
            let leaf = path.rsplit(['/', '\\']).next().unwrap_or(path);
            tracing::info!("required texture missing: {}", path);
            out.push(
                Diagnostic::new(catalog, &field.reason, "required_texture")
                    .with_detail(leaf.to_string()),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_content_is_recognised_and_skins_are_not() {
        assert!(is_map_mod(["data/maps/shipping/map11/map11.bin"].into_iter()));
        assert!(is_map_mod(["ASSETS/Maps/Particles/x.dds"].into_iter()));
        assert!(!is_map_mod(
            ["data/characters/jhin/skins/skin0.bin"].into_iter()
        ));
        assert!(!is_map_mod(std::iter::empty()));
    }

    /// Config carries the field name; the hash is derived, never written by hand.
    #[test]
    fn fields_are_hashed_from_their_configured_names() {
        let cfg = vec![hematite_types::config::RequiredTextureConfig {
            enabled: true,
            field: "CubemapProbePath".into(),
            reason: Some("missing_map_texture".into()),
        }];
        let fields = RequiredTextureField::from_config(&cfg);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].field_hash, 0xfe38_0acf);
    }

    /// An entry with no reason has nothing to report and is dropped.
    #[test]
    fn a_field_without_a_reason_is_dropped() {
        let cfg = vec![hematite_types::config::RequiredTextureConfig {
            enabled: true,
            field: "CubemapProbePath".into(),
            reason: None,
        }];
        assert!(RequiredTextureField::from_config(&cfg).is_empty());
    }

    /// The field name is hashed the way BIN field names are, not the way paths are.
    #[test]
    fn the_cubemap_field_hash_is_fnv1a_of_the_lowercased_name() {
        assert_eq!(crate::strings::fnv1a_hash("cubemapprobepath"), 0xfe38_0acf);
    }

    #[test]
    fn only_texture_extensions_count() {
        assert!(is_texture_path("x/Probe.DDS"));
        assert!(is_texture_path("x/probe.tex"));
        assert!(!is_texture_path("x/probe.bin"));
        assert!(!is_texture_path("x/probe"));
    }
}
