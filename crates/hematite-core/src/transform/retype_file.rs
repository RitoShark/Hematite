//! RetypeStringToFile — Riot's string → xxh64 `file` asset-reference migration.
//! Matching is per (class, field), NOT per field name: `SkinMeshDataProperties.texture`
//! migrated while `VfxEmitterDefinitionData.texture` stayed string.

use crate::context::FixContext;
use crate::strings::fnv1a_hash;
use hematite_types::bin::{BinTree, PropertyValue};
use hematite_types::config::ClassFieldTarget;
use std::collections::HashSet;

pub fn wad_ref_hash(path: &str) -> u64 {
    xxhash_rust::xxh64::xxh64(path.to_lowercase().as_bytes(), 0)
}

fn resolve_hash(s: &str) -> u32 {
    s.strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .and_then(|h| u32::from_str_radix(h, 16).ok())
        .unwrap_or_else(|| fnv1a_hash(s))
}

fn resolve_targets(targets: &[ClassFieldTarget]) -> HashSet<(u32, u32)> {
    targets
        .iter()
        .map(|t| (resolve_hash(&t.class), resolve_hash(&t.field)))
        .collect()
}

pub fn apply(ctx: &mut FixContext, targets: &[ClassFieldTarget]) -> u32 {
    apply_to_tree(&mut ctx.tree, targets)
}

/// Compatibility entry point for old configs. The generic rs_bin editor owns conversion.
pub fn apply_to_tree(tree: &mut BinTree, targets: &[ClassFieldTarget]) -> u32 {
    use hematite_types::edit::{EditOperation, Selector};
    let operations = [EditOperation::Convert {
        select: Selector {
            targets: targets.to_vec(),
            ..Default::default()
        },
        from: "string".into(),
        to: "file".into(),
    }];
    match super::edit::apply(tree, &operations, false) {
        Ok(changes) => changes,
        Err(error) => {
            tracing::error!("legacy conversion failed: {error:#}");
            0
        }
    }
}
pub fn detect(tree: &BinTree, targets: &[ClassFieldTarget]) -> bool {
    let resolved = resolve_targets(targets);
    tree.objects.values().any(|obj| {
        obj.properties.values().any(|prop| {
            (resolved.contains(&(obj.class_hash.0, prop.name_hash.0)) && holds_string(&prop.value))
                || recurse_detect(&prop.value, &resolved)
        })
    })
}

fn recurse_detect(value: &PropertyValue, targets: &HashSet<(u32, u32)>) -> bool {
    match value {
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => s.properties.values().any(|p| {
            (targets.contains(&(s.class_hash.0, p.name_hash.0)) && holds_string(&p.value))
                || recurse_detect(&p.value, targets)
        }),
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            items.iter().any(|v| recurse_detect(v, targets))
        }
        PropertyValue::Optional(inner) => inner
            .as_ref()
            .as_ref()
            .is_some_and(|v| recurse_detect(v, targets)),
        PropertyValue::Map(entries) => entries.iter().any(|(_k, v)| recurse_detect(v, targets)),
        _ => false,
    }
}

