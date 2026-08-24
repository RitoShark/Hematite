//! Visitor pattern for recursive BIN tree traversal.
//!
//! This module provides a single reusable walker that replaces 6 separate recursive
//! implementations from the old codebase. Fix modules implement [`PropertyVisitor`]
//! and let the walker handle traversal logic.

use hematite_types::bin::{BinObject, BinTree, PropertyValue, StructValue};
use hematite_types::hash::FieldHash;

/// Result of visiting a string value.
pub enum VisitResult {
    /// Don't change anything.
    Skip,
    /// Replace the string with a new value.
    Mutate(String),
}

/// Visitor trait for property tree traversal.
///
/// Implement only the methods you need — defaults are no-ops.
/// The walker calls these as it recurses through the property tree.
#[allow(unused_variables)]
pub trait PropertyVisitor {
    /// Called for each string value found.
    /// Return `VisitResult::Mutate(new)` to replace the string.
    fn visit_string(&mut self, value: &str, field_hash: FieldHash) -> VisitResult {
        VisitResult::Skip
    }

    /// Called for each field hash encountered.
    /// Return `Some(new_hash)` to rename the field.
    fn visit_field_hash(&mut self, hash: FieldHash) -> Option<FieldHash> {
        None
    }

    /// Called when entering an embedded/struct.
    /// Return false to skip its children.
    fn enter_struct(&mut self, class_hash: u32) -> bool {
        true
    }
}

/// Walk all properties in a BinObject, calling visitor methods.
/// Returns the number of mutations applied.
pub fn walk_object(obj: &mut BinObject, visitor: &mut dyn PropertyVisitor) -> u32 {
    let mut mutations = 0;
    let mut renames = Vec::new();

    for (field_hash, prop) in obj.properties.iter_mut() {
        if let Some(new_hash) = visitor.visit_field_hash(FieldHash(*field_hash)) {
            renames.push((*field_hash, new_hash.0));
            mutations += 1;
        }

        mutations += walk_value(&mut prop.value, *field_hash, visitor);
    }

    for (old_hash, new_hash) in renames {
        if let Some(mut prop) = obj.properties.swap_remove(&old_hash) {
            prop.name_hash = FieldHash(new_hash);
            obj.properties.insert(new_hash, prop);
        }
    }

    mutations
}

/// Walk all objects in a BinTree.
/// Returns the total number of mutations applied.
pub fn walk_tree(tree: &mut BinTree, visitor: &mut dyn PropertyVisitor) -> u32 {
    tree.objects
        .values_mut()
        .map(|obj| walk_object(obj, visitor))
        .sum()
}

fn walk_value(
    value: &mut PropertyValue,
    field_hash: u32,
    visitor: &mut dyn PropertyVisitor,
) -> u32 {
    use PropertyValue::*;
    let mut mutations = 0;

    match value {
        String(s) => {
            if let VisitResult::Mutate(new_val) = visitor.visit_string(s, FieldHash(field_hash)) {
                *s = new_val;
                mutations += 1;
            }
        }

        Struct(struct_val) | Embedded(struct_val) => {
            if visitor.enter_struct(struct_val.class_hash.0) {
                mutations += walk_struct(struct_val, visitor);
            }
        }

        Container(items) | UnorderedContainer(items) => {
            for item in items.iter_mut() {
                mutations += walk_value(item, field_hash, visitor);
            }
        }

        Optional(boxed) => {
            if let Some(ref mut inner) = **boxed {
                mutations += walk_value(inner, field_hash, visitor);
            }
        }

        Map(entries) => {
            for (key, val) in entries.iter_mut() {
                mutations += walk_value(key, field_hash, visitor);
                mutations += walk_value(val, field_hash, visitor);
            }
        }

        Bool(_) | I8(_) | U8(_) | I16(_) | U16(_) | I32(_) | U32(_) | I64(_) | U64(_) | F32(_)
        | Vector2(_) | Vector3(_) | Vector4(_) | Matrix4x4(_) | Hash(_) | WadHash(_) | Link(_)
        | Color(_) | BitBool(_) => {}
    }

    mutations
}

fn walk_struct(struct_val: &mut StructValue, visitor: &mut dyn PropertyVisitor) -> u32 {
    let mut mutations = 0;
    let mut renames = Vec::new();

    for (field_hash, prop) in struct_val.properties.iter_mut() {
        if let Some(new_hash) = visitor.visit_field_hash(FieldHash(*field_hash)) {
            renames.push((*field_hash, new_hash.0));
            mutations += 1;
        }

        mutations += walk_value(&mut prop.value, *field_hash, visitor);
    }

    for (old_hash, new_hash) in renames {
        if let Some(mut prop) = struct_val.properties.swap_remove(&old_hash) {
            prop.name_hash = FieldHash(new_hash);
            struct_val.properties.insert(new_hash, prop);
        }
    }

    mutations
}

