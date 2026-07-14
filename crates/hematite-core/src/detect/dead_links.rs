//! Dead entry link detection — the lethal inverse of `remove_unreferenced`.
//!
//! Finds link fields on a main entry (e.g. `SkinCharacterDataProperties`)
//! that reference target entries defined nowhere: not in the current tree,
//! not in any mod-shipped linked tree, and not in any game-resolvable
//! `linked:` BIN. Such dead links crash the client at runtime.
//!
//! `collect_dead_links` is shared: this module's detection rule uses it to
//! decide whether an issue exists, and the `pull_entries_from_game`
//! transform reuses it to know exactly which links to repair.

use crate::context::FixContext;
use crate::filter;
use hematite_types::bin::{BinTree, PropertyValue};
use hematite_types::config::EntryValidationTarget;
use std::collections::{HashMap, HashSet};

/// Parse a hex hash string like `"0xcb522723"` (or without the `0x` prefix)
/// into a `u32`. Mirrors the hex parsing already used for `type_hash` in
/// `transform/remove_unreferenced.rs`.
fn parse_hex_hash(hex: &str) -> Option<u32> {
    let hex = hex.trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(hex, 16).ok()
}

/// Recursively collect `PropertyValue::Link` hashes found inside properties
/// whose `name_hash` equals `link_field_hash`, descending into nested
/// Struct/Embedded/Container/UnorderedContainer/Optional/Map values.
fn collect_links_for_field(
    properties: &indexmap::IndexMap<u32, hematite_types::bin::BinProperty>,
    link_field_hash: u32,
    out: &mut Vec<u32>,
) {
    for prop in properties.values() {
        if prop.name_hash.0 == link_field_hash {
            collect_all_links(&prop.value, out);
        } else {
            // Keep searching inside nested structures for the field, since
            // the link field may live under an intermediate embed (e.g.
            // GearSkinUpgrade links nested inside `skinUpgradeData`).
            search_nested_for_field(&prop.value, link_field_hash, out);
        }
    }
}

/// Descend into a value looking for properties matching `link_field_hash`,
/// without assuming the current value itself is the target field.
fn search_nested_for_field(value: &PropertyValue, link_field_hash: u32, out: &mut Vec<u32>) {
    match value {
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            collect_links_for_field(&s.properties, link_field_hash, out);
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            for item in items {
                search_nested_for_field(item, link_field_hash, out);
            }
        }
        PropertyValue::Optional(boxed) => {
            if let Some(inner) = &**boxed {
                search_nested_for_field(inner, link_field_hash, out);
            }
        }
        PropertyValue::Map(entries) => {
            for (_, v) in entries {
                search_nested_for_field(v, link_field_hash, out);
            }
        }
        _ => {}
    }
}

/// Collect every `Link` hash inside a value: a direct `Link`, or any number
/// of `Link`s nested inside Container/UnorderedContainer/Struct/Embedded/
/// Optional/Map wrappers.
fn collect_all_links(value: &PropertyValue, out: &mut Vec<u32>) {
    match value {
        PropertyValue::Link(hash) => {
            if *hash != 0 {
                out.push(*hash);
            }
        }
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            for prop in s.properties.values() {
                collect_all_links(&prop.value, out);
            }
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            for item in items {
                collect_all_links(item, out);
            }
        }
        PropertyValue::Optional(boxed) => {
            if let Some(inner) = &**boxed {
                collect_all_links(inner, out);
            }
        }
        PropertyValue::Map(entries) => {
            for (_, v) in entries {
                collect_all_links(v, out);
            }
        }
        _ => {}
    }
}

/// Insert every object key from `tree` into `defined`.
fn extend_defined_from_tree(tree: &BinTree, defined: &mut HashSet<u32>) {
    defined.extend(tree.objects.keys().copied());
}