fn holds_string(value: &PropertyValue) -> bool {
    match value {
        PropertyValue::String(_) => true,
        PropertyValue::Optional(inner) => inner.as_ref().as_ref().is_some_and(holds_string),
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            items.iter().any(holds_string)
        }
        PropertyValue::Map(entries) => entries.iter().any(|(_k, v)| holds_string(v)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::bin::{BinObject, BinProperty, StructValue};
    use hematite_types::hash::{FieldHash, PathHash, TypeHash};
    use indexmap::IndexMap;

    fn target(class: &str, field: &str) -> ClassFieldTarget {
        ClassFieldTarget {
            class: class.to_string(),
            field: field.to_string(),
        }
    }

    fn prop(field: &str, value: PropertyValue) -> (u32, BinProperty) {
        let h = fnv1a_hash(field);
        (
            h,
            BinProperty {
                name_hash: FieldHash(h),
                value,
            },
        )
    }

    fn tree_with(class: &str, props: Vec<(u32, BinProperty)>) -> BinTree {
        let mut objects = IndexMap::new();
        objects.insert(
            1,
            BinObject {
                class_hash: TypeHash(fnv1a_hash(class)),
                path_hash: PathHash(1),
                properties: props.into_iter().collect(),
            },
        );
        BinTree {
            objects,
            ..Default::default()
        }
    }

    #[test]
    fn converts_targeted_string_and_records_trailer_pair() {
        let path = "ASSETS/Characters/Aatrox/Skins/Base/AatroxLoadscreen.tex";
        let mut tree = tree_with(
            "CensoredImage",
            vec![prop("image", PropertyValue::String(path.to_string()))],
        );
        let targets = [target("CensoredImage", "image")];
        assert!(detect(&tree, &targets));

        let changes = apply_to_tree(&mut tree, &targets);
        assert_eq!(changes, 1);
        assert!(!detect(&tree, &targets));

        let obj = tree.objects.get(&1).unwrap();
        match &obj.properties.get(&fnv1a_hash("image")).unwrap().value {
            PropertyValue::WadHash(h) => assert_eq!(*h, 0xb7a434886d1ce5e6),
            other => panic!("expected WadHash, got {other:?}"),
        }
        assert_eq!(
            tree.recorded_files
                .get(&0xb7a434886d1ce5e6)
                .map(String::as_str),
            Some(path)
        );
    }

    #[test]
    fn class_scoping_leaves_same_field_in_other_class_alone() {
        let mut tree = tree_with(
            "VfxEmitterDefinitionData",
            vec![prop(
                "texture",
                PropertyValue::String("ASSETS/foo.tex".to_string()),
            )],
        );
        let targets = [target("SkinMeshDataProperties", "texture")];
        assert!(!detect(&tree, &targets));
        assert_eq!(apply_to_tree(&mut tree, &targets), 0);
        assert!(matches!(
            tree.objects.get(&1).unwrap().properties[&fnv1a_hash("texture")].value,
            PropertyValue::String(_)
        ));
    }

    #[test]
    fn converts_inside_nested_struct_option_and_list() {
        let sampler = StructValue {
            class_hash: TypeHash(fnv1a_hash("StaticMaterialShaderSamplerDef")),
            properties: [prop(
                "texturePath",
                PropertyValue::String("ASSETS/mat.tex".to_string()),
            )]
            .into_iter()
            .collect(),
        };
        let mut tree = tree_with(
            "SkinCharacterDataProperties",
            vec![
                prop(
                    "iconCircle",
                    PropertyValue::Optional(Box::new(Some(PropertyValue::String(
                        "ASSETS/circle.tex".to_string(),
                    )))),
                ),
                prop(
                    "samplerValues",
                    PropertyValue::Container(vec![PropertyValue::Embedded(sampler)]),
                ),
            ],
        );
        let targets = [
            target("SkinCharacterDataProperties", "iconCircle"),
            target("StaticMaterialShaderSamplerDef", "texturePath"),
        ];
        assert!(detect(&tree, &targets));
        assert_eq!(apply_to_tree(&mut tree, &targets), 2);
        assert_eq!(tree.recorded_files.len(), 2);
        assert!(!detect(&tree, &targets));

        let obj = tree.objects.get(&1).unwrap();
        match &obj.properties.get(&fnv1a_hash("iconCircle")).unwrap().value {
            PropertyValue::Optional(inner) => assert!(matches!(
                inner.as_ref().as_ref(),
                Some(PropertyValue::WadHash(_))
            )),
            other => panic!("expected optional, got {other:?}"),
        }
    }

    #[test]
    fn hex_targets_resolve_without_names() {
        let field_hex = format!("0x{:08x}", fnv1a_hash("image"));
        let class_hex = format!("0x{:08x}", fnv1a_hash("CensoredImage"));
        let tree = tree_with(
            "CensoredImage",
            vec![prop(
                "image",
                PropertyValue::String("ASSETS/a.tex".to_string()),
            )],
        );
        assert!(detect(&tree, &[target(&class_hex, &field_hex)]));
    }

    #[test]
    fn already_migrated_value_does_not_detect() {
        let tree = tree_with(
            "CensoredImage",
            vec![prop("image", PropertyValue::WadHash(0x1234))],
        );
        assert!(!detect(&tree, &[target("CensoredImage", "image")]));
    }

    #[test]
    fn wad_ref_hash_matches_game_hashing() {
        assert_eq!(
            wad_ref_hash("ASSETS/Characters/Aatrox/Skins/Base/AatroxLoadscreen.tex"),
            0xb7a434886d1ce5e6
        );
    }
}