/// Every string value in a BIN tree, borrowed.
///
/// This used to deep-clone the entire tree purely to satisfy [`walk_tree`]'s `&mut`
/// signature, then copy every string out of the clone and throw the clone away. Several
/// rules call it once per BIN, so a mod with hundreds of BINs paid for hundreds of whole
/// tree copies. Measured, the clone and the string copies cost roughly ten times the
/// traversal they existed to perform.
pub fn string_refs(tree: &BinTree) -> Vec<&str> {
    let mut out = Vec::new();
    for obj in tree.objects.values() {
        for prop in obj.properties.values() {
            collect_strings(&prop.value, &mut out);
        }
    }
    out
}

fn collect_strings<'a>(value: &'a PropertyValue, out: &mut Vec<&'a str>) {
    match value {
        PropertyValue::String(s) => out.push(s.as_str()),
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            for prop in s.properties.values() {
                collect_strings(&prop.value, out);
            }
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            for v in items {
                collect_strings(v, out);
            }
        }
        PropertyValue::Optional(inner) => {
            if let Some(v) = inner.as_ref().as_ref() {
                collect_strings(v, out);
            }
        }
        PropertyValue::Map(entries) => {
            for (k, v) in entries {
                collect_strings(k, out);
                collect_strings(v, out);
            }
        }
        _ => {}
    }
}

/// Every string value paired with the field hash it is stored under.
///
/// Only a string that is a property's own value is paired. A string sitting loose inside a
/// container has no field of its own: it inherits the container's, which would make an
/// arbitrary list of paths look like repeated instances of that one field. Nested
/// properties, including ones several container levels down inside map placeables, do get
/// their own hash and are reported.
pub fn string_fields(tree: &BinTree) -> Vec<(u32, &str)> {
    let mut out = Vec::new();
    for obj in tree.objects.values() {
        for (field_hash, prop) in obj.properties.iter() {
            collect_string_fields(*field_hash, &prop.value, &mut out);
        }
    }
    out
}

fn collect_string_fields<'a>(field: u32, value: &'a PropertyValue, out: &mut Vec<(u32, &'a str)>) {
    match value {
        PropertyValue::String(s) => out.push((field, s.as_str())),
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            for (inner, prop) in s.properties.iter() {
                collect_string_fields(*inner, &prop.value, out);
            }
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            for v in items {
                // The item carries no field of its own, so a bare string here is skipped;
                // a struct item's own properties are reported under their own hashes.
                if !matches!(v, PropertyValue::String(_)) {
                    collect_string_fields(field, v, out);
                }
            }
        }
        PropertyValue::Optional(inner) => {
            if let Some(v) = inner.as_ref().as_ref() {
                collect_string_fields(field, v, out);
            }
        }
        PropertyValue::Map(entries) => {
            for (_k, v) in entries {
                if !matches!(v, PropertyValue::String(_)) {
                    collect_string_fields(field, v, out);
                }
            }
        }
        _ => {}
    }
}

