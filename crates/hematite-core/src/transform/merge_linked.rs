//! MergeLinkedBins transform.
//!
//! Recursively merges linked dependency BINs into the root/parent BIN
//! using first-wins semantics. Strips successfully merged BINs from
//! the parent's linked list and registers them for removal from the output WAD.

use crate::context::FixContext;
use std::collections::{HashSet, VecDeque};

/// Merges linked BIN files into the parent BIN tree and registers them for removal.
pub fn apply(ctx: &mut FixContext<'_>) -> u32 {
    let mut queue = VecDeque::new();
    for linked in &ctx.tree.linked {
        queue.push_back(linked.clone());
    }

    let mut visited = HashSet::new();
    let mut successfully_merged = HashSet::new();
    let mut objects_merged_count = 0;

    while let Some(path) = queue.pop_front() {
        if !visited.insert(path.clone()) {
            continue;
        }

        if let Some(linked_tree) = ctx.linked_trees.get(&path) {
            // Queue sub-dependencies of this linked tree
            for linked in &linked_tree.linked {
                queue.push_back(linked.clone());
            }

            // Merge objects with first-wins semantics
            for (&hash, obj) in &linked_tree.objects {
                if !ctx.tree.objects.contains_key(&hash) {
                    ctx.tree.objects.insert(hash, obj.clone());
                    objects_merged_count += 1;
                }
            }

            successfully_merged.insert(path);
        } else {
            tracing::warn!("Linked tree not found in ctx.linked_trees: {}", path);
        }
    }

    // Delete successfully merged dependency files from parent's linked list
    ctx.tree
        .linked
        .retain(|path| !successfully_merged.contains(path));

    // Register successfully merged paths for removal from WAD
    for path in successfully_merged {
        if !ctx.files_to_remove.contains(&path) {
            ctx.files_to_remove.push(path);
        }
    }

    objects_merged_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::bin::{BinObject, BinTree};
    use hematite_types::champion::CharacterRelations;
    use hematite_types::hash::{PathHash, TypeHash};
    use indexmap::IndexMap;
    use std::collections::HashMap;

    struct MockHashes;
    impl crate::traits::HashProvider for MockHashes {
        fn resolve_type(&self, _: TypeHash) -> Option<String> {
            None
        }
        fn resolve_field(&self, _: hematite_types::hash::FieldHash) -> Option<String> {
            None
        }
        fn resolve_entry(&self, _: PathHash) -> Option<String> {
            None
        }
        fn resolve_game_path(&self, _: hematite_types::hash::GameHash) -> Option<String> {
            None
        }
        fn type_hash(&self, _: &str) -> Option<TypeHash> {
            None
        }
        fn field_hash(&self, _: &str) -> Option<hematite_types::hash::FieldHash> {
            None
        }
        fn has_game_path(&self, _: &str) -> bool {
            false
        }
        fn is_loaded(&self) -> bool {
            true
        }
    }

    struct MockWad;
    impl crate::traits::WadProvider for MockWad {
        fn has_path(&self, _: &str) -> bool {
            false
        }
        fn has_hash(&self, _: u64) -> bool {
            false
        }
    }

    fn make_dummy_object(class_hash: u32, path_hash: u32) -> BinObject {
        BinObject {
            class_hash: TypeHash(class_hash),
            path_hash: PathHash(path_hash),
            properties: IndexMap::new(),
        }
    }

    #[test]
    fn test_recursive_merge_and_override_semantics() {
        // 1. Setup primary tree with dependencies on spell1 and spell2
        let mut main_tree = BinTree {
            linked: vec!["spell1.bin".to_string(), "spell2.bin".to_string()],
            ..Default::default()
        };
        main_tree.objects.insert(100, make_dummy_object(999, 100)); // class_hash 999 is root's version

        // 2. Setup linked spell1 (depends on particle)
        let mut spell1 = BinTree {
            linked: vec!["particle.bin".to_string()],
            ..Default::default()
        };
        spell1.objects.insert(100, make_dummy_object(111, 100)); // Should be overridden by root's 100
        spell1.objects.insert(200, make_dummy_object(111, 200)); // New, should be merged

        // 3. Setup linked spell2
        let mut spell2 = BinTree::default();
        spell2.objects.insert(200, make_dummy_object(222, 200)); // Should be overridden by spell1's 200 (first-wins)
        spell2.objects.insert(300, make_dummy_object(222, 300)); // New, should be merged

        // 4. Setup linked particle
        let mut particle = BinTree::default();
        particle.objects.insert(400, make_dummy_object(333, 400)); // New, should be merged recursively

        let mut linked_trees = HashMap::new();
        linked_trees.insert("spell1.bin".to_string(), spell1);
        linked_trees.insert("spell2.bin".to_string(), spell2);
        linked_trees.insert("particle.bin".to_string(), particle);

        let hash_provider = MockHashes;
        let wad_provider = MockWad;
        let champions = CharacterRelations::default();

        let mut ctx = FixContext {
            tree: main_tree,
            hashes: &hash_provider,
            wad: &wad_provider,
            champions: &champions,
            file_path: "main.bin".to_string(),
            files_to_remove: Vec::new(),
            linked_trees,
            shader_validator: None,
            game: None,
            additional_bins: Vec::new(),
            scope: Default::default(),
        };

        let merged_count = apply(&mut ctx);

        // 3 objects merged: 200 (from spell1), 300 (from spell2), 400 (from particle)
        assert_eq!(merged_count, 3);

        // Root class_hash for 100 must be preserved (not overridden by spell1's 111)
        let obj_100 = ctx.tree.objects.get(&100).expect("100 must exist");
        assert_eq!(obj_100.class_hash.0, 999);

        // Object 200 class_hash must come from spell1 (111), not spell2 (222)
        let obj_200 = ctx.tree.objects.get(&200).expect("200 must exist");
        assert_eq!(obj_200.class_hash.0, 111);

        // Object 300 and 400 must exist
        assert!(ctx.tree.objects.contains_key(&300));
        assert!(ctx.tree.objects.contains_key(&400));

        // Direct linked dependencies of root should be cleared (retained only if they failed to merge, but here all merged)
        assert!(ctx.tree.linked.is_empty());

        // All merged dependencies must be marked for removal from WAD
        assert_eq!(ctx.files_to_remove.len(), 3);
        assert!(ctx.files_to_remove.contains(&"spell1.bin".to_string()));
        assert!(ctx.files_to_remove.contains(&"spell2.bin".to_string()));
        assert!(ctx.files_to_remove.contains(&"particle.bin".to_string()));
    }
}
