//! Configurable edits over native rs_bin values. Names and hex hashes are accepted.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    /// Class of the object owning the selected path; empty means every class.
    #[serde(default)]
    pub classes: Vec<String>,
    /// Restrict traversal to these top-level entry paths/hashes.
    #[serde(default)]
    pub entries: Vec<String>,
    /// Field path relative to a matching class. `*` visits fields/container values.
    #[serde(default)]
    pub path: Vec<String>,
    /// Additional (immediate owner class, field) alternatives for large tables.
    #[serde(default)]
    pub targets: Vec<crate::config::ClassFieldTarget>,
}

/// A typed JSON value. Complex values recursively contain typed field values.
/// Examples: {"type":"file","value":"assets/a.tex"},
/// {"type":"embed","value":{"class":"Foo","fields":{"bar":{"type":"u8","value":1}}}}.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedValue {
    #[serde(rename = "type")]
    pub data_type: String,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum EditOperation {
    /// Create or replace selected fields. Missing final fields are allowed.
    Set {
        select: Selector,
        value: TypedValue,
    },
    /// Create only when absent; preserves an existing parent and its siblings.
    Ensure {
        select: Selector,
        value: TypedValue,
    },
    Remove {
        select: Selector,
    },
    Rename {
        select: Selector,
        to: String,
    },
    /// Convert matching payloads, retaining list/option/map wrappers and tags.
    Convert {
        select: Selector,
        from: String,
        to: String,
    },
    ReplaceString {
        select: Selector,
        pattern: String,
        replacement: String,
    },
    /// Change a selected pointer/embed class without replacing its fields.
    SetClass {
        select: Selector,
        class: String,
    },
    /// Create an entry in an existing or empty BIN; never overwrite implicitly.
    CreateEntry {
        path: String,
        class: String,
        fields: BTreeMap<String, TypedValue>,
    },
    RemoveEntry {
        paths: Vec<String>,
    },
}
