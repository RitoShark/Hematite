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
//! to resolve the missing upgrade) or, absent a fallback field, simply drop
//! the dead `Link` value from whatever container held it (CAC-shaped).

use crate::context::FixContext;
use crate::detect::dead_links::collect_dead_links;
use crate::filter;
use crate::seeds;
use hematite_types::bin::{BinObject, PropertyValue};
use hematite_types::config::EntryValidationTarget;
use std::collections::{HashMap, HashSet};

/// Hard cap on the number of game BINs visited while building the closure.
/// Prevents runaway BFS on pathological/cyclic `linked:` graphs.
const CLOSURE_CAP: usize = 64;

/// Parse a hex hash string like `"0xcb522723"` (or without the `0x` prefix)
/// into a `u32`.
fn parse_hex_hash(hex: &str) -> Option<u32> {
    let hex = hex.trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(hex, 16).ok()
}

/// Resolve a target's class hash: prefer the explicit `type_hash` hex
/// string, fall back to resolving `entry_type` through the hash dictionary.
fn resolve_target_class_hash(ctx: &FixContext, target: &EntryValidationTarget) -> Option<u32> {
    if let Some(hex) = &target.type_hash {
        if let Some(h) = parse_hex_hash(hex) {
            return Some(h);
        }
    }
    ctx.hashes.type_hash(&target.entry_type).map(|h| h.0)
}

/// Build the live game's BIN closure reachable from this fix session:
/// seed bin paths (from `seeds::discover_seeds`) plus every path already in
/// `ctx.tree.linked`, then BFS through each fetched tree's own `.linked`.
/// Returns a map of path_hash -> BinObject for every object seen, capped at
/// `CLOSURE_CAP` visited bins.
fn build_game_closure(
    ctx: &FixContext,
    game: &dyn crate::traits::GameProvider,
) -> HashMap<u32, BinObject> {
    let mut closure: HashMap<u32, BinObject> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();

    for seed in seeds::discover_seeds([ctx.file_path.as_str()]) {
        queue.push_back(seed.bin_path());
    }
    for linked in &ctx.tree.linked {
        queue.push_back(linked.clone());
    }

    while let Some(path) = queue.pop_front() {
        if seen.len() >= CLOSURE_CAP {
            break;
        }
        if !seen.insert(path.clone()) {
            continue;
        }

        let Some(tree) = game.game_bin(&path) else {
            continue;
        };

        for (&hash, obj) in &tree.objects {
            closure.entry(hash).or_insert_with(|| obj.clone());
        }
        for linked in &tree.linked {
            if !seen.contains(linked) {
                queue.push_back(linked.clone());
            }
        }
    }

    closure
}

/// Whether a closure object satisfies a dead link's target: the target's
/// class hash (resolved via hex or dictionary) matches the object's
/// `class_hash`, or the target's class hash couldn't be resolved at all (in
/// which case we accept on hash match alone).
fn closure_object_matches_target(
    ctx: &FixContext,
    target: &EntryValidationTarget,
    obj: &BinObject,
) -> bool {
    match resolve_target_class_hash(ctx, target) {
        Some(expected) => obj.class_hash.0 == expected,
        None => true,
    }
}

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

/// Drop dead `Link` values from properties whose `name_hash` equals
/// `link_field_hash`, mirroring how `collect_dead_links` locates the link
/// field: match at the current level first, otherwise keep searching for
/// the field inside nested Struct/Embedded/Container/Optional/Map values
/// (the link field may sit under an intermediate embed). Returns the
/// number of values dropped.
fn drop_dead_links_for_field(
    properties: &mut indexmap::IndexMap<u32, hematite_types::bin::BinProperty>,
    link_field_hash: u32,
    dead: &HashSet<u32>,
) -> u32 {
    let mut count = 0;
    for prop in properties.values_mut() {
        if prop.name_hash.0 == link_field_hash {
            count += drop_dead_links_from_value(&mut prop.value, dead);
        } else {
            count += drop_nested_for_field(&mut prop.value, link_field_hash, dead);
        }
    }
    count
}

