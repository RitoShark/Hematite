//! `pull_entries_from_game` transform — pulls referenced-but-missing target
//! entries (e.g. `GearSkinUpgrade`, `ContextualActionData`) out of the live
//! game's BIN closure and injects them into the mod's tree.
//!
//! Reuses [`crate::detect::dead_links::collect_dead_links`] to find dead
//! links, then walks a bounded BFS over the game's `linked:` closure
//! (seeded from both the mod's own seed BIN path and its own `linked`
//! list) looking for objects that satisfy those dead hashes.
//!
//! Links that can't be pulled either nuke a configured fallback field on
//! the main entry (gear: drop `skinUpgradeData` so the client stops trying
//! to resolve the missing upgrade) or, absent a fallback field, are KEPT
//! untouched (CAC-shaped; Topaz semantics — the game resolves or ignores
//! them at runtime, and deleting them killed voiceovers).

use crate::context::FixContext;
use crate::detect::dead_links::{
    build_game_closure, closure_object_matches_target, collect_dead_links,
};
use crate::filter;
use hematite_types::bin::PropertyValue;
use hematite_types::config::EntryValidationTarget;

/// Remove `field_hash` from a property map, recursing one level into
/// Struct/Embedded values when not found at the top level. Returns true if
/// a removal happened.
fn remove_field_one_level(
    properties: &mut indexmap::IndexMap<u32, hematite_types::bin::BinProperty>,
    field_hash: u32,
) -> bool {
    if properties.shift_remove(&field_hash).is_some() {
        return true;
    }
    for prop in properties.values_mut() {
        match &mut prop.value {
            PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
                if s.properties.shift_remove(&field_hash).is_some() {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Pull referenced-but-missing target entries out of the live game's BIN
/// closure and inject them into `ctx.tree`. Unpullable links nuke
/// `nuke_fallback_field` on the main entry when one is configured;
/// otherwise they are kept untouched.
///
/// Returns the number of changes applied (pulls + nukes).
pub fn apply(
    ctx: &mut FixContext<'_>,
    main_entry_type: &str,
    targets: &[EntryValidationTarget],
    nuke_fallback_field: Option<&str>,
) -> u32 {
    let Some(game) = ctx.game else {
        tracing::debug!("pull_entries_from_game: skipped, no game provider available");
        return 0;
    };

    let dead = collect_dead_links(ctx, main_entry_type, targets);
    if dead.is_empty() {
        return 0;
    }

    let closure = build_game_closure(ctx, game);

    let mut count = 0u32;
    let mut unpulled: Vec<(usize, u32)> = Vec::new();

    for (target_idx, hash) in &dead {
        let target = &targets[*target_idx];
        if let Some(obj) = closure.get(hash) {
            if closure_object_matches_target(ctx, target, obj) {
                ctx.tree.objects.insert(*hash, obj.clone());
                tracing::info!(
                    "pull_entries_from_game: pulled {} {:08x} from game closure",
                    target.entry_type,
                    hash
                );
                count += 1;
                continue;
            }
        }
        unpulled.push((*target_idx, *hash));
    }

    if unpulled.is_empty() {
        return count;
    }

    let Some(main_type_hash) = ctx.hashes.type_hash(main_entry_type) else {
        return count;
    };

    if let Some(field_name) = nuke_fallback_field {
        if let Some(field_hash) = ctx.hashes.field_hash(field_name) {
            let field_hash = field_hash.0;
            let main_keys = filter::object_keys_by_type(&ctx.tree, main_type_hash);
            for key in main_keys {
                if let Some(obj) = ctx.tree.objects.get_mut(&key) {
                    if remove_field_one_level(&mut obj.properties, field_hash) {
                        tracing::info!(
                            "pull_entries_from_game: nuked fallback field {} on main entry {:08x}",
                            field_name,
                            key
                        );
                        count += 1;
                    }
                }
            }
        }
        return count;
    }

    // No fallback field: unpullable links are KEPT, matching Topaz — the
    // game resolves them through always-loaded BINs (champion bin,
    // multi_skins combo bin) or silently ignores them. Deleting them killed
    // voiceovers on hand-fixed mods.
    for (target_idx, hash) in unpulled {
        tracing::debug!(
            "pull_entries_from_game: keeping unpullable {} link {:08x}",
            targets[target_idx].entry_type,
            hash
        );
    }

    count
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

    const MAIN_TYPE_HASH: u32 = 0x1234;
    const GEAR_TYPE_HASH: u32 = 0x27E0_C761;
    const GEAR_LINK_FIELD_HASH: u32 = 0xCB52_2723;
    const SKIN_UPGRADE_DATA_FIELD_HASH: u32 = 0xAAAA_0001;
    const CAC_LINK_FIELD_HASH: u32 = 0xD8F6_4A0D;

    struct MockHashProvider {
        types: HashMap<String, u32>,
        fields: HashMap<String, u32>,
    }

    impl MockHashProvider {
        fn new() -> Self {
            let mut types = HashMap::new();
            types.insert("skincharacterdataproperties".to_string(), MAIN_TYPE_HASH);
            types.insert("gearskinupgrade".to_string(), GEAR_TYPE_HASH);

            let mut fields = HashMap::new();
            fields.insert("skinupgradedata".to_string(), SKIN_UPGRADE_DATA_FIELD_HASH);

            Self { types, fields }
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
        fn field_hash(&self, name: &str) -> Option<FieldHash> {
            self.fields.get(&name.to_lowercase()).map(|&h| FieldHash(h))
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

    /// A `GameProvider` backed by a small map of path -> BinTree.
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

    fn gear_target() -> EntryValidationTarget {
        EntryValidationTarget {
            entry_type: "GearSkinUpgrade".to_string(),
            type_hash: Some(format!("0x{:08X}", GEAR_TYPE_HASH)),
            reference_field: "skinUpgradeData".to_string(),
            link_field: format!("0x{:08x}", GEAR_LINK_FIELD_HASH),
        }
    }

    fn cac_target() -> EntryValidationTarget {
        EntryValidationTarget {
            entry_type: "ContextualActionData".to_string(),
            type_hash: None,
            reference_field: "mContextualActionData".to_string(),
            link_field: format!("0x{:08x}", CAC_LINK_FIELD_HASH),
        }
    }

    /// Main object with a gear link nested inside a `skinUpgradeData` embed,
    /// mirroring the real GearSkinUpgrade structure the brief describes.
    fn make_gear_main_object(link_hash: u32) -> BinObject {
        let mut inner_props = IndexMap::new();
        inner_props.insert(
            GEAR_LINK_FIELD_HASH,
            BinProperty {
                name_hash: FieldHash(GEAR_LINK_FIELD_HASH),
                value: PropertyValue::Container(vec![PropertyValue::Link(link_hash)]),
            },
        );

        let mut properties = IndexMap::new();
        properties.insert(
            SKIN_UPGRADE_DATA_FIELD_HASH,
            BinProperty {
                name_hash: FieldHash(SKIN_UPGRADE_DATA_FIELD_HASH),
                value: PropertyValue::Embedded(hematite_types::bin::StructValue {
                    class_hash: TypeHash(GEAR_TYPE_HASH),
                    properties: inner_props,
                }),
            },
        );

        BinObject {
            class_hash: TypeHash(MAIN_TYPE_HASH),
            path_hash: PathHash(0),
            properties,
        }
    }

    /// Main object with a CAC-style link directly under the link field hash
    /// (no intermediate embed), matching how CAC references are typically
    /// stored.
    fn make_cac_main_object(link_hash: u32) -> BinObject {
        let mut properties = IndexMap::new();
        properties.insert(
            CAC_LINK_FIELD_HASH,
            BinProperty {
                name_hash: FieldHash(CAC_LINK_FIELD_HASH),
                value: PropertyValue::Container(vec![PropertyValue::Link(link_hash)]),
            },
        );

        BinObject {
            class_hash: TypeHash(MAIN_TYPE_HASH),
            path_hash: PathHash(0),
            properties,
        }
    }

    fn base_ctx<'a>(
        tree: BinTree,
        hashes: &'a dyn HashProvider,
        wad: &'a dyn WadProvider,
        file_path: &str,
    ) -> FixContext<'a> {
        FixContext {
            tree,
            hashes,
            wad,
            champions: Box::leak(Box::new(CharacterRelations::default())),
            file_path: file_path.to_string(),
            files_to_remove: Vec::new(),
            linked_trees: HashMap::new(),
            shader_validator: None,
            game: None,
            additional_bins: Vec::new(),
        }
    }

    #[test]
    fn pulls_missing_gear_entry_from_game_closure() {
        let hashes = MockHashProvider::new();
        let wad = MockWadProvider;

        let mut tree = BinTree::default();
        tree.objects.insert(0, make_gear_main_object(0x1111_1111));

        let file_path = "data/characters/x/skins/skin0.bin";
        let mut ctx = base_ctx(tree, &hashes, &wad, file_path);

        let mut game_tree = BinTree::default();
        game_tree.objects.insert(
            0x1111_1111,
            BinObject {
                class_hash: TypeHash(GEAR_TYPE_HASH),
                path_hash: PathHash(0x1111_1111),
                properties: IndexMap::new(),
            },
        );

        let mut bins = HashMap::new();
        bins.insert(file_path.to_string(), game_tree);
        let game = MapGameProvider { bins };
        ctx.game = Some(&game);

        let count = apply(
            &mut ctx,
            "SkinCharacterDataProperties",
            &[gear_target()],
            None,
        );

        assert!(count >= 1);
        assert!(ctx.tree.objects.contains_key(&0x1111_1111));
        let pulled = &ctx.tree.objects[&0x1111_1111];
        assert_eq!(pulled.class_hash.0, GEAR_TYPE_HASH);
    }

    #[test]
    fn nukes_fallback_field_when_unpullable() {
        let hashes = MockHashProvider::new();
        let wad = MockWadProvider;

        let mut tree = BinTree::default();
        tree.objects.insert(0, make_gear_main_object(0x2222_2222));

        let file_path = "data/characters/x/skins/skin0.bin";
        let mut ctx = base_ctx(tree, &hashes, &wad, file_path);

        // No defining tree anywhere in the game -> unpullable.
        let empty_game = EmptyGameProvider;
        ctx.game = Some(&empty_game);

        let count = apply(
            &mut ctx,
            "SkinCharacterDataProperties",
            &[gear_target()],
            Some("skinUpgradeData"),
        );

        assert!(count >= 1);
        let main_obj = &ctx.tree.objects[&0];
        assert!(
            !main_obj
                .properties
                .contains_key(&SKIN_UPGRADE_DATA_FIELD_HASH),
            "skinUpgradeData embed should have been removed"
        );
    }

    #[test]
    fn keeps_unpullable_link_when_no_nuke_field() {
        let hashes = MockHashProvider::new();
        let wad = MockWadProvider;

        let mut tree = BinTree::default();
        tree.objects.insert(0, make_cac_main_object(0x3333_3333));

        let file_path = "data/characters/x/skins/skin0.bin";
        let mut ctx = base_ctx(tree, &hashes, &wad, file_path);

        let empty_game = EmptyGameProvider;
        ctx.game = Some(&empty_game);

        let count = apply(
            &mut ctx,
            "SkinCharacterDataProperties",
            &[cac_target()],
            None,
        );

        assert_eq!(count, 0, "unpullable links are kept, not dropped");
        let main_obj = &ctx.tree.objects[&0];
        match &main_obj.properties[&CAC_LINK_FIELD_HASH].value {
            PropertyValue::Container(items) => {
                assert!(
                    items
                        .iter()
                        .any(|v| matches!(v, PropertyValue::Link(h) if *h == 0x3333_3333)),
                    "unpullable link value must survive untouched"
                );
            }
            other => panic!("expected Container, got {other:?}"),
        }
    }

    #[test]
    fn keeps_bare_link_property_when_unpullable() {
        let hashes = MockHashProvider::new();
        let wad = MockWadProvider;

        let mut properties = IndexMap::new();
        properties.insert(
            CAC_LINK_FIELD_HASH,
            BinProperty {
                name_hash: FieldHash(CAC_LINK_FIELD_HASH),
                value: PropertyValue::Link(0x4444_4444),
            },
        );
        let mut tree = BinTree::default();
        tree.objects.insert(
            0,
            BinObject {
                class_hash: TypeHash(MAIN_TYPE_HASH),
                path_hash: PathHash(0),
                properties,
            },
        );

        let file_path = "data/characters/x/skins/skin0.bin";
        let mut ctx = base_ctx(tree, &hashes, &wad, file_path);

        let empty_game = EmptyGameProvider;
        ctx.game = Some(&empty_game);

        let count = apply(
            &mut ctx,
            "SkinCharacterDataProperties",
            &[cac_target()],
            None,
        );

        assert_eq!(count, 0);
        match &ctx.tree.objects[&0].properties[&CAC_LINK_FIELD_HASH].value {
            PropertyValue::Link(h) => assert_eq!(*h, 0x4444_4444),
            other => panic!("bare Link must survive untouched, got {other:?}"),
        }
    }

    #[test]
    fn noop_without_game_provider() {
        let hashes = MockHashProvider::new();
        let wad = MockWadProvider;

        let mut tree = BinTree::default();
        tree.objects.insert(0, make_gear_main_object(0x4444_4444));

        let file_path = "data/characters/x/skins/skin0.bin";
        let mut ctx = base_ctx(tree, &hashes, &wad, file_path); // ctx.game == None

        let count = apply(
            &mut ctx,
            "SkinCharacterDataProperties",
            &[gear_target()],
            Some("skinUpgradeData"),
        );

        assert_eq!(count, 0);
        // Tree untouched: still has the original embed with the dead link.
        let main_obj = &ctx.tree.objects[&0];
        assert!(main_obj
            .properties
            .contains_key(&SKIN_UPGRADE_DATA_FIELD_HASH));
    }
}
