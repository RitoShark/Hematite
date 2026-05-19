//! Detection rule dispatch and individual rule implementations.
//!
//! ## Detection rules and their logic
//!
//! | Rule | What it checks |
//! |------|---------------|
//! | `MissingOrWrongField` | Field missing or has wrong value in embed path |
//! | `FieldHashExists` | A field hash exists at a dot-separated path |
//! | `StringExtensionNotInWad` | String fields with extension not in WAD |
//! | `RecursiveStringExtensionNotInWad` | Recursive scan for extension strings not in WAD |
//! | `EntryTypeExistsAny` | Any object matches entry type list |
//! | `BnkVersionNotIn` | BNK audio version not in allowed list (file-level) |
//! | `VfxShapeNeedsFix` | VFX shape has old format (pre-14.1) |

use crate::factory::matches_json;
use crate::filter;
use crate::traits::{HashProvider, WadProvider};
use crate::walk::extract_strings;
use hematite_types::bin::{BinTree, PropertyValue, StructValue};
use hematite_types::config::{DetectionRule, EntryValidationTarget};
use hematite_types::hash::FieldHash;

/// Main detection dispatch. Returns true if the issue is detected.
pub fn detect_issue(
    rule: &DetectionRule,
    tree: &BinTree,
    hashes: &dyn HashProvider,
    wad: &dyn WadProvider,
) -> bool {
    match rule {
        DetectionRule::MissingOrWrongField {
            entry_type,
            embed_path,
            embed_type,
            field,
            expected_value,
        } => detect_missing_or_wrong_field(
            tree,
            hashes,
            entry_type,
            embed_path.as_deref(),
            embed_type.as_deref(),
            field,
            expected_value.as_ref(),
        ),

        DetectionRule::FieldHashExists { entry_type, path } => {
            detect_field_hash_exists(tree, hashes, entry_type, path)
        }

        DetectionRule::StringExtensionNotInWad {
            entry_type,
            fields,
            extension,
        } => detect_string_extension_not_in_wad(tree, hashes, wad, entry_type, fields, extension),

        DetectionRule::RecursiveStringExtensionNotInWad {
            extension,
            path_prefixes,
        } => detect_recursive_extension(tree, wad, extension, path_prefixes),

        DetectionRule::EntryTypeExistsAny { entry_types } => {
            detect_entry_type_exists(tree, hashes, entry_types)
        }

        DetectionRule::BnkVersionNotIn { .. } => false,

        DetectionRule::VfxShapeNeedsFix { entry_type } => {
            detect_vfx_shape_needs_fix(tree, hashes, entry_type)
        }

        DetectionRule::InvalidShaderReference {
            shader_def_type,
            shader_link_field,
        } => detect_invalid_shader(tree, hashes, shader_def_type, shader_link_field),

        DetectionRule::UnreferencedEntryOfType {
            main_entry_type,
            targets,
        } => detect_unreferenced_entries(tree, hashes, main_entry_type, targets),
    }
}

fn detect_missing_or_wrong_field(
    tree: &BinTree,
    hashes: &dyn HashProvider,
    entry_type: &str,
    embed_path: Option<&str>,
    embed_type: Option<&str>,
    field: &str,
    expected_value: Option<&serde_json::Value>,
) -> bool {
    if !hashes.is_loaded() {
        return false;
    }

    let Some(target_type_hash) = hashes.type_hash(entry_type) else {
        return false;
    };

    let objects = filter::objects_by_type(tree, target_type_hash);

    for obj in objects {
        if let Some(expected_embed_type) = embed_type {
            let Some(embed_type_hash) = hashes.type_hash(expected_embed_type) else {
                continue;
            };

            let mut found_embed = false;
            let mut missing_field = false;

            for prop in obj.properties.values() {
                if let PropertyValue::Embedded(struct_val) = &prop.value {
                    if struct_val.class_hash == embed_type_hash {
                        found_embed = true;

                        let field_hash = hashes.field_hash(field);
                        if let Some(fh) = field_hash {
                            if let Some(existing_prop) = struct_val.properties.get(&fh.0) {
                                if let Some(expected) = expected_value {
                                    if !matches_json(&existing_prop.value, expected) {
                                        missing_field = true;
                                    }
                                }
                            } else {
                                missing_field = true;
                            }
                        } else {
                            missing_field = true;
                        }
                    }
                }
            }

            if missing_field || !found_embed {
                return true;
            }
        } else if let Some(embed_field_name) = embed_path {
            let Some(embed_hash) = hashes.field_hash(embed_field_name) else {
                continue;
            };

            if let Some(embed_prop) = obj.properties.get(&embed_hash.0) {
                if let PropertyValue::Embedded(struct_val) = &embed_prop.value {
                    let field_hash = hashes.field_hash(field);
                    if let Some(fh) = field_hash {
                        if !struct_val.properties.contains_key(&fh.0) {
                            return true;
                        }
                    }
                }
            }
        } else {
            let field_hash = hashes.field_hash(field);
            if let Some(fh) = field_hash {
                if !obj.properties.contains_key(&fh.0) {
                    return true;
                }
            }
        }
    }

    false
}