/// Descend into a value looking for properties matching `link_field_hash`,
/// without assuming the current value itself is the target field. The
/// mutable twin of `dead_links::search_nested_for_field`.
fn drop_nested_for_field(
    value: &mut PropertyValue,
    link_field_hash: u32,
    dead: &HashSet<u32>,
) -> u32 {
    match value {
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            drop_dead_links_for_field(&mut s.properties, link_field_hash, dead)
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            let mut count = 0;
            for item in items.iter_mut() {
                count += drop_nested_for_field(item, link_field_hash, dead);
            }
            count
        }
        PropertyValue::Optional(boxed) => {
            if let Some(inner) = &mut **boxed {
                drop_nested_for_field(inner, link_field_hash, dead)
            } else {
                0
            }
        }
        PropertyValue::Map(entries) => {
            let mut count = 0;
            for (_, v) in entries.iter_mut() {
                count += drop_nested_for_field(v, link_field_hash, dead);
            }
            count
        }
        _ => 0,
    }
}

/// Recursively drop any `PropertyValue::Link(hash)` matching a hash in
/// `dead` from containers/structs/optionals/maps. Only ever called on
/// values already scoped under the target's `link_field` (see
/// [`drop_dead_links_for_field`]). Returns the number of values dropped.
fn drop_dead_links_from_value(value: &mut PropertyValue, dead: &HashSet<u32>) -> u32 {
    match value {
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            let mut count = 0;
            for prop in s.properties.values_mut() {
                count += drop_dead_links_from_value(&mut prop.value, dead);
            }
            count
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            let before = items.len();
            items.retain(|item| !matches!(item, PropertyValue::Link(h) if dead.contains(h)));
            let mut count = (before - items.len()) as u32;
            for item in items.iter_mut() {
                count += drop_dead_links_from_value(item, dead);
            }
            count
        }
        PropertyValue::Optional(boxed) => {
            if let Some(inner) = &mut **boxed {
                if matches!(inner, PropertyValue::Link(h) if dead.contains(h)) {
                    **boxed = None;
                    return 1;
                }
                return drop_dead_links_from_value(inner, dead);
            }
            0
        }
        PropertyValue::Map(entries) => {
            let mut count = 0;
            for (_, v) in entries.iter_mut() {
                count += drop_dead_links_from_value(v, dead);
            }
            count
        }
        _ => 0,
    }
}

