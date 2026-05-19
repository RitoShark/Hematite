//! LTK ↔ Hematite type conversion.
//!
//! When LTK changes its API, only this file needs updating.

use anyhow::{bail, Result};
use hematite_types::bin::{BinObject, BinProperty, BinTree, PropertyValue, StructValue};
use hematite_types::hash::{FieldHash, PathHash, TypeHash};
use indexmap::IndexMap;
use league_toolkit::meta::property::{values::*, NoMeta};
use league_toolkit::meta::{
    Bin as LtkBin, BinObject as LtkBinObject, BinProperty as LtkBinProperty,
    PropertyValueEnum as LtkValue,
};

/// Convert LTK Bin to Hematite BinTree (after parsing).
pub fn ltk_tree_to_hematite(ltk_tree: LtkBin) -> Result<BinTree> {
    let mut objects = IndexMap::new();

    for (path_hash, ltk_obj) in ltk_tree.objects {
        let obj = ltk_object_to_hematite(ltk_obj)?;
        objects.insert(path_hash, obj);
    }

    let linked = ltk_tree.dependencies;

    Ok(BinTree { objects, linked })
}

/// Convert Hematite BinTree to LTK Bin (before writing).
pub fn hematite_tree_to_ltk(tree: &BinTree) -> Result<LtkBin> {
    let mut objects = Vec::new();

    for obj in tree.objects.values() {
        objects.push(hematite_object_to_ltk(obj)?);
    }

    Ok(LtkBin::new(objects, tree.linked.clone()))
}

/// Convert single LTK BinObject to Hematite BinObject.
fn ltk_object_to_hematite(ltk_obj: LtkBinObject) -> Result<BinObject> {
    let mut properties = IndexMap::new();

    for (name_hash, ltk_prop) in ltk_obj.properties {
        let prop = BinProperty {
            name_hash: FieldHash(name_hash),
            value: ltk_value_to_hematite(&ltk_prop.value)?,
        };
        properties.insert(name_hash, prop);
    }

    Ok(BinObject {
        class_hash: TypeHash(ltk_obj.class_hash),
        path_hash: PathHash(ltk_obj.path_hash),
        properties,
    })
}

/// Convert single Hematite BinObject to LTK BinObject.
fn hematite_object_to_ltk(obj: &BinObject) -> Result<LtkBinObject> {
    let mut properties = IndexMap::new();

    for (name_hash, prop) in &obj.properties {
        let ltk_prop = LtkBinProperty {
            name_hash: *name_hash,
            value: hematite_value_to_ltk(&prop.value)?,
        };
        properties.insert(*name_hash, ltk_prop);
    }

    Ok(LtkBinObject {
        path_hash: obj.path_hash.0,
        class_hash: obj.class_hash.0,
        properties,
    })
}

