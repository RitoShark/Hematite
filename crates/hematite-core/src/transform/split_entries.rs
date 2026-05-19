//! `SplitEntriesByType` transform.
//!
//! Move objects whose class name is in a target list out of the source
//! BIN and into a brand-new BIN file written at a templated path. Powers
//! VFX entry separation (the `VfxSystemDefinitionData` → `{champ}_vfx.bin`
//! split) and any future "factor out these object types" rule.
//!
//! The new BIN is appended to [`FixContext::additional_bins`]; the calling
//! CLI is responsible for writing those bytes into the rebuilt WAD.
//! Optionally, the new BIN's path is added to the source's linked-deps
//! list so the engine resolves both files together — that's what makes
//! the split behave the same as the original single-file BIN at runtime.

use crate::context::FixContext;
use crate::traits::HashProvider;
use hematite_types::bin::{BinObject, BinTree};
use hematite_types::hash::TypeHash;
use std::collections::HashSet;

/// Resolve `template` against `source_file_path`.
///
/// Supported substitutions:
///
/// * `{source_dir}` — directory (no trailing slash). Empty when the path
///   has no directory part.
/// * `{source_stem}` — file stem (no extension).
/// * `{source_ext}` — file extension (no leading dot). Empty when the
///   source has none.
/// * `{champion}` — champion folder name parsed from
///   `(data|assets)/characters/{champion}/...` (lowercased). Empty when
///   the source path doesn't match that shape.
/// * `{skin}` — first integer in the source stem (e.g. `skin27.bin` → `27`).
///   Empty when no digit run is present.
pub fn resolve_template(template: &str, source_file_path: &str) -> String {
    let path = source_file_path.replace('\\', "/");

    let (dir, file) = match path.rsplit_once('/') {
        Some((d, f)) => (d, f),
        None => ("", path.as_str()),
    };
    let (stem, ext) = match file.rsplit_once('.') {
        Some((s, e)) => (s, e),
        None => (file, ""),
    };

    let lower = path.to_lowercase();
    let champion = ["data/characters/", "assets/characters/"]
        .iter()
        .find_map(|p| lower.strip_prefix(p))
        .and_then(|rest| rest.split('/').next().map(|s| s.to_string()))
        .unwrap_or_default();

    let skin: String = stem
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();

    template
        .replace("{source_dir}", dir)
        .replace("{source_stem}", stem)
        .replace("{source_ext}", ext)
        .replace("{champion}", &champion)
        .replace("{skin}", &skin)
}

/// Apply the split. Returns the number of objects moved (not the number
/// of BINs created — that's at most one per invocation).
pub fn apply(
    ctx: &mut FixContext<'_>,
    entry_types: &[String],
    output_path_template: &str,
    link_in_source: bool,
) -> u32 {
    let class_hashes = resolve_class_hashes(ctx.hashes, entry_types);
    if class_hashes.is_empty() {
        tracing::warn!(
            entry_types = ?entry_types,
            "split_entries_by_type: none of the requested types resolved in the hash dictionary"
        );
        return 0;
    }

    // Snapshot keys to drain — IndexMap mutation during iteration is
    // borrow-checker-hostile, so collect first then move.
    let to_move: Vec<u32> = ctx
        .tree
        .objects
        .iter()
        .filter_map(|(k, obj)| class_hashes.contains(&obj.class_hash).then_some(*k))
        .collect();

    if to_move.is_empty() {
        return 0;
    }

    let mut new_tree = BinTree::default();
    for key in &to_move {
        if let Some(obj) = ctx.tree.objects.shift_remove(key) {
            let _ = insert_object(&mut new_tree, *key, obj);
        }
    }

    let moved = to_move.len() as u32;
    let new_path = resolve_template(output_path_template, &ctx.file_path);

    if link_in_source && !ctx.tree.linked.iter().any(|p| p == &new_path) {
        ctx.tree.linked.push(new_path.clone());
    }

    tracing::info!(
        source = %ctx.file_path,
        new_bin = %new_path,
        moved,
        "split_entries_by_type: extracted {} object(s) into new BIN",
        moved
    );

    ctx.additional_bins.push((new_path, new_tree));
    moved
}

fn resolve_class_hashes(hashes: &dyn HashProvider, entry_types: &[String]) -> HashSet<TypeHash> {
    entry_types
        .iter()
        .filter_map(|name| {
            let resolved = hashes.type_hash(name);
            if resolved.is_none() {
                tracing::warn!(
                    entry_type = name,
                    "split_entries_by_type: type name not in hash dictionary; skipping"
                );
            }
            resolved
        })
        .collect()
}

fn insert_object(tree: &mut BinTree, key: u32, obj: BinObject) -> bool {
    use indexmap::map::Entry;
    match tree.objects.entry(key) {
        Entry::Vacant(v) => {
            v.insert(obj);
            true
        }
        Entry::Occupied(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_basic_substitutions() {
        let out = resolve_template(
            "{source_dir}/{champion}_vfx_skin{skin}.bin",
            "data/characters/yone/skins/skin0.bin",
        );
        assert_eq!(out, "data/characters/yone/skins/yone_vfx_skin0.bin");
    }

    #[test]
    fn template_handles_assets_root_and_no_skin() {
        let out = resolve_template(
            "{champion}-{source_stem}-{source_ext}",
            "assets/characters/ahri/data.bin",
        );
        assert_eq!(out, "ahri-data-bin");
    }

    #[test]
    fn template_handles_path_with_no_directory() {
        // Bare filename with no champion / skin to resolve.
        let out = resolve_template("{source_stem}.{source_ext}", "skin0.bin");
        assert_eq!(out, "skin0.bin");
    }

    #[test]
    fn template_falls_back_to_empty_for_unknown_vars() {
        let out = resolve_template(
            "{champion}/{skin}/{source_stem}",
            "data/characters/yone/skins/skin27.bin",
        );
        assert_eq!(out, "yone/27/skin27");
    }
}