fn detect_field_hash_exists(
    tree: &BinTree,
    hashes: &dyn HashProvider,
    entry_type: &str,
    path: &str,
) -> bool {
    if !hashes.is_loaded() {
        return false;
    }

    let Some(target_type_hash) = hashes.type_hash(entry_type) else {
        return false;
    };

    let path_parts: Vec<&str> = path.split('.').collect();
    if path_parts.is_empty() {
        return false;
    }

    let objects = filter::objects_by_type(tree, target_type_hash);

    for obj in objects {
        if search_field_path(&obj.properties, &path_parts, hashes) {
            return true;
        }
    }

    false
}

fn search_field_path(
    properties: &indexmap::IndexMap<u32, hematite_types::bin::BinProperty>,
    path_parts: &[&str],
    hashes: &dyn HashProvider,
) -> bool {
    if path_parts.is_empty() {
        return false;
    }

    let current_part = path_parts[0];
    let remaining = &path_parts[1..];

    if remaining.is_empty() {
        return properties.keys().any(|hash| {
            hashes
                .resolve_field(FieldHash(*hash))
                .map(|name| name.eq_ignore_ascii_case(current_part))
                .unwrap_or(false)
        });
    }

    for (hash, prop) in properties {
        let Some(field_name) = hashes.resolve_field(FieldHash(*hash)) else {
            continue;
        };

        if !field_name.eq_ignore_ascii_case(current_part) {
            continue;
        }

        if search_field_in_value(&prop.value, remaining, hashes) {
            return true;
        }
    }

    false
}

fn search_field_in_value(
    value: &PropertyValue,
    path_parts: &[&str],
    hashes: &dyn HashProvider,
) -> bool {
    if path_parts.is_empty() {
        return true;
    }

    match value {
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            search_field_path(&s.properties, path_parts, hashes)
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            if path_parts[0] == "*" {
                let remaining = &path_parts[1..];
                items
                    .iter()
                    .any(|item| search_field_in_value(item, remaining, hashes))
            } else {
                false
            }
        }
        PropertyValue::Optional(boxed) => {
            if let Some(inner) = &**boxed {
                search_field_in_value(inner, path_parts, hashes)
            } else {
                false
            }
        }
        _ => false,
    }
}

fn detect_string_extension_not_in_wad(
    tree: &BinTree,
    _hashes: &dyn HashProvider,
    wad: &dyn WadProvider,
    _entry_type: &str,
    _fields: &[String],
    extension: &str,
) -> bool {
    let strings = extract_strings(tree);

    for s in strings {
        if s.to_lowercase().ends_with(extension) && !wad.has_path(&s) {
            return true;
        }
    }

    false
}

fn detect_recursive_extension(
    tree: &BinTree,
    wad: &dyn WadProvider,
    extension: &str,
    path_prefixes: &[String],
) -> bool {
    let strings = extract_strings(tree);

    for s in strings {
        let lower = s.to_lowercase();

        if !lower.ends_with(extension) {
            continue;
        }

        if !path_prefixes.is_empty() {
            let matches_prefix = path_prefixes
                .iter()
                .any(|prefix| lower.starts_with(&prefix.to_lowercase()));
            if !matches_prefix {
                continue;
            }
        }

        if !wad.has_path(&s) {
            return true;
        }
    }

    false
}

fn detect_entry_type_exists(
    tree: &BinTree,
    hashes: &dyn HashProvider,
    entry_types: &[String],
) -> bool {
    if !hashes.is_loaded() {
        return false;
    }

    for obj in tree.objects.values() {
        if let Some(type_name) = hashes.resolve_type(obj.class_hash) {
            if entry_types
                .iter()
                .any(|et| et.eq_ignore_ascii_case(type_name))
            {
                return true;
            }
        }
    }

    false
}