/// Convert LTK PropertyValueEnum to Hematite PropertyValue.
pub fn ltk_value_to_hematite(ltk_val: &LtkValue) -> Result<PropertyValue> {
    let val = match ltk_val {
        // Primitives
        LtkValue::Bool(v) => PropertyValue::Bool(v.value),
        LtkValue::I8(v) => PropertyValue::I8(v.value),
        LtkValue::U8(v) => PropertyValue::U8(v.value),
        LtkValue::I16(v) => PropertyValue::I16(v.value),
        LtkValue::U16(v) => PropertyValue::U16(v.value),
        LtkValue::I32(v) => PropertyValue::I32(v.value),
        LtkValue::U32(v) => PropertyValue::U32(v.value),
        LtkValue::I64(v) => PropertyValue::I64(v.value),
        LtkValue::U64(v) => PropertyValue::U64(v.value),
        LtkValue::F32(v) => PropertyValue::F32(v.value),

        // Vectors (LTK uses glam types, we use arrays)
        LtkValue::Vector2(v) => PropertyValue::Vector2(v.value.to_array()),
        LtkValue::Vector3(v) => PropertyValue::Vector3(v.value.to_array()),
        LtkValue::Vector4(v) => PropertyValue::Vector4(v.value.to_array()),

        // Matrix (LTK uses Matrix44, we use Matrix4x4)
        LtkValue::Matrix44(v) => PropertyValue::Matrix4x4(v.value.to_cols_array_2d()),

        // Strings & hashes
        LtkValue::String(v) => PropertyValue::String(v.value.clone()),
        LtkValue::Hash(v) => PropertyValue::Hash(v.value),
        LtkValue::WadChunkLink(v) => PropertyValue::Link(
            v.value
                .try_into()
                .map_err(|_| anyhow::anyhow!("WadChunkLink value {} exceeds u32::MAX", v.value))?,
        ),
        LtkValue::ObjectLink(v) => PropertyValue::Link(v.value),
        LtkValue::Color(v) => PropertyValue::Color([v.value.r, v.value.g, v.value.b, v.value.a]),
        LtkValue::BitBool(v) => PropertyValue::BitBool(if v.value { 1 } else { 0 }),
        LtkValue::None(_) => bail!("None value encountered in BIN"),

        // Nested structures
        LtkValue::Struct(s) => PropertyValue::Struct(ltk_struct_to_hematite(s)?),
        LtkValue::Embedded(e) => {
            // Embedded wraps Struct via .0
            PropertyValue::Embedded(ltk_struct_to_hematite(&e.0)?)
        }

        // Collections
        LtkValue::Container(c) => PropertyValue::Container(ltk_container_to_vec(c)?),
        LtkValue::UnorderedContainer(uc) => {
            // UnorderedContainer wraps Container via .0
            PropertyValue::UnorderedContainer(ltk_container_to_vec(&uc.0)?)
        }

        // Optional
        LtkValue::Optional(o) => PropertyValue::Optional(Box::new(ltk_optional_to_option(o)?)),

        // Map
        LtkValue::Map(m) => {
            let mut pairs = Vec::new();
            for (k, v) in m.entries() {
                pairs.push((ltk_value_to_hematite(k)?, ltk_value_to_hematite(v)?));
            }
            PropertyValue::Map(pairs)
        }
    };

    Ok(val)
}

