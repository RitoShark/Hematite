//! RetypeStringToFile — Riot's string → xxh64 `file` asset-reference migration.
//! Matching is per (class, field), NOT per field name: `SkinMeshDataProperties.texture`
//! migrated while `VfxEmitterDefinitionData.texture` stayed string.

use crate::context::FixContext;
use crate::strings::fnv1a_hash;
use hematite_types::bin::{BinTree, PropertyValue, StructValue};
use hematite_types::config::ClassFieldTarget;
use std::collections::{BTreeMap, HashSet};

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

pub fn apply_to_tree(tree: &mut BinTree, targets: &[ClassFieldTarget]) -> u32 {
    let resolved = resolve_targets(targets);
    let BinTree {
        objects,
        trailer_files,
        ..
    } = tree;

    let mut changes = 0u32;
    for obj in objects.values_mut() {
        for prop in obj.properties.values_mut() {
            if resolved.contains(&(obj.class_hash.0, prop.name_hash.0)) {
                changes += convert_in_place(&mut prop.value, trailer_files);
            }
            changes += recurse_apply(&mut prop.value, &resolved, trailer_files);
        }
    }
    changes
}

fn recurse_apply(
    value: &mut PropertyValue,
    targets: &HashSet<(u32, u32)>,
    pairs: &mut BTreeMap<u64, String>,
) -> u32 {
    let mut changes = 0u32;
    match value {
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            changes += apply_struct(s, targets, pairs);
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            for item in items {
                changes += recurse_apply(item, targets, pairs);
            }
        }
        PropertyValue::Optional(inner) => {
            if let Some(v) = inner.as_mut() {
                changes += recurse_apply(v, targets, pairs);
            }
        }
        PropertyValue::Map(entries) => {
            for (_k, v) in entries {
                changes += recurse_apply(v, targets, pairs);
            }
        }
        _ => {}
    }
    changes
}

fn apply_struct(
    s: &mut StructValue,
    targets: &HashSet<(u32, u32)>,
    pairs: &mut BTreeMap<u64, String>,
) -> u32 {
    let mut changes = 0u32;
    for prop in s.properties.values_mut() {
        if targets.contains(&(s.class_hash.0, prop.name_hash.0)) {
            changes += convert_in_place(&mut prop.value, pairs);
        }
        changes += recurse_apply(&mut prop.value, targets, pairs);
    }
    changes
}

// Only string payloads convert; option/list/map wrappers keep their shape and the
// write adapter derives the new element tags (option[string] → option[file]) itself.
fn convert_in_place(value: &mut PropertyValue, pairs: &mut BTreeMap<u64, String>) -> u32 {
    match value {
        PropertyValue::String(s) => {
            let hash = wad_ref_hash(s);
            pairs.insert(hash, std::mem::take(s));
            *value = PropertyValue::WadHash(hash);
            1
        }
        PropertyValue::Optional(inner) => match inner.as_mut() {
            Some(v) => convert_in_place(v, pairs),
            None => 0,
        },
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            items.iter_mut().map(|v| convert_in_place(v, pairs)).sum()
        }
        PropertyValue::Map(entries) => entries
            .iter_mut()
            .map(|(_k, v)| convert_in_place(v, pairs))
            .sum(),
        _ => 0,
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

/// Per-target detection for check mode.
///
/// [`detect`] answers "does this tree need the migration at all", which is everything a
/// fix needs to know. A check needs more: it has to say WHICH property is unmigrated,
/// because the consequence differs per property. An unreadable animation path is a hard
/// crash; an unresolved interface asset merely renders missing. One rule therefore has
/// to be able to report two different severities, and that requires knowing the hit.
///
/// Returns each matching `(class_hash, field_hash)` pair mapped to one example string
/// value, used as the diagnostic's `detail` so the UI can name the offending asset.
pub fn detect_hits(
    tree: &BinTree,
    targets: &[ClassFieldTarget],
) -> BTreeMap<(u32, u32), String> {
    let resolved = resolve_targets(targets);
    let mut hits = BTreeMap::new();
    for obj in tree.objects.values() {
        for prop in obj.properties.values() {
            let key = (obj.class_hash.0, prop.name_hash.0);
            if resolved.contains(&key) {
                if let Some(sample) = first_string(&prop.value) {
                    hits.entry(key).or_insert(sample);
                }
            }
            recurse_hits(&prop.value, &resolved, &mut hits);
        }
    }
    hits
}

fn recurse_hits(
    value: &PropertyValue,
    targets: &HashSet<(u32, u32)>,
    hits: &mut BTreeMap<(u32, u32), String>,
) {
    match value {
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            for p in s.properties.values() {
                let key = (s.class_hash.0, p.name_hash.0);
                if targets.contains(&key) {
                    if let Some(sample) = first_string(&p.value) {
                        hits.entry(key).or_insert(sample);
                    }
                }
                recurse_hits(&p.value, targets, hits);
            }
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            for v in items {
                recurse_hits(v, targets, hits);
            }
        }
        PropertyValue::Optional(inner) => {
            if let Some(v) = inner.as_ref().as_ref() {
                recurse_hits(v, targets, hits);
            }
        }
        PropertyValue::Map(entries) => {
            for (_k, v) in entries {
                recurse_hits(v, targets, hits);
            }
        }
        _ => {}
    }
}

/// First string inside a value, looking through the container shapes the migration uses.
///
/// An OPTION's single value can be a bare scalar rather than a one-element list, so both
/// shapes have to be handled or scalar options read as empty.
fn first_string(value: &PropertyValue) -> Option<String> {
    match value {
        PropertyValue::String(s) => Some(s.clone()),
        PropertyValue::Optional(inner) => inner.as_ref().as_ref().and_then(first_string),
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            items.iter().find_map(first_string)
        }
        PropertyValue::Map(entries) => entries.iter().find_map(|(_k, v)| first_string(v)),
        _ => None,
    }
}

/// Resolve one target to the `(class_hash, field_hash)` key [`detect_hits`] returns, so
/// callers can map a hit back to the config entry that declared it.
pub fn target_key(target: &ClassFieldTarget) -> (u32, u32) {
    (resolve_hash(&target.class), resolve_hash(&target.field))
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
    use hematite_types::bin::{BinObject, BinProperty};
    use hematite_types::hash::{FieldHash, PathHash, TypeHash};
    use indexmap::IndexMap;

    fn target(class: &str, field: &str) -> ClassFieldTarget {
        ClassFieldTarget {
            class: class.to_string(),
            field: field.to_string(),
            reason: None,
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
            tree.trailer_files
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
        assert_eq!(tree.trailer_files.len(), 2);
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