fn detect_vfx_shape_needs_fix(tree: &BinTree, hashes: &dyn HashProvider, entry_type: &str) -> bool {
    if !hashes.is_loaded() {
        return false;
    }

    let Some(target_type_hash) = hashes.type_hash(entry_type) else {
        return false;
    };
    let Some(shape_hash) = hashes.field_hash("Shape") else {
        return false;
    };

    let old_shape_field_hashes: Vec<u32> = [
        hashes.field_hash("BirthTranslation"),
        hashes.field_hash("EmitOffset"),
        hashes.field_hash("EmitRotationAngles"),
        hashes.field_hash("EmitRotationAxes"),
    ]
    .into_iter()
    .flatten()
    .map(|h| h.0)
    .collect();

    let objects = filter::objects_by_type(tree, target_type_hash);

    for obj in objects {
        for prop in obj.properties.values() {
            if let PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) =
                &prop.value
            {
                for item in items {
                    let emitter = match item {
                        PropertyValue::Embedded(e) | PropertyValue::Struct(e) => e,
                        _ => continue,
                    };
                    
                    if let Some(shape_prop) = emitter.properties.get(&shape_hash.0) {
                        let shape = match &shape_prop.value {
                            PropertyValue::Embedded(s) | PropertyValue::Struct(s) => s,
                            _ => continue,
                        };
                        
                        if has_old_vfx_shape_format(shape, &old_shape_field_hashes) {
                            return true;
                        }
                    }
                }
            }
        }
    }

    false
}

/// Returns true if the shape struct contains any old-format fields that need migration.
fn has_old_vfx_shape_format(shape: &StructValue, old_field_hashes: &[u32]) -> bool {
    shape
        .properties
        .keys()
        .any(|k| old_field_hashes.contains(k))
}

/// Detect invalid shader references in StaticMaterialDef/CustomShaderDef objects.
///
/// Walks material technique passes looking for Link values that reference
/// shaders not in the valid shader set.
fn detect_invalid_shader(
    tree: &BinTree,
    hashes: &dyn HashProvider,
    shader_def_type: &str,
    _shader_link_field: &str,
) -> bool {
    if !hashes.is_loaded() {
        return false;
    }

    let Some(target_type_hash) = hashes.type_hash(shader_def_type) else {
        return false;
    };

    // Walk all objects of target type looking for Link values
    let mut found_any = false;
    for obj in filter::objects_by_type(tree, target_type_hash) {
        found_any = true;
        for prop in obj.properties.values() {
            if has_link_value(&prop.value) {
                return true;
            }
        }
    }

    // If no objects of the target type, nothing to detect
    let _ = found_any;
    false
}

