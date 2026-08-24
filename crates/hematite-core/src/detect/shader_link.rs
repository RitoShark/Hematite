//! Dead shader-link detection: the "outdated material" crash.
//!
//! A `StaticMaterialDef` links its shader by entry key. When a patch renames or removes
//! a shader, a mod built against the old name links an entry that no longer exists. The
//! engine's resolver returns null and the game goes down with it, with no error code
//! attached, so the crash cannot be recognised from the client log afterwards. Catching
//! it before the mod is installed is the only place it can be caught at all.
//!
//! ## Why this is scoped to one property
//! BINs are full of links that legitimately point outside the mod: interface elements to
//! other interface objects, particles, base-game objects. Those resolve fine. Collecting
//! every link and flagging whatever is not a known shader would mark essentially every
//! interface and skin mod as crashing. So only links under the `shader` property of a
//! `StaticMaterialDef` count, and a BIN with no materials yields no candidates at all.
//!
//! ## Fail-open
//! A link is reported ONLY when the game's shader set was read successfully and the link
//! is in neither that set nor the mod's own objects. If the shader data cannot be read,
//! nothing is reported: "could not validate" must never render as "broken", or the first
//! false crash report teaches users to ignore the checker.

use crate::context::FixContext;
use hematite_types::bin::{BinTree, PropertyValue};
use std::collections::HashSet;

/// FNV-1a of `shader`: the property a material links its shader through.
pub const SHADER_PROP_HASH: u32 = 0x355d_5568;

/// FNV-1a of `StaticMaterialDef`: the only class whose `shader` link is a shader link.
pub const STATIC_MATERIAL_DEF: u32 = 0xff9d_3409;

/// Shader links the mod references, from `StaticMaterialDef` objects only.
///
/// Walks nested values because a material's passes sit in embedded structs and
/// containers rather than directly on the object.
fn shader_links(tree: &BinTree) -> HashSet<u32> {
    let mut out = HashSet::new();
    for obj in tree.objects.values() {
        if obj.class_hash.0 != STATIC_MATERIAL_DEF {
            continue;
        }
        for (name_hash, prop) in &obj.properties {
            collect(*name_hash, &prop.value, &mut out);
        }
    }
    out
}

fn collect(name_hash: u32, value: &PropertyValue, out: &mut HashSet<u32>) {
    if name_hash == SHADER_PROP_HASH {
        if let PropertyValue::Link(h) = value {
            if *h != 0 {
                out.insert(*h);
            }
        }
    }
    match value {
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            for (child_name, child) in &s.properties {
                collect(*child_name, &child.value, out);
            }
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            for v in items {
                // Container elements keep the parent's field identity: a list of passes
                // under `shader` is still the shader property.
                collect(name_hash, v, out);
            }
        }
        PropertyValue::Optional(inner) => {
            if let Some(v) = inner.as_ref().as_ref() {
                collect(name_hash, v, out);
            }
        }
        PropertyValue::Map(entries) => {
            for (_k, v) in entries {
                collect(name_hash, v, out);
            }
        }
        _ => {}
    }
}

/// Entry keys the mod defines itself. A link to one of these resolves inside the mod and
/// is not dead, even though the game has never heard of it.
fn owned_keys(tree: &BinTree) -> HashSet<u32> {
    tree.objects.values().map(|o| o.path_hash.0).collect()
}

/// Shader links that resolve nowhere: not in the game, not in the mod.
///
/// Empty when the game's shader set is unavailable, which is the fail-open case rather
/// than a clean result. Use [`can_validate`] to tell the two apart.
pub fn dead_links(ctx: &FixContext) -> Vec<u32> {
    let Some(game) = ctx.game else {
        return Vec::new();
    };
    let Some(valid) = game.shader_defs() else {
        return Vec::new();
    };
    if valid.is_empty() {
        return Vec::new();
    }

    let links = shader_links(&ctx.tree);
    if links.is_empty() {
        return Vec::new();
    }
    let owned = owned_keys(&ctx.tree);

    let mut dead: Vec<u32> = links
        .into_iter()
        .filter(|h| !valid.contains(h) && !owned.contains(h))
        .collect();
    dead.sort_unstable();
    dead
}

