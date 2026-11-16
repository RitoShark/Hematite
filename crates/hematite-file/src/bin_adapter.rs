//! BIN parsing adapter using rs_bin.

use crate::convert::rs_bin_tree_to_hematite;
use anyhow::Result;
use hematite_core::traits::BinProvider;
use hematite_types::bin::BinTree;
use rs_bin::Bin as RsBin;
use rs_io::{Parse, Serialize};

/// BIN provider backed by rs_bin.
pub struct FileBinProvider;

impl FileBinProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FileBinProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl BinProvider for FileBinProvider {
    fn parse_bytes(&self, data: &[u8]) -> Result<BinTree> {
        let rs_bin =
            RsBin::from_bytes(data).map_err(|e| anyhow::anyhow!("Failed to parse BIN: {:?}", e))?;
        rs_bin_tree_to_hematite(rs_bin)
    }

    fn write_bytes(&self, tree: &BinTree) -> Result<Vec<u8>> {
        use crate::convert::hematite_tree_to_rs_bin;

        let rs_bin = hematite_tree_to_rs_bin(tree)?;

        rs_bin
            .to_bytes()
            .map_err(|e| anyhow::anyhow!("Failed to write BIN: {:?}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::bin::{BinObject, BinProperty, PropertyValue};
    use hematite_types::hash::{FieldHash, PathHash, TypeHash};
    use indexmap::IndexMap;

    #[test]
    fn test_roundtrip_empty_tree() {
        let provider = FileBinProvider::new();
        let tree = BinTree {
            objects: IndexMap::new(),
            ..Default::default()
        };

        let bytes = provider.write_bytes(&tree).unwrap();
        let parsed = provider.parse_bytes(&bytes).unwrap();

        assert_eq!(tree.objects.len(), parsed.objects.len());
    }

    #[test]
    fn test_roundtrip_simple_object() {
        let provider = FileBinProvider::new();

        let mut properties = IndexMap::new();
        properties.insert(
            0x1111,
            BinProperty {
                name_hash: FieldHash(0x1111),
                value: PropertyValue::I32(42),
            },
        );
        properties.insert(
            0x2222,
            BinProperty {
                name_hash: FieldHash(0x2222),
                value: PropertyValue::String("test".to_string()),
            },
        );

        let obj = BinObject {
            path_hash: PathHash(0x1234),
            class_hash: TypeHash(0x5678),
            properties,
        };

        let mut objects = IndexMap::new();
        objects.insert(0x1234, obj);

        let tree = BinTree {
            objects,
            ..Default::default()
        };

        let bytes = provider.write_bytes(&tree).unwrap();
        let parsed = provider.parse_bytes(&bytes).unwrap();

        assert_eq!(tree.objects.len(), parsed.objects.len());
        let parsed_obj = parsed.objects.get(&0x1234).unwrap();
        assert_eq!(parsed_obj.path_hash.0, 0x1234);
        assert_eq!(parsed_obj.class_hash.0, 0x5678);
        assert_eq!(parsed_obj.properties.len(), 2);

        if let PropertyValue::I32(val) = parsed_obj.properties.get(&0x1111).unwrap().value {
            assert_eq!(val, 42);
        } else {
            panic!("Expected I32 value");
        }

        if let PropertyValue::String(val) = &parsed_obj.properties.get(&0x2222).unwrap().value {
            assert_eq!(val, "test");
        } else {
            panic!("Expected String value");
        }
    }
}

#[cfg(test)]
mod native_edit_tests {
    use super::*;
    use hematite_core::transform::edit;
    use hematite_types::edit::EditOperation;
    use rs_bin::{BinEntry, BinType, BinValue};
    use serde_json::json;

    #[test]
    fn binary_roundtrip_preserves_both_hash_widths() {
        for value in [
            BinValue::Hash(u32::MAX),
            BinValue::Hash64(0),
            BinValue::Hash64(7),
            BinValue::Hash64(0x7394ac037b93317f),
            BinValue::Hash64(u64::MAX),
        ] {
            let mut bin = RsBin::new();
            bin.entries.push(BinEntry {
                path_hash: 1,
                class_hash: 2,
                fields: [(3, value.clone())].into_iter().collect(),
            });
            let bytes = bin.to_bytes().unwrap();
            let tree = FileBinProvider.parse_bytes(&bytes).unwrap();
            let written = FileBinProvider.write_bytes(&tree).unwrap();
            assert_eq!(written, bytes, "hash width changed for {value:?}");
            assert_eq!(
                RsBin::from_bytes(&written).unwrap().entries[0].fields[&3],
                value
            );
        }
    }

    #[test]
    fn binary_roundtrip_preserves_empty_tags_and_patch_metadata_during_edits() {
        let mut bin = RsBin::new();
        bin.is_patch = true;
        bin.patch_header = [1, 0, 0, 0, 0, 0, 0, 0];
        bin.patches.push(rs_bin::BinPatch {
            key_hash: 2,
            path: "some.field".into(),
            value: BinValue::U8(1),
        });
        bin.entries.push(BinEntry {
            path_hash: 1,
            class_hash: edit::hash("Custom").unwrap(),
            fields: [
                (
                    edit::hash("asset").unwrap(),
                    BinValue::String("ASSETS/custom.tex".into()),
                ),
                (
                    2,
                    BinValue::Option {
                        item: BinType::File,
                        value: None,
                    },
                ),
                (
                    3,
                    BinValue::List {
                        is_list2: true,
                        item: BinType::Embed,
                        items: vec![],
                    },
                ),
                (
                    4,
                    BinValue::Map {
                        key: BinType::Hash,
                        value: BinType::String,
                        entries: vec![],
                    },
                ),
            ]
            .into_iter()
            .collect(),
        });
        let before = bin.to_bytes().unwrap();
        let mut tree = FileBinProvider.parse_bytes(&before).unwrap();
        assert_eq!(FileBinProvider.write_bytes(&tree).unwrap(), before);
        let ops: Vec<EditOperation> = serde_json::from_value(json!([
            {"op":"convert","select":{"classes":["Custom"],"path":["asset"]},"from":"string","to":"file"}
        ])).unwrap();
        assert_eq!(edit::apply(&mut tree, &ops, true).unwrap(), 1);
        assert_eq!(FileBinProvider.write_bytes(&tree).unwrap(), before);
        edit::apply(&mut tree, &ops, false).unwrap();
        let after = RsBin::from_bytes(&FileBinProvider.write_bytes(&tree).unwrap()).unwrap();
        bin.entries[0].fields.insert(
            edit::hash("asset").unwrap(),
            rs_bin::value_from_text(r#"file = "ASSETS/custom.tex""#, None).unwrap(),
        );
        assert_eq!(after, bin);
    }
}

#[cfg(test)]
mod edit_pipeline_tests {
    use super::*;
    use hematite_core::{context::FixContext, pipeline::apply_fixes_in_phase, traits::WadProvider};
    use hematite_types::{
        champion::CharacterRelations,
        config::{FixConfig, FixPhase},
    };

    struct EmptyWad;
    impl WadProvider for EmptyWad {
        fn has_path(&self, _: &str) -> bool {
            false
        }
        fn has_hash(&self, _: u64) -> bool {
            false
        }
    }

    #[test]
    fn json_rules_preview_apply_and_repeat_through_real_pipeline() {
        let config: FixConfig = serde_json::from_str(
            include_str!("../../../config/edit_example.json").trim_start_matches('\u{feff}'),
        )
        .unwrap();
        let bin = rs_bin::from_text(
            r#"#PROP_text
version: u32 = 3
entries: map[hash,embed] = {
    0x1 = SkinMeshDataProperties { texture: string = "assets/custom.tex" }
}
"#,
            None,
        )
        .unwrap();
        let original = bin.to_bytes().unwrap();
        let hashes = crate::hash_adapter::TxtHashProvider::new();
        let champions = CharacterRelations::default();
        let mut ctx = FixContext {
            tree: FileBinProvider.parse_bytes(&original).unwrap(),
            hashes: &hashes,
            wad: &EmptyWad,
            champions: &champions,
            file_path: "test.bin".into(),
            files_to_remove: vec![],
            linked_trees: Default::default(),
            shader_validator: None,
            game: None,
            additional_bins: vec![],
        };
        let ids = config.default_fix_ids();
        let standard = apply_fixes_in_phase(&mut ctx, &config, &ids, false, FixPhase::Standard);
        assert_eq!(standard.fixes_applied, 0);
        let preview = apply_fixes_in_phase(&mut ctx, &config, &ids, true, FixPhase::PostRepath);
        assert_eq!(preview.fixes_applied, 1);
        assert_eq!(FileBinProvider.write_bytes(&ctx.tree).unwrap(), original);
        let applied = apply_fixes_in_phase(&mut ctx, &config, &ids, false, FixPhase::PostRepath);
        assert_eq!(applied.fixes_applied, 1);
        assert_eq!(applied.fixes_failed, 0);
        assert_eq!(
            preview.applied_fixes[0].changes_count,
            applied.applied_fixes[0].changes_count
        );
        let repeated = apply_fixes_in_phase(&mut ctx, &config, &ids, false, FixPhase::PostRepath);
        assert_eq!(repeated.fixes_applied, 0);
        assert_eq!(repeated.fixes_failed, 0);
    }
}