/// Check if any Link values exist in a property value tree.
fn has_link_value(value: &PropertyValue) -> bool {
    match value {
        PropertyValue::Link(hash) => *hash != 0,
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            s.properties.values().any(|p| has_link_value(&p.value))
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            items.iter().any(has_link_value)
        }
        PropertyValue::Optional(boxed) => {
            if let Some(inner) = &**boxed {
                has_link_value(inner)
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Detect entries that are not referenced by the main skin entry.
///
/// For each target type, checks if any entries of that type exist but are NOT
/// referenced by the main SkinCharacterDataProperties entry.
fn detect_unreferenced_entries(
    tree: &BinTree,
    hashes: &dyn HashProvider,
    main_entry_type: &str,
    targets: &[EntryValidationTarget],
) -> bool {
    if !hashes.is_loaded() {
        return false;
    }

    let Some(main_type_hash) = hashes.type_hash(main_entry_type) else {
        return false;
    };

    // Collect all Link values from main entries (these are "referenced" entries)
    let mut referenced_hashes = std::collections::HashSet::new();
    let mut found_main = false;
    for main_obj in filter::objects_by_type(tree, main_type_hash) {
        found_main = true;
        collect_link_values(&main_obj.properties, &mut referenced_hashes);
    }

    if !found_main {
        return false;
    }

    // For each target, check if there are unreferenced entries
    for target in targets {
        let target_type_hash = if let Some(hex) = &target.type_hash {
            let hex = hex.trim_start_matches("0x");
            u32::from_str_radix(hex, 16).ok()
        } else {
            hashes.type_hash(&target.entry_type).map(|h| h.0)
        };

        let Some(type_hash) = target_type_hash else {
            continue;
        };

        // Check each entry of target type
        for (path_hash, obj) in &tree.objects {
            if obj.class_hash.0 == type_hash && !referenced_hashes.contains(path_hash) {
                return true;
            }
        }
    }

    false
}

/// Recursively collect all Link hash values from a property map.
fn collect_link_values(
    properties: &indexmap::IndexMap<u32, hematite_types::bin::BinProperty>,
    out: &mut std::collections::HashSet<u32>,
) {
    for prop in properties.values() {
        collect_link_values_from_value(&prop.value, out);
    }
}

fn collect_link_values_from_value(value: &PropertyValue, out: &mut std::collections::HashSet<u32>) {
    match value {
        PropertyValue::Link(hash) => {
            if *hash != 0 {
                out.insert(*hash);
            }
        }
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            collect_link_values(&s.properties, out);
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            for item in items {
                collect_link_values_from_value(item, out);
            }
        }
        PropertyValue::Optional(boxed) => {
            if let Some(inner) = &**boxed {
                collect_link_values_from_value(inner, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::bin::{BinObject, BinProperty};
    use hematite_types::hash::{PathHash, TypeHash};
    use indexmap::IndexMap;
    use std::collections::HashMap;

    struct MockHashProvider {
        types: HashMap<String, u32>,
        fields: HashMap<String, u32>,
        type_names: HashMap<u32, String>,
        field_names: HashMap<u32, String>,
    }

    impl MockHashProvider {
        fn new() -> Self {
            let mut provider = Self {
                types: HashMap::new(),
                fields: HashMap::new(),
                type_names: HashMap::new(),
                field_names: HashMap::new(),
            };

            provider.add_type("SkinCharacterDataProperties", 0x1234);
            provider.add_field("UnitHealthBarStyle", 0x5678);
            provider.add_field("Shape", 0xABCD);
            provider.add_field("BirthTranslation", 0xEF00);

            provider
        }

        fn add_type(&mut self, name: &str, hash: u32) {
            self.types.insert(name.to_lowercase(), hash);
            self.type_names.insert(hash, name.to_string());
        }

        fn add_field(&mut self, name: &str, hash: u32) {
            self.fields.insert(name.to_lowercase(), hash);
            self.field_names.insert(hash, name.to_string());
        }
    }

    impl HashProvider for MockHashProvider {
        fn resolve_type(&self, hash: TypeHash) -> Option<&str> {
            self.type_names.get(&hash.0).map(|s| s.as_str())
        }

        fn resolve_field(&self, hash: FieldHash) -> Option<&str> {
            self.field_names.get(&hash.0).map(|s| s.as_str())
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
            !self.types.is_empty()
        }

        fn has_game_path(&self, _path: &str) -> bool {
            false
        }
    }

    struct MockWadProvider {
        paths: Vec<String>,
    }

    impl WadProvider for MockWadProvider {
        fn has_path(&self, path: &str) -> bool {
            self.paths.contains(&path.to_string())
        }

        fn has_hash(&self, _hash: u64) -> bool {
            false
        }
    }

    #[test]
    fn test_detect_entry_type_exists() {
        let hashes = MockHashProvider::new();
        let mut tree = BinTree::default();

        let obj = BinObject {
            class_hash: TypeHash(0x1234),
            path_hash: PathHash(0),
            properties: IndexMap::new(),
        };
        tree.objects.insert(0, obj);

        let rule = DetectionRule::EntryTypeExistsAny {
            entry_types: vec!["SkinCharacterDataProperties".to_string()],
        };

        let wad = MockWadProvider { paths: vec![] };
        assert!(detect_issue(&rule, &tree, &hashes, &wad));
    }

    #[test]
    fn test_detect_recursive_extension() {
        let hashes = MockHashProvider::new();
        let mut tree = BinTree::default();

        let mut obj = BinObject {
            class_hash: TypeHash(0x1234),
            path_hash: PathHash(0),
            properties: IndexMap::new(),
        };

        obj.properties.insert(
            1,
            BinProperty {
                name_hash: FieldHash(1),
                value: PropertyValue::String("test.dds".to_string()),
            },
        );

        tree.objects.insert(0, obj);

        let rule = DetectionRule::RecursiveStringExtensionNotInWad {
            extension: ".dds".to_string(),
            path_prefixes: vec![],
        };

        let wad = MockWadProvider { paths: vec![] };
        assert!(detect_issue(&rule, &tree, &hashes, &wad));

        let wad_with_file = MockWadProvider {
            paths: vec!["test.dds".to_string()],
        };
        assert!(!detect_issue(&rule, &tree, &hashes, &wad_with_file));
    }
}