/// Convert Hematite PropertyValue to LTK PropertyValueEnum.
pub fn hematite_value_to_ltk(val: &PropertyValue) -> Result<LtkValue> {
    let ltk_val = match val {
        // Primitives - use ::new() constructors
        PropertyValue::Bool(v) => LtkValue::Bool(Bool::new(*v)),
        PropertyValue::I8(v) => LtkValue::I8(I8::new(*v)),
        PropertyValue::U8(v) => LtkValue::U8(U8::new(*v)),
        PropertyValue::I16(v) => LtkValue::I16(I16::new(*v)),
        PropertyValue::U16(v) => LtkValue::U16(U16::new(*v)),
        PropertyValue::I32(v) => LtkValue::I32(I32::new(*v)),
        PropertyValue::U32(v) => LtkValue::U32(U32::new(*v)),
        PropertyValue::I64(v) => LtkValue::I64(I64::new(*v)),
        PropertyValue::U64(v) => LtkValue::U64(U64::new(*v)),
        PropertyValue::F32(v) => LtkValue::F32(F32::new(*v)),

        // Vectors - convert arrays to glam types
        PropertyValue::Vector2(v) => LtkValue::Vector2(Vector2::new((*v).into())),
        PropertyValue::Vector3(v) => LtkValue::Vector3(Vector3::new((*v).into())),
        PropertyValue::Vector4(v) => LtkValue::Vector4(Vector4::new((*v).into())),

        // Matrix - convert 2D array to glam Mat4
        PropertyValue::Matrix4x4(v) => {
            LtkValue::Matrix44(Matrix44::new(glam::Mat4::from_cols_array_2d(v)))
        }

        // Strings & hashes
        PropertyValue::String(v) => LtkValue::String(String::new(v.clone())),
        PropertyValue::Hash(v) => LtkValue::Hash(Hash::new(*v)),
        PropertyValue::Link(v) => LtkValue::ObjectLink(ObjectLink::new(*v)),
        PropertyValue::WadHash(v) => LtkValue::WadChunkLink(WadChunkLink::new(*v)),
        PropertyValue::Color(rgba) => LtkValue::Color(Color::new(ltk_primitives::Color {
            r: rgba[0],
            g: rgba[1],
            b: rgba[2],
            a: rgba[3],
        })),
        PropertyValue::BitBool(v) => LtkValue::BitBool(BitBool::new(*v != 0)),

        // Nested structures
        PropertyValue::Struct(s) => LtkValue::Struct(hematite_struct_to_ltk(s)?),
        PropertyValue::Embedded(s) => LtkValue::Embedded(Embedded(hematite_struct_to_ltk(s)?)),

        // Collections
        PropertyValue::Container(items) => vec_to_ltk_container(items)?,
        PropertyValue::UnorderedContainer(items) => {
            LtkValue::UnorderedContainer(UnorderedContainer(match vec_to_ltk_container(items)? {
                LtkValue::Container(c) => c,
                _ => unreachable!("vec_to_ltk_container always returns Container"),
            }))
        }

        // Optional
        PropertyValue::Optional(opt) => option_to_ltk_optional(opt.as_ref())?,

        // Map
        PropertyValue::Map(pairs) => {
            if pairs.is_empty() {
                // Empty map - default to U32->U32
                LtkValue::Map(Map::empty(
                    league_toolkit::meta::property::Kind::U32,
                    league_toolkit::meta::property::Kind::U32,
                ))
            } else {
                let key_kind = hematite_value_to_ltk(&pairs[0].0)?.kind();
                let value_kind = hematite_value_to_ltk(&pairs[0].1)?.kind();

                let mut ltk_pairs = Vec::new();
                for (k, v) in pairs {
                    ltk_pairs.push((hematite_value_to_ltk(k)?, hematite_value_to_ltk(v)?));
                }
                LtkValue::Map(Map::new(key_kind, value_kind, ltk_pairs)?)
            }
        }
    };

    Ok(ltk_val)
}

/// Convert LTK Struct to Hematite StructValue.
fn ltk_struct_to_hematite(ltk_struct: &Struct) -> Result<StructValue> {
    let mut properties = IndexMap::new();

    for (name_hash, ltk_prop) in &ltk_struct.properties {
        let prop = BinProperty {
            name_hash: FieldHash(*name_hash),
            value: ltk_value_to_hematite(&ltk_prop.value)?,
        };
        properties.insert(*name_hash, prop);
    }

    Ok(StructValue {
        class_hash: TypeHash(ltk_struct.class_hash),
        properties,
    })
}

/// Convert Hematite StructValue to LTK Struct.
fn hematite_struct_to_ltk(s: &StructValue) -> Result<Struct> {
    let mut properties = IndexMap::new();

    for (name_hash, prop) in &s.properties {
        let ltk_prop = LtkBinProperty {
            name_hash: *name_hash,
            value: hematite_value_to_ltk(&prop.value)?,
        };
        properties.insert(*name_hash, ltk_prop);
    }

    Ok(Struct {
        class_hash: s.class_hash.0,
        properties,
        meta: NoMeta,
    })
}