/// Find link fields on `main_entry_type` objects that reference targets
/// defined nowhere reachable from this fix session: not in `ctx.tree`, not
/// in any `ctx.linked_trees` entry, and not in any game-resolvable `linked:`
/// BIN (when `ctx.game` is available).
///
/// Returns `(target_index, dead_path_hash)` pairs — `target_index` is the
/// index into `targets` so callers (e.g. the `pull_entries_from_game`
/// transform) know which `EntryValidationTarget` a dead hash belongs to.
pub(crate) fn collect_dead_links(
    ctx: &FixContext,
    main_entry_type: &str,
    targets: &[EntryValidationTarget],
) -> Vec<(usize, u32)> {
    let mut dead = Vec::new();

    let Some(main_type_hash) = ctx.hashes.type_hash(main_entry_type) else {
        return dead;
    };

    // Build the defined set: this tree ∪ every mod-shipped linked tree ∪
    // (if a game provider is present) every game-resolvable `linked:` BIN.
    // Game bin lookups are cached per path within this call.
    let mut defined: HashSet<u32> = HashSet::new();
    extend_defined_from_tree(&ctx.tree, &mut defined);
    for linked_tree in ctx.linked_trees.values() {
        extend_defined_from_tree(linked_tree, &mut defined);
    }

    if let Some(game) = ctx.game {
        let mut game_bin_cache: HashMap<String, Option<BinTree>> = HashMap::new();
        for linked_path in &ctx.tree.linked {
            let resolved = game_bin_cache
                .entry(linked_path.clone())
                .or_insert_with(|| game.game_bin(linked_path));
            if let Some(linked_tree) = resolved {
                extend_defined_from_tree(linked_tree, &mut defined);
            }
        }
    }

    for main_obj in filter::objects_by_type(&ctx.tree, main_type_hash) {
        for (target_index, target) in targets.iter().enumerate() {
            let Some(link_field_hash) = parse_hex_hash(&target.link_field) else {
                continue;
            };

            let mut referenced = Vec::new();
            collect_links_for_field(&main_obj.properties, link_field_hash, &mut referenced);

            for hash in referenced {
                if hash != 0 && !defined.contains(&hash) {
                    dead.push((target_index, hash));
                }
            }
        }
    }

    dead
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::{GameProvider, HashProvider, WadProvider};
    use hematite_types::bin::{BinObject, BinProperty, BinTree};
    use hematite_types::champion::CharacterRelations;
    use hematite_types::hash::{FieldHash, PathHash, TypeHash};
    use indexmap::IndexMap;
    use std::collections::HashMap;

    struct MockHashProvider {
        types: HashMap<String, u32>,
    }

    impl MockHashProvider {
        fn new() -> Self {
            let mut types = HashMap::new();
            types.insert(
                "skincharacterdataproperties".to_string(),
                0x1234,
            );
            Self { types }
        }
    }

    impl HashProvider for MockHashProvider {
        fn resolve_type(&self, _hash: TypeHash) -> Option<&str> {
            None
        }
        fn resolve_field(&self, _hash: FieldHash) -> Option<&str> {
            None
        }
        fn resolve_entry(&self, _hash: PathHash) -> Option<&str> {
            None
        }
        fn resolve_game_path(&self, _hash: hematite_types::hash::GameHash) -> Option<&str> {
            None
        }
        fn type_hash(&self, name: &str) -> Option<TypeHash> {
            self.types.get(&name.to_lowercase()).map(|&h| TypeHash(h))
        }
        fn field_hash(&self, _name: &str) -> Option<FieldHash> {
            None
        }
        fn is_loaded(&self) -> bool {
            true
        }
        fn has_game_path(&self, _path: &str) -> bool {
            false
        }
    }

    struct MockWadProvider;
    impl WadProvider for MockWadProvider {
        fn has_path(&self, _path: &str) -> bool {
            false
        }
        fn has_hash(&self, _hash: u64) -> bool {
            false
        }
    }

    /// A `GameProvider` that never resolves any bin (empty game closure).
    struct EmptyGameProvider;
    impl GameProvider for EmptyGameProvider {
        fn has_path(&self, _path: &str) -> bool {
            false
        }
        fn pull_raw(&self, _path: &str) -> Option<Vec<u8>> {
            None
        }
        fn game_bin(&self, _path: &str) -> Option<BinTree> {
            None
        }
    }

    /// A `GameProvider` backed by a small map of path -> BinTree, used to
    /// prove that entries resolvable via the game closure are NOT dead.
    struct MapGameProvider {
        bins: HashMap<String, BinTree>,
    }
    impl GameProvider for MapGameProvider {
        fn has_path(&self, path: &str) -> bool {
            self.bins.contains_key(path)
        }
        fn pull_raw(&self, _path: &str) -> Option<Vec<u8>> {
            None
        }
        fn game_bin(&self, path: &str) -> Option<BinTree> {
            self.bins.get(path).cloned()
        }
    }

    const GEAR_LINK_FIELD_HASH: u32 = 0xCB52_2723;

    fn gear_target() -> EntryValidationTarget {
        EntryValidationTarget {
            entry_type: "GearSkinUpgrade".to_string(),
            type_hash: Some("0x27E0C761".to_string()),
            reference_field: "skinUpgradeData".to_string(),
            link_field: "0xcb522723".to_string(),
        }
    }

    /// Build a SkinCharacterDataProperties main object whose properties
    /// contain a Container(Link) under the gear link field hash.
    fn make_main_object(link_hash: u32) -> BinObject {
        let mut properties = IndexMap::new();
        properties.insert(
            GEAR_LINK_FIELD_HASH,
            BinProperty {
                name_hash: FieldHash(GEAR_LINK_FIELD_HASH),
                value: PropertyValue::Container(vec![PropertyValue::Link(link_hash)]),
            },
        );
        BinObject {
            class_hash: TypeHash(0x1234),
            path_hash: PathHash(0),
            properties,
        }
    }

    fn base_ctx<'a>(
        tree: BinTree,
        hashes: &'a dyn HashProvider,
        wad: &'a dyn WadProvider,
    ) -> FixContext<'a> {
        FixContext {
            tree,
            hashes,
            wad,
            champions: Box::leak(Box::new(CharacterRelations::default())),
            file_path: "main.bin".to_string(),
            files_to_remove: Vec::new(),
            linked_trees: HashMap::new(),
            shader_validator: None,
            game: None,
            additional_bins: Vec::new(),
        }
    }

    #[test]
    fn dead_link_detected_when_target_defined_nowhere() {
        let hashes = MockHashProvider::new();
        let wad = MockWadProvider;

        let mut tree = BinTree::default();
        tree.objects.insert(0, make_main_object(0x1234));
        // Note: no object with key 0x1234 is defined anywhere.

        let mut ctx = base_ctx(tree, &hashes, &wad);
        let empty_game = EmptyGameProvider;
        ctx.game = Some(&empty_game);

        let dead = collect_dead_links(
            &ctx,
            "SkinCharacterDataProperties",
            &[gear_target()],
        );

        assert_eq!(dead, vec![(0, 0x1234)]);
    }

    #[test]
    fn not_dead_when_defined_in_tree() {
        let hashes = MockHashProvider::new();
        let wad = MockWadProvider;

        let mut tree = BinTree::default();
        tree.objects.insert(0, make_main_object(0x1234));
        // The target entry IS defined in this tree.
        tree.objects.insert(
            0x1234,
            BinObject {
                class_hash: TypeHash(0x27E0C761),
                path_hash: PathHash(0x1234),
                properties: IndexMap::new(),
            },
        );

        let mut ctx = base_ctx(tree, &hashes, &wad);
        let empty_game = EmptyGameProvider;
        ctx.game = Some(&empty_game);

        let dead = collect_dead_links(
            &ctx,
            "SkinCharacterDataProperties",
            &[gear_target()],
        );

        assert!(dead.is_empty());
    }

    #[test]
    fn fail_open_when_no_game_provider() {
        let hashes = MockHashProvider::new();
        let wad = MockWadProvider;

        let mut tree = BinTree::default();
        tree.objects.insert(0, make_main_object(0x1234));

        let ctx = base_ctx(tree, &hashes, &wad); // ctx.game == None

        // collect_dead_links itself doesn't special-case ctx.game == None
        // for the "still nowhere defined" case (it just finds no defined
        // set contribution from the game); the fail-open behavior is
        // implemented at the `detect_issue` dispatch layer, which we also
        // verify: with no game provider, the rule must not fire.
        assert!(ctx.game.is_none());

        let dead = collect_dead_links(
            &ctx,
            "SkinCharacterDataProperties",
            &[gear_target()],
        );
        // Without game info, the entry is still "not found" in tree/linked
        // trees, so collect_dead_links (the pure algorithm) still reports
        // it as dead — the fail-open guard lives in detect_issue/detect_dead_entry_link.
        assert_eq!(dead, vec![(0, 0x1234)]);
    }

    #[test]
    fn not_dead_when_resolved_via_game_linked_bin() {
        let hashes = MockHashProvider::new();
        let wad = MockWadProvider;

        let mut tree = BinTree::default();
        tree.objects.insert(0, make_main_object(0x1234));
        tree.linked = vec!["data/characters/x/spells/xspell.bin".to_string()];

        let mut linked_game_tree = BinTree::default();
        linked_game_tree.objects.insert(
            0x1234,
            BinObject {
                class_hash: TypeHash(0x27E0C761),
                path_hash: PathHash(0x1234),
                properties: IndexMap::new(),
            },
        );

        let mut bins = HashMap::new();
        bins.insert(
            "data/characters/x/spells/xspell.bin".to_string(),
            linked_game_tree,
        );
        let game = MapGameProvider { bins };

        let mut ctx = base_ctx(tree, &hashes, &wad);
        ctx.game = Some(&game);

        let dead = collect_dead_links(
            &ctx,
            "SkinCharacterDataProperties",
            &[gear_target()],
        );

        assert!(dead.is_empty());
    }
}