/// Pull referenced-but-missing target entries out of the live game's BIN
/// closure and inject them into `ctx.tree`. Unpullable links either nuke
/// `nuke_fallback_field` on the main entry or drop the dead link value.
///
/// Returns the number of changes applied (pulls + nukes/drops).
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

    // Group unpulled hashes by target so the drop walk stays scoped to the
    // container(s) under each target's own link_field — an unscoped sweep
    // could remove an unrelated Link elsewhere on the object whose u32
    // value happens to collide with a dead hash from a different field.
    let mut dead_by_target: HashMap<usize, HashSet<u32>> = HashMap::new();
    for (target_idx, hash) in unpulled {
        dead_by_target.entry(target_idx).or_default().insert(hash);
    }

    let main_keys = filter::object_keys_by_type(&ctx.tree, main_type_hash);
    for (target_idx, dead_set) in &dead_by_target {
        let target = &targets[*target_idx];
        let Some(link_field_hash) = parse_hex_hash(&target.link_field) else {
            continue;
        };
        for key in &main_keys {
            if let Some(obj) = ctx.tree.objects.get_mut(key) {
                let dropped =
                    drop_dead_links_for_field(&mut obj.properties, link_field_hash, dead_set);
                if dropped > 0 {
                    tracing::info!(
                        "pull_entries_from_game: dropped {} dead {} link value(s) from main entry {:08x}",
                        dropped,
                        target.entry_type,
                        key
                    );
                    count += dropped;
                }
            }
        }
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
            self.fields
                .get(&name.to_lowercase())
                .map(|&h| FieldHash(h))
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

        let count = apply(&mut ctx, "SkinCharacterDataProperties", &[gear_target()], None);

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
            !main_obj.properties.contains_key(&SKIN_UPGRADE_DATA_FIELD_HASH),
            "skinUpgradeData embed should have been removed"
        );
    }

    #[test]
    fn drops_dead_link_when_no_nuke_field() {
        let hashes = MockHashProvider::new();
        let wad = MockWadProvider;

        let mut tree = BinTree::default();
        tree.objects.insert(0, make_cac_main_object(0x3333_3333));

        let file_path = "data/characters/x/skins/skin0.bin";
        let mut ctx = base_ctx(tree, &hashes, &wad, file_path);

        let empty_game = EmptyGameProvider;
        ctx.game = Some(&empty_game);

        let count = apply(&mut ctx, "SkinCharacterDataProperties", &[cac_target()], None);

        assert!(count >= 1);
        let main_obj = &ctx.tree.objects[&0];
        let prop = &main_obj.properties[&CAC_LINK_FIELD_HASH];
        match &prop.value {
            PropertyValue::Container(items) => {
                assert!(
                    !items
                        .iter()
                        .any(|v| matches!(v, PropertyValue::Link(h) if *h == 0x3333_3333)),
                    "dead link value should have been dropped from container"
                );
            }
            other => panic!("expected Container, got {other:?}"),
        }
    }

    #[test]
    fn drop_walk_is_scoped_to_target_link_field() {
        const UNRELATED_FIELD_HASH: u32 = 0xBBBB_0002;
        const DEAD_HASH: u32 = 0x5555_5555;

        let hashes = MockHashProvider::new();
        let wad = MockWadProvider;

        // Main object: dead Link under the CAC link_field AND a second,
        // unrelated property whose Link has the SAME u32 value. Only the
        // link_field container may lose the value.
        let mut main_obj = make_cac_main_object(DEAD_HASH);
        main_obj.properties.insert(
            UNRELATED_FIELD_HASH,
            BinProperty {
                name_hash: FieldHash(UNRELATED_FIELD_HASH),
                value: PropertyValue::Container(vec![PropertyValue::Link(DEAD_HASH)]),
            },
        );

        let mut tree = BinTree::default();
        tree.objects.insert(0, main_obj);

        let file_path = "data/characters/x/skins/skin0.bin";
        let mut ctx = base_ctx(tree, &hashes, &wad, file_path);

        let empty_game = EmptyGameProvider;
        ctx.game = Some(&empty_game);

        let count = apply(&mut ctx, "SkinCharacterDataProperties", &[cac_target()], None);

        assert_eq!(count, 1, "exactly one drop: only the link_field's value");

        let main_obj = &ctx.tree.objects[&0];

        // The target link_field container must no longer hold the dead value.
        match &main_obj.properties[&CAC_LINK_FIELD_HASH].value {
            PropertyValue::Container(items) => {
                assert!(
                    !items
                        .iter()
                        .any(|v| matches!(v, PropertyValue::Link(h) if *h == DEAD_HASH)),
                    "dead link should be dropped from the target link_field"
                );
            }
            other => panic!("expected Container, got {other:?}"),
        }

        // The unrelated property must be untouched despite the colliding value.
        match &main_obj.properties[&UNRELATED_FIELD_HASH].value {
            PropertyValue::Container(items) => {
                assert!(
                    items
                        .iter()
                        .any(|v| matches!(v, PropertyValue::Link(h) if *h == DEAD_HASH)),
                    "unrelated property's Link must survive the scoped drop walk"
                );
            }
            other => panic!("expected Container, got {other:?}"),
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
        assert!(main_obj.properties.contains_key(&SKIN_UPGRADE_DATA_FIELD_HASH));
    }
}
