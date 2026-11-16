//! Generic, atomic BIN edits. rs_bin owns value syntax, numeric conversion and hashing.
use anyhow::{bail, ensure, Context, Result};
use hematite_types::bin::BinTree;
use hematite_types::convert::{hematite_tree_to_rs_bin, rs_bin_tree_to_hematite};
use hematite_types::edit::{EditOperation, Selector, TypedValue};
use indexmap::IndexMap;
use rs_bin::{BinEntry, BinType, BinValue};
use serde_json::Value;
use std::collections::BTreeMap;

type Fields = IndexMap<u32, BinValue>;

pub fn hash(name: &str) -> Result<u32> {
    if let Some(hex) = name.strip_prefix("0x").or_else(|| name.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).context("invalid hex hash");
    }
    let value = rs_bin::value_from_text(&format!("hash = {}", serde_json::to_string(name)?), None)?;
    match value {
        BinValue::Hash(h) => Ok(h),
        _ => unreachable!(),
    }
}

fn tag(name: &str) -> Result<BinType> {
    Ok(match name {
        "none" => BinType::None,
        "bool" => BinType::Bool,
        "i8" => BinType::I8,
        "u8" => BinType::U8,
        "i16" => BinType::I16,
        "u16" => BinType::U16,
        "i32" => BinType::I32,
        "u32" => BinType::U32,
        "i64" => BinType::I64,
        "u64" => BinType::U64,
        "f32" => BinType::F32,
        "vec2" => BinType::Vec2,
        "vec3" => BinType::Vec3,
        "vec4" => BinType::Vec4,
        "mtx44" => BinType::Mtx44,
        "rgba" => BinType::Rgba,
        "string" => BinType::String,
        "hash" => BinType::Hash,
        "file" => BinType::File,
        "link" => BinType::Link,
        "pointer" => BinType::Pointer,
        "embed" => BinType::Embed,
        "flag" => BinType::Flag,
        _ => bail!("unknown scalar BIN type: {name}"),
    })
}

