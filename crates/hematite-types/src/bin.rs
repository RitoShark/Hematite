//! BIN tree types — Hematite's own representation of League BIN files.
//!
//! These types mirror the structure of LTK's `BinTree` / `BinObject` / `PropertyValueEnum`
//! but are **owned by Hematite**. The LTK adapter crate converts between these and
//! whatever LTK version is currently in use, isolating the rest of the codebase from
//! LTK breaking changes.
//!
//! ## Key types
//! - [`BinTree`] — A parsed `.bin` file (map of path_hash → [`BinObject`])
//! - [`BinObject`] — A single object/entry in the tree
//! - [`BinProperty`] — A named property (field_hash + value)
//! - [`PropertyValue`] — The value of a property (enum over all League types)

use crate::hash::{FieldHash, PathHash, TypeHash};
use indexmap::IndexMap;

/// A parsed BIN file — a map of entry path hashes to objects.
#[derive(Debug, Clone, Default)]
pub struct BinTree {
    pub objects: IndexMap<u32, BinObject>,
    /// Linked BIN dependencies (paths from the BIN header's `dependencies` list).
    pub linked: Vec<String>,
    /// Raw bytes after the declared BIN body (e.g. the CELMAP hash→path side
    /// table). Preserved verbatim through parse → write.
    pub trailing: Vec<u8>,
    /// xxh64 `file` hash → original path pairs produced by transforms that
    /// retype path strings into hashes. Merged into the trailer side table by
    /// the write adapter so the readable paths are never lost.
    pub trailer_files: std::collections::BTreeMap<u64, String>,
}

/// A single object in a BIN tree.
#[derive(Debug, Clone)]
pub struct BinObject {
    /// Class hash identifying the object's type (e.g. SkinCharacterDataProperties).
    pub class_hash: TypeHash,
    /// Path hash of this entry.
    pub path_hash: PathHash,
    /// The object's properties, keyed by field name hash.
    pub properties: IndexMap<u32, BinProperty>,
}

/// A single property in a BIN object.
#[derive(Debug, Clone)]
pub struct BinProperty {
    /// Field name hash.
    pub name_hash: FieldHash,
    /// The property's value.
    pub value: PropertyValue,
}

/// All possible property value types in a BIN file.
///
/// Mirrors rs_bin's `BinValue`. The conversion between
/// rs_bin and Hematite types happens in `hematite-file/src/convert.rs`.
#[derive(Debug, Clone)]
pub enum PropertyValue {
    // Primitives
    Bool(bool),
    I8(i8),
    U8(u8),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F32(f32),
    // Vectors
    Vector2([f32; 2]),
    Vector3([f32; 3]),
    Vector4([f32; 4]),
    // Matrices
    Matrix4x4([[f32; 4]; 4]),
    // Strings & hashes
    String(String),
    Hash(u32),
    WadHash(u64),
    // Links
    Link(u32),
    // Color
    Color([u8; 4]),
    // Nested structures
    Struct(StructValue),
    Embedded(StructValue),
    // Collections
    Container(Vec<PropertyValue>),
    UnorderedContainer(Vec<PropertyValue>),
    // Optional
    Optional(Box<Option<PropertyValue>>),
    // Map
    Map(Vec<(PropertyValue, PropertyValue)>),
    // Flags / bitfield
    BitBool(u8),
}

/// A struct-like value containing typed properties.
#[derive(Debug, Clone)]
pub struct StructValue {
    /// The class hash of this struct type.
    pub class_hash: TypeHash,
    /// Properties within the struct.
    pub properties: IndexMap<u32, BinProperty>,
}