/// Owned copy of every string value. Prefer [`string_refs`] unless ownership is needed.
pub fn extract_strings(tree: &BinTree) -> Vec<String> {
    string_refs(tree).into_iter().map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::bin::BinProperty;
    use hematite_types::hash::{PathHash, TypeHash};
    use indexmap::IndexMap;

    #[test]
    fn test_walk_object_visit_strings() {
        struct TestVisitor {
            count: u32,
        }

        impl PropertyVisitor for TestVisitor {
            fn visit_string(&mut self, _value: &str, _hash: FieldHash) -> VisitResult {
                self.count += 1;
                VisitResult::Skip
            }
        }

        let mut obj = BinObject {
            class_hash: TypeHash(0x12345678),
            path_hash: PathHash(0x11111111),
            properties: IndexMap::new(),
        };

        obj.properties.insert(
            0x1,
            BinProperty {
                name_hash: FieldHash(0x1),
                value: PropertyValue::String("test1.tex".to_string()),
            },
        );

        obj.properties.insert(
            0x2,
            BinProperty {
                name_hash: FieldHash(0x2),
                value: PropertyValue::String("test2.skn".to_string()),
            },
        );

        let mut visitor = TestVisitor { count: 0 };
        walk_object(&mut obj, &mut visitor);

        assert_eq!(visitor.count, 2);
    }

    #[test]
    fn test_walk_object_mutate_strings() {
        struct ReplaceVisitor;

        impl PropertyVisitor for ReplaceVisitor {
            fn visit_string(&mut self, value: &str, _hash: FieldHash) -> VisitResult {
                if value.ends_with(".tex") {
                    VisitResult::Mutate(value.replace(".tex", ".dds"))
                } else {
                    VisitResult::Skip
                }
            }
        }

        let mut obj = BinObject {
            class_hash: TypeHash(0x12345678),
            path_hash: PathHash(0x11111111),
            properties: IndexMap::new(),
        };

        obj.properties.insert(
            0x1,
            BinProperty {
                name_hash: FieldHash(0x1),
                value: PropertyValue::String("test.tex".to_string()),
            },
        );

        let mut visitor = ReplaceVisitor;
        let mutations = walk_object(&mut obj, &mut visitor);

        assert_eq!(mutations, 1);
        if let PropertyValue::String(s) = &obj.properties[&0x1].value {
            assert_eq!(s, "test.dds");
        } else {
            panic!("Expected String value");
        }
    }

    #[test]
    fn test_walk_object_rename_hash() {
        struct RenameVisitor {
            from: u32,
            to: u32,
        }

        impl PropertyVisitor for RenameVisitor {
            fn visit_field_hash(&mut self, hash: FieldHash) -> Option<FieldHash> {
                if hash.0 == self.from {
                    Some(FieldHash(self.to))
                } else {
                    None
                }
            }
        }

        let mut obj = BinObject {
            class_hash: TypeHash(0x12345678),
            path_hash: PathHash(0x11111111),
            properties: IndexMap::new(),
        };

        obj.properties.insert(
            0xABCD,
            BinProperty {
                name_hash: FieldHash(0xABCD),
                value: PropertyValue::U32(42),
            },
        );

        let mut visitor = RenameVisitor {
            from: 0xABCD,
            to: 0x1234,
        };
        let mutations = walk_object(&mut obj, &mut visitor);

        assert_eq!(mutations, 1);
        assert!(obj.properties.contains_key(&0x1234));
        assert!(!obj.properties.contains_key(&0xABCD));
        assert_eq!(obj.properties[&0x1234].name_hash.0, 0x1234);
    }

    #[test]
    fn test_walk_nested_struct() {
        struct CountVisitor {
            strings: u32,
        }

        impl PropertyVisitor for CountVisitor {
            fn visit_string(&mut self, _value: &str, _hash: FieldHash) -> VisitResult {
                self.strings += 1;
                VisitResult::Skip
            }
        }

        let mut inner_props = IndexMap::new();
        inner_props.insert(
            0x1,
            BinProperty {
                name_hash: FieldHash(0x1),
                value: PropertyValue::String("inner.tex".to_string()),
            },
        );

        let struct_val = StructValue {
            class_hash: TypeHash(0x99999999),
            properties: inner_props,
        };

        let mut obj = BinObject {
            class_hash: TypeHash(0x12345678),
            path_hash: PathHash(0x11111111),
            properties: IndexMap::new(),
        };

        obj.properties.insert(
            0x1,
            BinProperty {
                name_hash: FieldHash(0x1),
                value: PropertyValue::String("outer.tex".to_string()),
            },
        );

        obj.properties.insert(
            0x2,
            BinProperty {
                name_hash: FieldHash(0x2),
                value: PropertyValue::Embedded(struct_val),
            },
        );

        let mut visitor = CountVisitor { strings: 0 };
        walk_object(&mut obj, &mut visitor);

        assert_eq!(visitor.strings, 2); // outer + inner
    }

    #[test]
    fn test_walk_container() {
        struct CountVisitor {
            strings: u32,
        }

        impl PropertyVisitor for CountVisitor {
            fn visit_string(&mut self, _value: &str, _hash: FieldHash) -> VisitResult {
                self.strings += 1;
                VisitResult::Skip
            }
        }

        let mut obj = BinObject {
            class_hash: TypeHash(0x12345678),
            path_hash: PathHash(0x11111111),
            properties: IndexMap::new(),
        };

        obj.properties.insert(
            0x1,
            BinProperty {
                name_hash: FieldHash(0x1),
                value: PropertyValue::Container(vec![
                    PropertyValue::String("item1.tex".to_string()),
                    PropertyValue::String("item2.tex".to_string()),
                    PropertyValue::String("item3.tex".to_string()),
                ]),
            },
        );

        let mut visitor = CountVisitor { strings: 0 };
        walk_object(&mut obj, &mut visitor);

        assert_eq!(visitor.strings, 3);
    }

    #[test]
    fn test_extract_strings() {
        let mut tree = BinTree::default();

        let mut obj = BinObject {
            class_hash: TypeHash(0x12345678),
            path_hash: PathHash(0x11111111),
            properties: IndexMap::new(),
        };

        obj.properties.insert(
            0x1,
            BinProperty {
                name_hash: FieldHash(0x1),
                value: PropertyValue::String("test1.tex".to_string()),
            },
        );

        obj.properties.insert(
            0x2,
            BinProperty {
                name_hash: FieldHash(0x2),
                value: PropertyValue::String("test2.skn".to_string()),
            },
        );

        tree.objects.insert(0x11111111, obj);

        let strings = extract_strings(&tree);
        assert_eq!(strings.len(), 2);
        assert!(strings.contains(&"test1.tex".to_string()));
        assert!(strings.contains(&"test2.skn".to_string()));
    }
}