/// Render typed JSON into rs_bin's value grammar; rs_bin parses and validates the value.
fn body(ty: &str, json: &Value) -> Result<String> {
    if let Some((container, rest)) = ty.split_once('[') {
        let inner = rest.strip_suffix(']').context("unclosed container type")?;
        let items = json
            .as_array()
            .context("container value must be an array")?;
        if container == "map" {
            let (key, value) = inner.split_once(',').context("map needs key,value types")?;
            ensure!(tag(key)?.is_primitive(), "map keys must be primitive");
            tag(value)?;
            let mut rendered = Vec::new();
            for pair in items {
                let pair = pair
                    .as_array()
                    .context("map entries must be [key,value] pairs")?;
                ensure!(pair.len() == 2, "map entries must have two values");
                rendered.push(format!(
                    "{} = {}",
                    body(key, &pair[0])?,
                    body(value, &pair[1])?
                ));
            }
            return Ok(format!("{{ {} }}", rendered.join("\n")));
        }
        ensure!(
            matches!(container, "list" | "list2" | "option"),
            "unknown container: {container}"
        );
        tag(inner)?;
        ensure!(
            container != "option" || items.len() <= 1,
            "option holds at most one value"
        );
        let rendered = items
            .iter()
            .map(|v| body(inner, v))
            .collect::<Result<Vec<_>>>()?;
        return Ok(format!("{{ {} }}", rendered.join("\n")));
    }
    let ty_tag = tag(ty)?;
    if matches!(ty_tag, BinType::Embed | BinType::Pointer) {
        if json.is_null() && ty_tag == BinType::Pointer {
            return Ok("null".into());
        }
        let object = json
            .as_object()
            .context("struct value must contain class and fields")?;
        ensure!(
            object.keys().all(|k| k == "class" || k == "fields"),
            "unknown struct value key"
        );
        let class = hash(
            object
                .get("class")
                .and_then(Value::as_str)
                .context("missing struct class")?,
        )?;
        let fields: BTreeMap<String, TypedValue> = serde_json::from_value(
            object
                .get("fields")
                .cloned()
                .context("missing struct fields")?,
        )?;
        let fields = typed_fields(&fields)?;
        return Ok(rs_bin::value_to_text(
            &BinValue::Embed { class, fields },
            None,
        ));
    }
    if matches!(
        ty_tag,
        BinType::Vec2 | BinType::Vec3 | BinType::Vec4 | BinType::Mtx44 | BinType::Rgba
    ) {
        let values = json
            .as_array()
            .context("vector/matrix/color must be an array")?;
        ensure!(
            values.iter().all(Value::is_number),
            "components must be numbers"
        );
        return Ok(format!(
            "{{ {} }}",
            values
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    ensure!(
        !json.is_array() && !json.is_object(),
        "expected scalar value"
    );
    if matches!(ty_tag, BinType::Hash | BinType::Link | BinType::File) {
        if let Some(n) = json.as_u64() {
            return Ok(format!("0x{n:x}"));
        }
        if let Some(s) = json.as_str() {
            if s.starts_with("0x") || s.starts_with("0X") {
                let n = u64::from_str_radix(&s[2..], 16).context("invalid hex hash")?;
                return Ok(format!("0x{n:x}"));
            }
        }
    }
    Ok(serde_json::to_string(json)?)
}

pub fn typed_value(value: &TypedValue) -> Result<BinValue> {
    let rendered = body(&value.data_type, &value.value)?;
    rs_bin::value_from_text(&format!("{} = {rendered}", value.data_type), None)
        .with_context(|| format!("invalid {} value", value.data_type))
}

fn typed_fields(fields: &BTreeMap<String, TypedValue>) -> Result<Fields> {
    let mut out = Fields::new();
    for (name, value) in fields {
        ensure!(
            out.insert(hash(name)?, typed_value(value)?).is_none(),
            "duplicate field hash: {name}"
        );
    }
    Ok(out)
}

fn remember_paths(ty: &str, json: &Value, files: &mut BTreeMap<u64, String>) -> Result<()> {
    if ty == "file" {
        if let Some(path) = json
            .as_str()
            .filter(|s| !s.starts_with("0x") && !s.starts_with("0X"))
        {
            let BinValue::File(h) = typed_value(&TypedValue {
                data_type: ty.into(),
                value: json.clone(),
            })?
            else {
                unreachable!()
            };
            files.insert(h, path.into());
        }
    } else if matches!(ty, "pointer" | "embed") && !json.is_null() {
        let fields: BTreeMap<String, TypedValue> = serde_json::from_value(json["fields"].clone())?;
        for value in fields.values() {
            remember_paths(&value.data_type, &value.value, files)?;
        }
    } else if let Some((container, inner)) = ty.split_once('[') {
        let inner = inner.trim_end_matches(']');
        for value in json.as_array().context("expected container")? {
            if container == "map" {
                let (key_ty, value_ty) = inner.split_once(',').context("invalid map type")?;
                remember_paths(key_ty, &value[0], files)?;
                remember_paths(value_ty, &value[1], files)?;
            } else {
                remember_paths(inner, value, files)?;
            }
        }
    }
    Ok(())
}

struct CompiledSelector {
    classes: Vec<u32>,
    entries: Vec<u32>,
    paths: Vec<Vec<Option<u32>>>,
    targets: Vec<(u32, u32)>,
}
impl CompiledSelector {
    fn new(s: &Selector) -> Result<Self> {
        ensure!(
            !s.path.is_empty() || !s.targets.is_empty(),
            "selector needs a field path or targets"
        );
        ensure!(
            s.path.is_empty() || s.targets.is_empty(),
            "use path or targets, not both"
        );
        Ok(Self {
            classes: s.classes.iter().map(|s| hash(s)).collect::<Result<_>>()?,
            entries: s.entries.iter().map(|s| hash(s)).collect::<Result<_>>()?,
            paths: if s.path.is_empty() {
                vec![]
            } else {
                vec![s
                    .path
                    .iter()
                    .map(|s| {
                        if s == "*" {
                            Ok(None)
                        } else {
                            hash(s).map(Some)
                        }
                    })
                    .collect::<Result<_>>()?]
            },
            targets: s
                .targets
                .iter()
                .map(|t| Ok((hash(&t.class)?, hash(&t.field)?)))
                .collect::<Result<_>>()?,
        })
    }
}

/// Execute on a copy and commit only after every operation succeeds. Preview uses the same path.
pub fn apply(tree: &mut BinTree, operations: &[EditOperation], preview: bool) -> Result<u32> {
    let mut bin = hematite_tree_to_rs_bin(tree)?;
    let mut files = tree.recorded_files.clone();
    let mut changes = 0;
    for (index, operation) in operations.iter().enumerate() {
        changes += execute(&mut bin, operation, &mut files)
            .with_context(|| format!("operation {}", index + 1))?;
    }
    if changes > 0 && !preview {
        let mut updated = rs_bin_tree_to_hematite(bin)?;
        updated.recorded_files.extend(files);
        *tree = updated;
    }
    Ok(changes)
}

fn execute(
    bin: &mut rs_bin::Bin,
    op: &EditOperation,
    files: &mut BTreeMap<u64, String>,
) -> Result<u32> {
    let select = match op {
        EditOperation::CreateEntry {
            path,
            class,
            fields,
        } => {
            let entry = BinEntry {
                path_hash: hash(path)?,
                class_hash: hash(class)?,
                fields: typed_fields(fields)?,
            };
            if let Some(existing) = bin.entries.iter().find(|e| e.path_hash == entry.path_hash) {
                ensure!(*existing == entry, "entry already exists: {path}");
                return Ok(0);
            }
            bin.entries.push(entry);
            for value in fields.values() {
                remember_paths(&value.data_type, &value.value, files)?;
            }
            return Ok(1);
        }
        EditOperation::RemoveEntry { paths } => {
            let hashes = paths.iter().map(|p| hash(p)).collect::<Result<Vec<_>>>()?;
            let before = bin.entries.len();
            bin.entries.retain(|e| !hashes.contains(&e.path_hash));
            return Ok((before - bin.entries.len()) as u32);
        }
        EditOperation::Set { select, value } | EditOperation::Ensure { select, value } => {
            typed_value(value)?;
            select
        }
        EditOperation::Convert { select, from, to } => {
            tag(from)?;
            tag(to)?;
            select
        }
        EditOperation::ReplaceString {
            select, pattern, ..
        } => {
            regex::Regex::new(pattern)?;
            select
        }
        EditOperation::Rename { select, to } => {
            hash(to)?;
            select
        }
        EditOperation::SetClass { select, class } => {
            hash(class)?;
            select
        }
        EditOperation::Remove { select } => select,
    };
    let compiled = CompiledSelector::new(select)?;
    let mut count = 0;
    for entry in &mut bin.entries {
        if compiled.entries.is_empty() || compiled.entries.contains(&entry.path_hash) {
            count += visit_fields(entry.class_hash, &mut entry.fields, &compiled, op, files)?;
        }
    }
    Ok(count)
}

fn visit_fields(
    class: u32,
    fields: &mut Fields,
    select: &CompiledSelector,
    op: &EditOperation,
    files: &mut BTreeMap<u64, String>,
) -> Result<u32> {
    let mut count = 0;
    // Descend first so newly inserted values aren't revisited by this operation.
    for value in fields.values_mut() {
        count += visit_value(value, select, op, files)?;
    }
    if select.classes.is_empty() || select.classes.contains(&class) {
        for path in &select.paths {
            count += at_fields(fields, path, op, files)?;
        }
        for &(target_class, field) in &select.targets {
            if class == target_class {
                count += at_fields(fields, &[Some(field)], op, files)?;
            }
        }
    }
    Ok(count)
}

fn visit_value(
    value: &mut BinValue,
    select: &CompiledSelector,
    op: &EditOperation,
    files: &mut BTreeMap<u64, String>,
) -> Result<u32> {
    let mut count = 0;
    match value {
        BinValue::Embed { class, fields } | BinValue::Pointer { class, fields } => {
            count += visit_fields(*class, fields, select, op, files)?
        }
        BinValue::List { items, .. } => {
            for item in items {
                count += visit_value(item, select, op, files)?;
            }
        }
        BinValue::Option { value: Some(v), .. } => count += visit_value(v, select, op, files)?,
        BinValue::Map { entries, .. } => {
            for (_, v) in entries {
                count += visit_value(v, select, op, files)?;
            }
        }
        _ => {}
    }
    Ok(count)
}

fn at_fields(
    fields: &mut Fields,
    path: &[Option<u32>],
    op: &EditOperation,
    files: &mut BTreeMap<u64, String>,
) -> Result<u32> {
    let Some((first, rest)) = path.split_first() else {
        bail!("empty field path");
    };
    let keys = match first {
        Some(h) => vec![*h],
        None => fields.keys().copied().collect(),
    };
    let mut count = 0;
    for key in keys {
        if rest.is_empty() {
            count += edit_field(fields, key, op, files)?;
        } else if let Some(value) = fields.get_mut(&key) {
            count += at_value(value, rest, op, files)?;
        }
    }
    Ok(count)
}

fn at_value(
    value: &mut BinValue,
    path: &[Option<u32>],
    op: &EditOperation,
    files: &mut BTreeMap<u64, String>,
) -> Result<u32> {
    match value {
        BinValue::Embed { fields, .. } | BinValue::Pointer { fields, .. } => {
            at_fields(fields, path, op, files)
        }
        BinValue::List { items, .. } if path.first() == Some(&None) => {
            ensure!(
                path.len() > 1,
                "select the container field to edit its payloads"
            );
            let mut count = 0;
            for item in items {
                count += at_value(item, &path[1..], op, files)?;
            }
            Ok(count)
        }
        BinValue::Map { entries, .. } if path.first() == Some(&None) => {
            ensure!(path.len() > 1, "select the map field to edit its payloads");
            let mut count = 0;
            for (_, item) in entries {
                count += at_value(item, &path[1..], op, files)?;
            }
            Ok(count)
        }
        BinValue::Option {
            value: Some(inner), ..
        } if path.first() == Some(&None) => {
            ensure!(
                path.len() > 1,
                "select the option field to edit its payload"
            );
            at_value(inner, &path[1..], op, files)
        }
        _ => Ok(0),
    }
}

fn edit_field(
    fields: &mut Fields,
    key: u32,
    op: &EditOperation,
    files: &mut BTreeMap<u64, String>,
) -> Result<u32> {
    match op {
        EditOperation::Set { value, .. } | EditOperation::Ensure { value, .. } => {
            if matches!(op, EditOperation::Ensure { .. }) && fields.contains_key(&key) {
                return Ok(0);
            }
            let next = typed_value(value)?;
            if fields.get(&key) == Some(&next) {
                return Ok(0);
            }
            remember_paths(&value.data_type, &value.value, files)?;
            fields.insert(key, next);
            Ok(1)
        }
        EditOperation::Remove { .. } => Ok(fields.shift_remove(&key).is_some() as u32),
        EditOperation::Rename { to, .. } => {
            let new = hash(to)?;
            if new == key || !fields.contains_key(&key) {
                return Ok(0);
            }
            ensure!(
                !fields.contains_key(&new),
                "rename would overwrite field {to}"
            );
            let index = fields.get_index_of(&key).unwrap();
            let value = fields.shift_remove(&key).unwrap();
            fields.shift_insert(index, new, value);
            Ok(1)
        }
        EditOperation::Convert { from, to, .. } => match fields.get_mut(&key) {
            Some(v) => convert(v, tag(from)?, tag(to)?, files),
            None => Ok(0),
        },
        EditOperation::SetClass { class, .. } => {
            let class_hash = hash(class)?;
            match fields.get_mut(&key) {
                Some(BinValue::Embed { class, .. } | BinValue::Pointer { class, .. })
                    if *class != class_hash =>
                {
                    *class = class_hash;
                    Ok(1)
                }
                _ => Ok(0),
            }
        }
        EditOperation::ReplaceString {
            pattern,
            replacement,
            ..
        } => match fields.get_mut(&key) {
            Some(value) => Ok(replace_strings(
                value,
                &regex::Regex::new(pattern)?,
                replacement,
            )),
            None => Ok(0),
        },
        _ => unreachable!(),
    }
}

fn replace_strings(value: &mut BinValue, regex: &regex::Regex, replacement: &str) -> u32 {
    match value {
        BinValue::String(text) => {
            let next = regex.replace_all(text, replacement).into_owned();
            if next == *text {
                0
            } else {
                *text = next;
                1
            }
        }
        BinValue::List { items, .. } => items
            .iter_mut()
            .map(|v| replace_strings(v, regex, replacement))
            .sum(),
        BinValue::Option { value: Some(v), .. } => replace_strings(v, regex, replacement),
        BinValue::Map { entries, .. } => entries
            .iter_mut()
            .map(|(_, v)| replace_strings(v, regex, replacement))
            .sum(),
        _ => 0,
    }
}

fn convert(
    value: &mut BinValue,
    from: BinType,
    to: BinType,
    files: &mut BTreeMap<u64, String>,
) -> Result<u32> {
    if from == to {
        return Ok(0);
    }
    match value {
        BinValue::List { item, items, .. } if *item == from => {
            for v in items {
                convert(v, from, to, files)?;
            }
            *item = to;
            return Ok(1);
        }
        BinValue::Option { item, value } if *item == from => {
            if let Some(v) = value {
                convert(v, from, to, files)?;
            }
            *item = to;
            return Ok(1);
        }
        BinValue::Map {
            value: item,
            entries,
            ..
        } if *item == from => {
            for (_, v) in entries {
                convert(v, from, to, files)?;
            }
            *item = to;
            return Ok(1);
        }
        _ => {}
    }
    if value.ty() != from {
        return Ok(0);
    }
    let text = rs_bin::value_to_text(value, None);
    let next = if to == BinType::String {
        BinValue::String(match value {
            BinValue::File(h) => files.get(h).cloned().unwrap_or(text),
            _ => text,
        })
    } else {
        rs_bin::value_from_text_as(&text, to, None)
            .context("unsupported or invalid type conversion")?
    };
    if let (BinValue::String(path), BinValue::File(h)) = (&*value, &next) {
        files.insert(*h, path.clone());
    }
    *value = next;
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn operations(value: Value) -> Vec<EditOperation> {
        serde_json::from_value(value).unwrap()
    }
    fn tree(text: &str) -> BinTree {
        let value = rs_bin::value_from_text(&format!("embed = {text}"), None).unwrap();
        let BinValue::Embed { class, fields } = value else {
            panic!()
        };
        let mut bin = rs_bin::Bin::new();
        bin.entries.push(BinEntry {
            path_hash: 1,
            class_hash: class,
            fields,
        });
        rs_bin_tree_to_hematite(bin).unwrap()
    }
    fn native(tree: &BinTree) -> rs_bin::Bin {
        hematite_tree_to_rs_bin(tree).unwrap()
    }

    #[test]
    fn file_conversion_is_scoped_and_preserves_empty_container_tags() {
        let mut tree = tree(
            r#"Root {
            children: list[embed] = {
                Migrated { texture: string = "ASSETS/a.tex", empty: list[string] = {}, optional: option[string] = {}, mapping: map[hash,string] = {} }
                Unchanged { texture: string = "ASSETS/a.tex" }
            }
            unrelated: option[vec3] = {}
        }"#,
        );
        let before = native(&tree);
        let ops = operations(
            json!([{"op":"convert", "select":{"classes":["Migrated"],"path":["*"]},"from":"string","to":"file"}]),
        );
        assert_eq!(apply(&mut tree, &ops, true).unwrap(), 4);
        assert_eq!(native(&tree), before);
        assert_eq!(apply(&mut tree, &ops, false).unwrap(), 4);
        let after = native(&tree);
        let BinValue::List { items, .. } = &after.entries[0].fields[&hash("children").unwrap()]
        else {
            panic!()
        };
        let BinValue::Embed { fields, .. } = &items[0] else {
            panic!()
        };
        let expected = rs_bin::value_from_text(r#"file = "ASSETS/a.tex""#, None).unwrap();
        assert_eq!(fields[&hash("texture").unwrap()], expected);
        assert!(matches!(
            fields[&hash("empty").unwrap()],
            BinValue::List {
                item: BinType::File,
                ..
            }
        ));
        assert!(matches!(
            fields[&hash("optional").unwrap()],
            BinValue::Option {
                item: BinType::File,
                ..
            }
        ));
        assert!(matches!(
            fields[&hash("mapping").unwrap()],
            BinValue::Map {
                key: BinType::Hash,
                value: BinType::File,
                ..
            }
        ));
        let BinValue::Embed { fields, .. } = &items[1] else {
            panic!()
        };
        assert!(matches!(
            fields[&hash("texture").unwrap()],
            BinValue::String(_)
        ));
        assert_eq!(
            after.entries[0].fields[&hash("unrelated").unwrap()],
            before.entries[0].fields[&hash("unrelated").unwrap()]
        );
        assert_eq!(apply(&mut tree, &ops, false).unwrap(), 0);
        assert_eq!(tree.recorded_files.len(), 1);
    }

    #[test]
    fn path_rename_leaves_same_named_fields_elsewhere_unchanged() {
        let mut tree = tree(
            r#"Material {
            name: string = "root"
            samplers: list[embed] = { Sampler { name: string = "texture" } }
            other: embed = Sampler { name: string = "keep" }
        }"#,
        );
        let ops = operations(
            json!([{"op":"rename","select":{"classes":["Material"],"path":["samplers","*","name"]},"to":"path"}]),
        );
        assert_eq!(apply(&mut tree, &ops, false).unwrap(), 1);
        let bin = native(&tree);
        assert!(bin.entries[0].fields.contains_key(&hash("name").unwrap()));
        let BinValue::Embed { fields, .. } = &bin.entries[0].fields[&hash("other").unwrap()] else {
            panic!()
        };
        assert!(fields.contains_key(&hash("name").unwrap()));
        assert_eq!(apply(&mut tree, &ops, false).unwrap(), 0);
    }

    #[test]
    fn creates_nested_bin_contents_from_json_without_dictionary() {
        let mut tree = BinTree::default();
        let ops = operations(
            json!([{"op":"create_entry","path":"0x1234","class":"CustomClass","fields":{
                "asset":{"type":"file","value":"assets/custom.tex"},
                "children":{"type":"list[embed]","value":[{"class":"CustomChild","fields":{"enabled":{"type":"bool","value":true}}}]},
                "empty":{"type":"option[file]","value":[]},
                "mapping":{"type":"map[hash,embed]","value":[["key",{"class":"CustomChild","fields":{}}]]},
                "color":{"type":"rgba","value":[1,2,3,4]},
                "offset":{"type":"vec3","value":[1,2,3]}
            }}]),
        );
        assert_eq!(apply(&mut tree, &ops, false).unwrap(), 1);
        let bin = native(&tree);
        assert_eq!(bin.entries[0].path_hash, 0x1234);
        assert!(matches!(
            bin.entries[0].fields[&hash("empty").unwrap()],
            BinValue::Option {
                item: BinType::File,
                value: None
            }
        ));
        assert_eq!(apply(&mut tree, &ops, false).unwrap(), 0);
    }

    #[test]
    fn failed_later_operation_rolls_back_earlier_edits() {
        let mut tree = tree("Root { a: u8 = 1, b: u8 = 2 }");
        let before = native(&tree);
        let ops = operations(json!([
            {"op":"set","select":{"path":["a"]},"value":{"type":"u8","value":9}},
            {"op":"rename","select":{"path":["a"]},"to":"b"}
        ]));
        let error = apply(&mut tree, &ops, false).unwrap_err();
        assert!(format!("{error:#}").contains("operation 2"));
        assert_eq!(native(&tree), before);
        assert!(apply(&mut tree, &ops, true).is_err());
    }

    #[test]
    fn ensure_parent_and_set_preserve_siblings_and_support_repeated_runs() {
        let ops = operations(json!([
            {"op":"ensure","select":{"classes":["Root"],"path":["parent"]},"value":{"type":"embed","value":{"class":"Parent","fields":{}}}},
            {"op":"set","select":{"classes":["Root"],"path":["parent","style"]},"value":{"type":"u8","value":12}}
        ]));
        for source in [
            "Root {}",
            "Root { parent: embed = Parent { sibling: string = \"keep\" } }",
        ] {
            let mut tree = tree(source);
            assert!(apply(&mut tree, &ops, false).unwrap() > 0);
            assert_eq!(apply(&mut tree, &ops, false).unwrap(), 0);
            let bin = native(&tree);
            let BinValue::Embed { fields, .. } = &bin.entries[0].fields[&hash("parent").unwrap()]
            else {
                panic!()
            };
            assert_eq!(fields[&hash("style").unwrap()], BinValue::U8(12));
            if source.contains("sibling") {
                assert!(fields.contains_key(&hash("sibling").unwrap()));
            }
        }
    }

    #[test]
    fn rejects_invalid_config_and_out_of_range_values() {
        assert!(serde_json::from_value::<EditOperation>(
            json!({"op":"remove","select":{"clasess":["Root"],"path":["a"]}})
        )
        .is_err());
        let mut tree = tree("Root {}");
        for (ty, value) in [
            ("u8", json!(256)),
            ("i8", json!(-129)),
            ("vec3", json!([1, 2])),
            ("bogus", json!(1)),
        ] {
            let ops = operations(
                json!([{"op":"set","select":{"path":["a"]},"value":{"type":ty,"value":value}}]),
            );
            assert!(apply(&mut tree, &ops, false).is_err(), "{ty}");
        }
    }

    #[test]
    fn selected_string_replacement_and_entry_removal_do_not_touch_other_fields() {
        let mut tree = tree(
            r#"Root { icon: string = "assets/icon.DDS", texture: string = "assets/body.dds" }"#,
        );
        let ops = operations(json!([
            {"op":"replace_string","select":{"classes":["Root"],"path":["icon"]},"pattern":"(?i)\\.dds$","replacement":".tex"}
        ]));
        assert_eq!(apply(&mut tree, &ops, false).unwrap(), 1);
        let bin = native(&tree);
        assert_eq!(
            bin.entries[0].fields[&hash("icon").unwrap()],
            BinValue::String("assets/icon.tex".into())
        );
        assert_eq!(
            bin.entries[0].fields[&hash("texture").unwrap()],
            BinValue::String("assets/body.dds".into())
        );
        assert_eq!(apply(&mut tree, &ops, false).unwrap(), 0);
        let remove = operations(json!([{"op":"remove_entry","paths":["0x1"]}]));
        assert_eq!(apply(&mut tree, &remove, false).unwrap(), 1);
        assert!(tree.objects.is_empty());
    }
    #[test]
    fn generic_conversion_handles_numbers_and_pointer_embed() {
        let mut tree = tree("Root { n: u8 = 12, child: embed = Child { flag: bool = true } }");
        let ops = operations(json!([
            {"op":"convert","select":{"path":["n"]},"from":"u8","to":"u32"},
            {"op":"convert","select":{"path":["child"]},"from":"embed","to":"pointer"},
            {"op":"set_class","select":{"path":["child"]},"class":"NewChild"}
        ]));
        assert_eq!(apply(&mut tree, &ops, false).unwrap(), 3);
        let bin = native(&tree);
        assert_eq!(
            bin.entries[0].fields[&hash("n").unwrap()],
            BinValue::U32(12)
        );
        assert!(
            matches!(bin.entries[0].fields[&hash("child").unwrap()], BinValue::Pointer { class, .. } if class == hash("NewChild").unwrap())
        );
    }
}