/// Convert LTK Container enum to Vec<PropertyValue>.
fn ltk_container_to_vec(c: &Container) -> Result<Vec<PropertyValue>> {
    let mut vec = Vec::new();

    match c {
        Container::Bool { items, .. } => {
            for item in items {
                vec.push(PropertyValue::Bool(item.value));
            }
        }
        Container::I8 { items, .. } => {
            for item in items {
                vec.push(PropertyValue::I8(item.value));
            }
        }
        Container::U8 { items, .. } => {
            for item in items {
                vec.push(PropertyValue::U8(item.value));
            }
        }
        Container::I16 { items, .. } => {
            for item in items {
                vec.push(PropertyValue::I16(item.value));
            }
        }
        Container::U16 { items, .. } => {
            for item in items {
                vec.push(PropertyValue::U16(item.value));
            }
        }
        Container::I32 { items, .. } => {
            for item in items {
                vec.push(PropertyValue::I32(item.value));
            }
        }
        Container::U32 { items, .. } => {
            for item in items {
                vec.push(PropertyValue::U32(item.value));
            }
        }
        Container::I64 { items, .. } => {
            for item in items {
                vec.push(PropertyValue::I64(item.value));
            }
        }
        Container::U64 { items, .. } => {
            for item in items {
                vec.push(PropertyValue::U64(item.value));
            }
        }
        Container::F32 { items, .. } => {
            for item in items {
                vec.push(PropertyValue::F32(item.value));
            }
        }
        Container::Vector2 { items, .. } => {
            for item in items {
                vec.push(PropertyValue::Vector2(item.value.to_array()));
            }
        }
        Container::Vector3 { items, .. } => {
            for item in items {
                vec.push(PropertyValue::Vector3(item.value.to_array()));
            }
        }
        Container::Vector4 { items, .. } => {
            for item in items {
                vec.push(PropertyValue::Vector4(item.value.to_array()));
            }
        }
        Container::Matrix44 { items, .. } => {
            for item in items {
                vec.push(PropertyValue::Matrix4x4(item.value.to_cols_array_2d()));
            }
        }
        Container::String { items, .. } => {
            for item in items {
                vec.push(PropertyValue::String(item.value.clone()));
            }
        }
        Container::Hash { items, .. } => {
            for item in items {
                vec.push(PropertyValue::Hash(item.value));
            }
        }
        Container::Struct { items, .. } => {
            for item in items {
                vec.push(PropertyValue::Struct(ltk_struct_to_hematite(item)?));
            }
        }
        Container::Embedded { items, .. } => {
            for item in items {
                vec.push(PropertyValue::Embedded(ltk_struct_to_hematite(&item.0)?));
            }
        }
        Container::WadChunkLink { items, .. } => {
            for item in items {
                vec.push(PropertyValue::Link(
                    item.value.try_into().map_err(|_| anyhow::anyhow!("WadChunkLink value {} exceeds u32::MAX", item.value))?
                ));
            }
        }
        Container::ObjectLink { items, .. } => {
            for item in items {
                vec.push(PropertyValue::Link(item.value));
            }
        }
        Container::Color { items, .. } => {
            for item in items {
                vec.push(PropertyValue::Color([item.value.r, item.value.g, item.value.b, item.value.a]));
            }
        }
        Container::BitBool { items, .. } => {
            for item in items {
                vec.push(PropertyValue::BitBool(if item.value { 1 } else { 0 }));
            }
        }
        Container::None { .. } => {}
    }

    Ok(vec)
}

