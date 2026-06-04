//! rs_bin ↔ Hematite type conversion.
//!
//! When the file-format crate changes its API, only this file needs updating.

use anyhow::{bail, Result};
use hematite_types::bin::{BinObject, BinProperty, BinTree, PropertyValue, StructValue};
use hematite_types::hash::{FieldHash, PathHash, TypeHash};
use indexmap::IndexMap;
use rs_bin::{Bin as RsBin, BinEntry, BinType, BinValue};

/// Convert an rs_bin `Bin` to a Hematite `BinTree` (after parsing).
pub fn ltk_tree_to_hematite(rs_bin: RsBin) -> Result<BinTree> {
    let mut objects = IndexMap::new();

    for entry in rs_bin.entries {
        let obj = entry_to_hematite(entry)?;
        objects.insert(obj.path_hash.0, obj);
    }

    Ok(BinTree {
        objects,
        linked: rs_bin.linked,
    })
}

/// Convert a Hematite `BinTree` to an rs_bin `Bin` (before writing).
pub fn hematite_tree_to_ltk(tree: &BinTree) -> Result<RsBin> {
    let mut bin = RsBin::new();
    bin.linked = tree.linked.clone();

    for obj in tree.objects.values() {
        bin.entries.push(hematite_object_to_entry(obj)?);
    }

    Ok(bin)
}

/// Convert a single rs_bin `BinEntry` to a Hematite `BinObject`.
fn entry_to_hematite(entry: BinEntry) -> Result<BinObject> {
    let mut properties = IndexMap::new();

    for (name_hash, value) in entry.fields {
        let prop = BinProperty {
            name_hash: FieldHash(name_hash),
            value: ltk_value_to_hematite(&value)?,
        };
        properties.insert(name_hash, prop);
    }

    Ok(BinObject {
        class_hash: TypeHash(entry.class_hash),
        path_hash: PathHash(entry.path_hash),
        properties,
    })
}

/// Convert a single Hematite `BinObject` to an rs_bin `BinEntry`.
fn hematite_object_to_entry(obj: &BinObject) -> Result<BinEntry> {
    let mut fields = IndexMap::new();

    for (name_hash, prop) in &obj.properties {
        fields.insert(*name_hash, hematite_value_to_ltk(&prop.value)?);
    }

    Ok(BinEntry {
        path_hash: obj.path_hash.0,
        class_hash: obj.class_hash.0,
        fields,
    })
}

/// Flatten a row-major 4x4 matrix into the 16-element layout rs_bin stores.
fn flatten_mtx(m: &[[f32; 4]; 4]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for (row, chunk) in m.iter().zip(out.chunks_exact_mut(4)) {
        chunk.copy_from_slice(row);
    }
    out
}

/// Unflatten rs_bin's 16-element matrix back into a row-major 4x4 matrix.
fn unflatten_mtx(m: &[f32; 16]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for (row, chunk) in out.iter_mut().zip(m.chunks_exact(4)) {
        row.copy_from_slice(chunk);
    }
    out
}

/// Convert an rs_bin `BinValue` to a Hematite `PropertyValue`.
pub fn ltk_value_to_hematite(val: &BinValue) -> Result<PropertyValue> {
    let out = match val {
        // hematite's PropertyValue has no None variant; LTK errored on it, so we do too.
        BinValue::None => bail!("None value encountered in BIN"),

        // Primitives
        BinValue::Bool(v) => PropertyValue::Bool(*v),
        BinValue::I8(v) => PropertyValue::I8(*v),
        BinValue::U8(v) => PropertyValue::U8(*v),
        BinValue::I16(v) => PropertyValue::I16(*v),
        BinValue::U16(v) => PropertyValue::U16(*v),
        BinValue::I32(v) => PropertyValue::I32(*v),
        BinValue::U32(v) => PropertyValue::U32(*v),
        BinValue::I64(v) => PropertyValue::I64(*v),
        BinValue::U64(v) => PropertyValue::U64(*v),
        BinValue::F32(v) => PropertyValue::F32(*v),

        // Vectors / matrix
        BinValue::Vec2(v) => PropertyValue::Vector2(*v),
        BinValue::Vec3(v) => PropertyValue::Vector3(*v),
        BinValue::Vec4(v) => PropertyValue::Vector4(*v),
        BinValue::Mtx44(v) => PropertyValue::Matrix4x4(unflatten_mtx(v)),

        // Strings & hashes
        BinValue::String(v) => PropertyValue::String(v.clone()),
        BinValue::Hash(v) => PropertyValue::Hash(*v),
        BinValue::File(v) => PropertyValue::WadHash(*v),
        BinValue::Link(v) => PropertyValue::Link(*v),
        BinValue::Rgba(v) => PropertyValue::Color(*v),
        BinValue::Flag(b) => PropertyValue::BitBool(*b as u8),

        // Nested structures
        BinValue::Pointer { class, fields } => {
            PropertyValue::Struct(struct_to_hematite(*class, fields)?)
        }
        BinValue::Embed { class, fields } => {
            PropertyValue::Embedded(struct_to_hematite(*class, fields)?)
        }

        // Collections
        BinValue::List { is_list2, items, .. } => {
            let mut vec = Vec::with_capacity(items.len());
            for item in items {
                vec.push(ltk_value_to_hematite(item)?);
            }
            if *is_list2 {
                PropertyValue::UnorderedContainer(vec)
            } else {
                PropertyValue::Container(vec)
            }
        }

        // Optional
        BinValue::Option { value, .. } => {
            let opt = match value {
                Some(inner) => Some(ltk_value_to_hematite(inner)?),
                None => None,
            };
            PropertyValue::Optional(Box::new(opt))
        }

        // Map
        BinValue::Map { entries, .. } => {
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                pairs.push((ltk_value_to_hematite(k)?, ltk_value_to_hematite(v)?));
            }
            PropertyValue::Map(pairs)
        }
    };

    Ok(out)
}