/// Whether the shader set is available at all. `false` means the check cannot run.
pub fn can_validate(ctx: &FixContext) -> bool {
    ctx.game
        .and_then(|g| g.shader_defs())
        .is_some_and(|s| !s.is_empty())
}

/// Boolean verdict for the detection dispatch.
pub fn detect(ctx: &FixContext) -> bool {
    !dead_links(ctx).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::bin::{BinObject, BinProperty, StructValue};
    use hematite_types::hash::{FieldHash, PathHash, TypeHash};
    use indexmap::IndexMap;

    fn prop(name: u32, value: PropertyValue) -> (u32, BinProperty) {
        (
            name,
            BinProperty {
                name_hash: FieldHash(name),
                value,
            },
        )
    }

    fn tree_of(objects: Vec<(u32, u32, Vec<(u32, BinProperty)>)>) -> BinTree {
        let mut map = IndexMap::new();
        for (key, class, props) in objects {
            map.insert(
                key,
                BinObject {
                    class_hash: TypeHash(class),
                    path_hash: PathHash(key),
                    properties: props.into_iter().collect(),
                },
            );
        }
        BinTree {
            objects: map,
            linked: Vec::new(),
            trailing: Vec::new(),
            trailer_files: Default::default(),
        }
    }

    #[test]
    fn finds_a_direct_shader_link() {
        let t = tree_of(vec![(
            1,
            STATIC_MATERIAL_DEF,
            vec![prop(SHADER_PROP_HASH, PropertyValue::Link(0xabcd))],
        )]);
        assert_eq!(shader_links(&t), HashSet::from([0xabcd]));
    }

    /// Material passes are nested, so a walk that only looks at top-level properties
    /// would find nothing on a real material.
    #[test]
    fn finds_a_nested_shader_link() {
        let inner = StructValue {
            class_hash: TypeHash(0x1234),
            properties: vec![prop(SHADER_PROP_HASH, PropertyValue::Link(0xbeef))]
                .into_iter()
                .collect(),
        };
        let t = tree_of(vec![(
            1,
            STATIC_MATERIAL_DEF,
            vec![prop(0x9999, PropertyValue::Embedded(inner))],
        )]);
        assert_eq!(shader_links(&t), HashSet::from([0xbeef]));
    }

    /// The scoping rule that keeps interface mods from being mass-flagged: a link on
    /// any other class, or under any other property, is not a shader link.
    #[test]
    fn ignores_links_outside_static_material_def() {
        let t = tree_of(vec![(
            1,
            0xdead_0000,
            vec![prop(SHADER_PROP_HASH, PropertyValue::Link(0xabcd))],
        )]);
        assert!(shader_links(&t).is_empty());
    }

    #[test]
    fn ignores_other_properties_on_a_material() {
        let t = tree_of(vec![(
            1,
            STATIC_MATERIAL_DEF,
            vec![prop(0x1111_2222, PropertyValue::Link(0xabcd))],
        )]);
        assert!(shader_links(&t).is_empty());
    }

    #[test]
    fn a_null_link_is_not_a_link() {
        let t = tree_of(vec![(
            1,
            STATIC_MATERIAL_DEF,
            vec![prop(SHADER_PROP_HASH, PropertyValue::Link(0))],
        )]);
        assert!(shader_links(&t).is_empty());
    }

    /// A mod that ships its own shader definition links to itself, and that resolves.
    #[test]
    fn mod_owned_keys_are_collected() {
        let t = tree_of(vec![
            (
                0x5555,
                STATIC_MATERIAL_DEF,
                vec![prop(SHADER_PROP_HASH, PropertyValue::Link(0x7777))],
            ),
            (0x7777, 0xaaaa, vec![]),
        ]);
        assert!(owned_keys(&t).contains(&0x7777));
    }
}