/// Convert Vec<PropertyValue> to LTK Container (infers type from first element).
fn vec_to_ltk_container(items: &[PropertyValue]) -> Result<LtkValue> {
    if items.is_empty() {
        // Empty container - default to None type
        return Ok(LtkValue::Container(Container::from(Vec::<None>::new())));
    }

    let container = match &items[0] {
        PropertyValue::Bool(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::Bool(v) = item {
                    ltk_items.push(Bool::new(*v));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::I8(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::I8(v) = item {
                    ltk_items.push(I8::new(*v));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::U8(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::U8(v) = item {
                    ltk_items.push(U8::new(*v));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::I16(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::I16(v) = item {
                    ltk_items.push(I16::new(*v));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::U16(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::U16(v) = item {
                    ltk_items.push(U16::new(*v));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::I32(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::I32(v) = item {
                    ltk_items.push(I32::new(*v));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::U32(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::U32(v) = item {
                    ltk_items.push(U32::new(*v));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::I64(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::I64(v) = item {
                    ltk_items.push(I64::new(*v));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::U64(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::U64(v) = item {
                    ltk_items.push(U64::new(*v));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::F32(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::F32(v) = item {
                    ltk_items.push(F32::new(*v));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::Vector2(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::Vector2(v) = item {
                    ltk_items.push(Vector2::new((*v).into()));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::Vector3(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::Vector3(v) = item {
                    ltk_items.push(Vector3::new((*v).into()));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::Vector4(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::Vector4(v) = item {
                    ltk_items.push(Vector4::new((*v).into()));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::Matrix4x4(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::Matrix4x4(v) = item {
                    ltk_items.push(Matrix44::new(glam::Mat4::from_cols_array_2d(v)));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::String(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::String(v) = item {
                    ltk_items.push(String::new(v.clone()));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::Hash(_) | PropertyValue::Link(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                match item {
                    PropertyValue::Hash(v) | PropertyValue::Link(v) => {
                        ltk_items.push(Hash::new(*v));
                    }
                    _ => bail!("Mixed types in container"),
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::WadHash(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::WadHash(v) = item {
                    ltk_items.push(WadChunkLink::new(*v));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::Color(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::Color(rgba) = item {
                    ltk_items.push(Color::new(ltk_primitives::Color {
                        r: rgba[0],
                        g: rgba[1],
                        b: rgba[2],
                        a: rgba[3],
                    }));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::BitBool(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::BitBool(v) = item {
                    ltk_items.push(BitBool::new(*v != 0));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::Struct(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::Struct(v) = item {
                    ltk_items.push(hematite_struct_to_ltk(v)?);
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        PropertyValue::Embedded(_) => {
            let mut ltk_items = Vec::new();
            for item in items {
                if let PropertyValue::Embedded(v) = item {
                    ltk_items.push(Embedded(hematite_struct_to_ltk(v)?));
                } else {
                    bail!("Mixed types in container");
                }
            }
            Container::from(ltk_items)
        }
        _ => bail!("Unsupported container item type"),
    };

    Ok(LtkValue::Container(container))
}

/// Convert LTK Optional enum to Option<PropertyValue>.
fn ltk_optional_to_option(o: &Optional) -> Result<Option<PropertyValue>> {
    let opt = match o {
        Optional::None(_) => None,
        Optional::Bool(v) => v.as_ref().map(|inner| PropertyValue::Bool(inner.value)),
        Optional::I8(v) => v.as_ref().map(|inner| PropertyValue::I8(inner.value)),
        Optional::U8(v) => v.as_ref().map(|inner| PropertyValue::U8(inner.value)),
        Optional::I16(v) => v.as_ref().map(|inner| PropertyValue::I16(inner.value)),
        Optional::U16(v) => v.as_ref().map(|inner| PropertyValue::U16(inner.value)),
        Optional::I32(v) => v.as_ref().map(|inner| PropertyValue::I32(inner.value)),
        Optional::U32(v) => v.as_ref().map(|inner| PropertyValue::U32(inner.value)),
        Optional::I64(v) => v.as_ref().map(|inner| PropertyValue::I64(inner.value)),
        Optional::U64(v) => v.as_ref().map(|inner| PropertyValue::U64(inner.value)),
        Optional::F32(v) => v.as_ref().map(|inner| PropertyValue::F32(inner.value)),
        Optional::Vector2(v) => v
            .as_ref()
            .map(|inner| PropertyValue::Vector2(inner.value.to_array())),
        Optional::Vector3(v) => v
            .as_ref()
            .map(|inner| PropertyValue::Vector3(inner.value.to_array())),
        Optional::Vector4(v) => v
            .as_ref()
            .map(|inner| PropertyValue::Vector4(inner.value.to_array())),
        Optional::Matrix44(v) => v
            .as_ref()
            .map(|inner| PropertyValue::Matrix4x4(inner.value.to_cols_array_2d())),
        Optional::String(v) => v
            .as_ref()
            .map(|inner| PropertyValue::String(inner.value.clone())),
        Optional::Hash(v) => v.as_ref().map(|inner| PropertyValue::Hash(inner.value)),
        Optional::Struct(v) => match v {
            Some(s) => Some(PropertyValue::Struct(ltk_struct_to_hematite(s)?)),
            None => None,
        },
        Optional::Embedded(v) => match v {
            Some(e) => Some(PropertyValue::Embedded(ltk_struct_to_hematite(&e.0)?)),
            None => None,
        },
        Optional::Color(v) => v.as_ref().map(|inner| {
            PropertyValue::Color([inner.value.r, inner.value.g, inner.value.b, inner.value.a])
        }),
        Optional::WadChunkLink(v) => match v {
            Some(inner) => Some(PropertyValue::Link(inner.value.try_into().map_err(
                |_| anyhow::anyhow!("WadChunkLink value {} exceeds u32::MAX", inner.value),
            )?)),
            None => None,
        },
        Optional::ObjectLink(v) => v.as_ref().map(|inner| PropertyValue::Link(inner.value)),
        Optional::BitBool(v) => v
            .as_ref()
            .map(|inner| PropertyValue::BitBool(if inner.value { 1 } else { 0 })),
    };

    Ok(opt)
}

/// Convert Option<PropertyValue> to LTK Optional (infers type from Some value).
fn option_to_ltk_optional(opt: &Option<PropertyValue>) -> Result<LtkValue> {
    let ltk_opt = match opt {
        None => LtkValue::Optional(Optional::None(None)),
        Some(val) => match val {
            PropertyValue::Bool(v) => LtkValue::Optional(Optional::from(Bool::new(*v))),
            PropertyValue::I8(v) => LtkValue::Optional(Optional::from(I8::new(*v))),
            PropertyValue::U8(v) => LtkValue::Optional(Optional::from(U8::new(*v))),
            PropertyValue::I16(v) => LtkValue::Optional(Optional::from(I16::new(*v))),
            PropertyValue::U16(v) => LtkValue::Optional(Optional::from(U16::new(*v))),
            PropertyValue::I32(v) => LtkValue::Optional(Optional::from(I32::new(*v))),
            PropertyValue::U32(v) => LtkValue::Optional(Optional::from(U32::new(*v))),
            PropertyValue::I64(v) => LtkValue::Optional(Optional::from(I64::new(*v))),
            PropertyValue::U64(v) => LtkValue::Optional(Optional::from(U64::new(*v))),
            PropertyValue::F32(v) => LtkValue::Optional(Optional::from(F32::new(*v))),
            PropertyValue::Vector2(v) => {
                LtkValue::Optional(Optional::from(Vector2::new((*v).into())))
            }
            PropertyValue::Vector3(v) => {
                LtkValue::Optional(Optional::from(Vector3::new((*v).into())))
            }
            PropertyValue::Vector4(v) => {
                LtkValue::Optional(Optional::from(Vector4::new((*v).into())))
            }
            PropertyValue::Matrix4x4(v) => LtkValue::Optional(Optional::from(Matrix44::new(
                glam::Mat4::from_cols_array_2d(v),
            ))),
            PropertyValue::String(v) => LtkValue::Optional(Optional::from(String::new(v.clone()))),
            PropertyValue::Hash(v) | PropertyValue::Link(v) => {
                LtkValue::Optional(Optional::from(Hash::new(*v)))
            }
            PropertyValue::WadHash(v) => LtkValue::Optional(Optional::from(WadChunkLink::new(*v))),
            PropertyValue::Color(rgba) => {
                LtkValue::Optional(Optional::from(Color::new(ltk_primitives::Color {
                    r: rgba[0],
                    g: rgba[1],
                    b: rgba[2],
                    a: rgba[3],
                })))
            }
            PropertyValue::BitBool(v) => LtkValue::Optional(Optional::from(BitBool::new(*v != 0))),
            PropertyValue::Struct(s) => {
                LtkValue::Optional(Optional::from(hematite_struct_to_ltk(s)?))
            }
            PropertyValue::Embedded(s) => {
                LtkValue::Optional(Optional::from(Embedded(hematite_struct_to_ltk(s)?)))
            }
            _ => bail!("Unsupported optional type"),
        },
    };

    Ok(ltk_opt)
}