/// Convert a Hematite `PropertyValue` to an rs_bin `BinValue`.
pub fn hematite_value_to_ltk(val: &PropertyValue) -> Result<BinValue> {
    let out = match val {
        // Primitives
        PropertyValue::Bool(v) => BinValue::Bool(*v),
        PropertyValue::I8(v) => BinValue::I8(*v),
        PropertyValue::U8(v) => BinValue::U8(*v),
        PropertyValue::I16(v) => BinValue::I16(*v),
        PropertyValue::U16(v) => BinValue::U16(*v),
        PropertyValue::I32(v) => BinValue::I32(*v),
        PropertyValue::U32(v) => BinValue::U32(*v),
        PropertyValue::I64(v) => BinValue::I64(*v),
        PropertyValue::U64(v) => BinValue::U64(*v),
        PropertyValue::F32(v) => BinValue::F32(*v),

        // Vectors / matrix
        PropertyValue::Vector2(v) => BinValue::Vec2(*v),
        PropertyValue::Vector3(v) => BinValue::Vec3(*v),
        PropertyValue::Vector4(v) => BinValue::Vec4(*v),
        PropertyValue::Matrix4x4(v) => BinValue::Mtx44(flatten_mtx(v)),

        // Strings & hashes
        PropertyValue::String(v) => BinValue::String(v.clone()),
        PropertyValue::Hash(v) => BinValue::Hash(*v),
        PropertyValue::Link(v) => BinValue::Link(*v),
        PropertyValue::WadHash(v) => BinValue::File(*v),
        PropertyValue::Color(rgba) => BinValue::Rgba(*rgba),
        PropertyValue::BitBool(v) => BinValue::Flag(*v != 0),

        // Nested structures
        PropertyValue::Struct(s) => {
            let (class, fields) = hematite_struct_to_ltk(s)?;
            BinValue::Pointer { class, fields }
        }
        PropertyValue::Embedded(s) => {
            let (class, fields) = hematite_struct_to_ltk(s)?;
            BinValue::Embed { class, fields }
        }

        // Collections
        PropertyValue::Container(items) => vec_to_ltk_list(items, false)?,
        PropertyValue::UnorderedContainer(items) => vec_to_ltk_list(items, true)?,

        // Optional
        PropertyValue::Optional(opt) => option_to_ltk_optional(opt.as_ref())?,

        // Map
        PropertyValue::Map(pairs) => {
            let (key, value) = if pairs.is_empty() {
                // Empty map - default to U32 -> U32 (matches the previous behavior).
                (BinType::U32, BinType::U32)
            } else {
                (
                    hematite_value_to_ltk(&pairs[0].0)?.ty(),
                    hematite_value_to_ltk(&pairs[0].1)?.ty(),
                )
            };
            let mut entries = Vec::with_capacity(pairs.len());
            for (k, v) in pairs {
                entries.push((hematite_value_to_ltk(k)?, hematite_value_to_ltk(v)?));
            }
            BinValue::Map {
                key,
                value,
                entries,
            }
        }
    };

    Ok(out)
}

/// Build a Hematite `StructValue` from an rs_bin struct body.
fn struct_to_hematite(class: u32, fields: &IndexMap<u32, BinValue>) -> Result<StructValue> {
    let mut properties = IndexMap::new();

    for (name_hash, value) in fields {
        let prop = BinProperty {
            name_hash: FieldHash(*name_hash),
            value: ltk_value_to_hematite(value)?,
        };
        properties.insert(*name_hash, prop);
    }

    Ok(StructValue {
        class_hash: TypeHash(class),
        properties,
    })
}

/// Build an rs_bin struct body (class hash + fields) from a Hematite `StructValue`.
fn hematite_struct_to_ltk(s: &StructValue) -> Result<(u32, IndexMap<u32, BinValue>)> {
    let mut fields = IndexMap::new();

    for (name_hash, prop) in &s.properties {
        fields.insert(*name_hash, hematite_value_to_ltk(&prop.value)?);
    }

    Ok((s.class_hash.0, fields))
}

/// Convert a Vec<PropertyValue> to an rs_bin `List`/`List2` (infers item type from first element).
fn vec_to_ltk_list(items: &[PropertyValue], is_list2: bool) -> Result<BinValue> {
    let mut converted = Vec::with_capacity(items.len());
    for item in items {
        converted.push(hematite_value_to_ltk(item)?);
    }

    // Infer element tag from the first element; default to U32 for empty lists
    // (matches the previous behavior).
    let item = converted.first().map(BinValue::ty).unwrap_or(BinType::U32);

    Ok(BinValue::List {
        is_list2,
        item,
        items: converted,
    })
}

/// Convert an Option<PropertyValue> to an rs_bin `Option` (infers item type from Some value).
fn option_to_ltk_optional(opt: &Option<PropertyValue>) -> Result<BinValue> {
    let (item, value) = match opt {
        // Empty option - default to U32 (matches the previous behavior of an absent value).
        None => (BinType::U32, None),
        Some(val) => {
            let converted = hematite_value_to_ltk(val)?;
            (converted.ty(), Some(Box::new(converted)))
        }
    };

    Ok(BinValue::Option { item, value })
}
