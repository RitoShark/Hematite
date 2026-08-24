//! Dead animation clips that never actually load.
//!
//! A dead `.anm` is normally fatal: the engine resolves the clip by literal path and
//! dereferences the result without checking. Two cases look identical to that rule and are
//! not fatal at all, and both were found by chasing false positives on real mods.
//!
//! ## A mesh particle's animation
//! A `VfxMeshDefinitionData` names a mesh, a skeleton and an animation for it. When that
//! animation is missing the mesh renders un-animated: a small visual defect, not a crash.
//! The earlier assumption was the opposite, that a complete mesh with a dead clip must
//! crash, and a mod that plays perfectly well disproved it. These are dropped outright
//! rather than downgraded, because there is nothing for a player to notice.
//!
//! ## A clip in a graph the skin does not link
//! A combined skin BIN can carry several `AnimationGraphData` objects, for instance a
//! leftover base-skin graph beside the live one. A skin links exactly one, through
//! `SkinAnimationProperties`, and clips reached only through the others never load. The
//! case that found this had a dead base-path clip sitting in an unlinked graph while the
//! linked graph used the live one.
//!
//! Both are fail-open: anything ambiguous stays reported. In particular, a BIN with no
//! identifiable link suppresses nothing, because guessing which graph is live would
//! silence real crashes.

use hematite_types::bin::{BinTree, PropertyValue};
use std::collections::{HashMap, HashSet};

/// `AnimationGraphData`. A skin loads only the graph it links.
const ANIMATION_GRAPH_DATA: u32 = 0xf5fb_07c7;
/// `VfxMeshDefinitionData`. Holds a mesh, its skeleton, and the animation to play on it.
const VFX_MESH_DEFINITION_DATA: u32 = 0x6a88_780b;

/// `mMeshName`, the `.skn`.
const FIELD_MESH_NAME: u32 = 0x8c41_a32e;
/// `mMeshSkeletonName`, the `.skl`.
const FIELD_MESH_SKELETON: u32 = 0x9059_5a15;
/// `mAnimationName`, a single `.anm`.
const FIELD_ANIMATION_NAME: u32 = 0xfbd1_6fb5;
/// `mAnimationVariants`, a list of `.anm`.
const FIELD_ANIMATION_VARIANTS: u32 = 0x147f_071c;
/// `mAnimationNames`, a list of `.anm`.
const FIELD_ANIMATION_NAMES: u32 = 0x30b2_f4b2;

/// Compare form for an animation path: lowercased, forward slashes.
pub fn normalize(path: &str) -> String {
    path.to_ascii_lowercase().replace('\\', "/")
}

fn is_anm(value: &str) -> bool {
    value.to_ascii_lowercase().ends_with(".anm")
}

/// Every `.anm` a mesh particle plays on its own mesh.
///
/// A mesh definition qualifies only when it names both a `.skn` and a `.skl` alongside the
/// clip. Without those it is not a mesh particle, and whatever the animation field holds is
/// not covered by the reasoning above.
pub fn mesh_animations(tree: &BinTree) -> HashSet<String> {
    let mut out = HashSet::new();
    for obj in tree.objects.values() {
        // A mesh definition is usually embedded in a particle definition, but nothing stops
        // it being a top-level entry of its own. Checking only the nested case would leave
        // the exemption silently not applying to whichever mod ships it that way.
        if obj.class_hash.0 == VFX_MESH_DEFINITION_DATA {
            collect_mesh_def(&obj.properties, &mut out);
        }
        for prop in obj.properties.values() {
            walk_for_mesh_defs(&prop.value, &mut out);
        }
    }
    out
}

fn walk_for_mesh_defs(value: &PropertyValue, out: &mut HashSet<String>) {
    match value {
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            if s.class_hash.0 == VFX_MESH_DEFINITION_DATA {
                collect_mesh_def(&s.properties, out);
            }
            for prop in s.properties.values() {
                walk_for_mesh_defs(&prop.value, out);
            }
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            for v in items {
                walk_for_mesh_defs(v, out);
            }
        }
        PropertyValue::Optional(inner) => {
            if let Some(v) = inner.as_ref().as_ref() {
                walk_for_mesh_defs(v, out);
            }
        }
        PropertyValue::Map(entries) => {
            for (_k, v) in entries {
                walk_for_mesh_defs(v, out);
            }
        }
        _ => {}
    }
}

fn collect_mesh_def(
    properties: &indexmap::IndexMap<u32, hematite_types::bin::BinProperty>,
    out: &mut HashSet<String>,
) {
    let has_extension = |field: u32, ext: &str| {
        matches!(
            properties.get(&field).map(|p| &p.value),
            Some(PropertyValue::String(s)) if s.to_ascii_lowercase().ends_with(ext)
        )
    };
    // Both siblings, or this is not a mesh particle and the exemption does not apply.
    if !has_extension(FIELD_MESH_NAME, ".skn") || !has_extension(FIELD_MESH_SKELETON, ".skl") {
        return;
    }

    if let Some(PropertyValue::String(s)) =
        properties.get(&FIELD_ANIMATION_NAME).map(|p| &p.value)
    {
        if is_anm(s) {
            out.insert(normalize(s));
        }
    }
    for field in [FIELD_ANIMATION_VARIANTS, FIELD_ANIMATION_NAMES] {
        let Some(prop) = properties.get(&field) else {
            continue;
        };
        if let PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) =
            &prop.value
        {
            for item in items {
                if let PropertyValue::String(s) = item {
                    if is_anm(s) {
                        out.insert(normalize(s));
                    }
                }
            }
        }
    }
}

/// Clips reachable only through an `AnimationGraphData` the BIN never links.
///
/// Returns the empty set unless the BIN holds at least two graphs AND at least one of them
/// is linked. One graph means nothing to scope out; no identifiable link means we cannot
/// tell which graph is live, and suppressing on a guess would silence real crashes.
///
/// A clip named by both a linked and an unlinked graph is NOT suppressed: it loads.
pub fn unlinked_graph_animations(tree: &BinTree) -> HashSet<String> {
    let mut graphs: HashMap<u32, HashSet<String>> = HashMap::new();
    for obj in tree.objects.values() {
        if obj.class_hash.0 != ANIMATION_GRAPH_DATA {
            continue;
        }
        let mut clips = HashSet::new();
        for prop in obj.properties.values() {
            collect_animations(&prop.value, &mut clips);
        }
        graphs.insert(obj.path_hash.0, clips);
    }
    if graphs.len() < 2 {
        return HashSet::new();
    }

    let mut linked: HashSet<u32> = HashSet::new();
    for obj in tree.objects.values() {
        for prop in obj.properties.values() {
            collect_graph_links(&prop.value, &graphs, &mut linked);
        }
    }
    if linked.is_empty() {
        return HashSet::new();
    }

    let mut live: HashSet<String> = HashSet::new();
    let mut orphaned: HashSet<String> = HashSet::new();
    for (key, clips) in &graphs {
        if linked.contains(key) {
            live.extend(clips.iter().cloned());
        } else {
            orphaned.extend(clips.iter().cloned());
        }
    }
    orphaned.difference(&live).cloned().collect()
}

fn collect_animations(value: &PropertyValue, out: &mut HashSet<String>) {
    match value {
        PropertyValue::String(s) => {
            if is_anm(s) {
                out.insert(normalize(s));
            }
        }
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            for prop in s.properties.values() {
                collect_animations(&prop.value, out);
            }
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            for v in items {
                collect_animations(v, out);
            }
        }
        PropertyValue::Optional(inner) => {
            if let Some(v) = inner.as_ref().as_ref() {
                collect_animations(v, out);
            }
        }
        PropertyValue::Map(entries) => {
            for (k, v) in entries {
                collect_animations(k, out);
                collect_animations(v, out);
            }
        }
        _ => {}
    }
}

/// Any link anywhere in the tree that points at one of these graphs.
///
/// Deliberately not restricted to `SkinAnimationProperties`: whatever holds the link, a
/// graph something points at is one that can load, and treating it as orphaned would
/// suppress a real crash.
fn collect_graph_links(
    value: &PropertyValue,
    graphs: &HashMap<u32, HashSet<String>>,
    out: &mut HashSet<u32>,
) {
    match value {
        PropertyValue::Link(key) => {
            if graphs.contains_key(key) {
                out.insert(*key);
            }
        }
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            for prop in s.properties.values() {
                collect_graph_links(&prop.value, graphs, out);
            }
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => {
            for v in items {
                collect_graph_links(v, graphs, out);
            }
        }
        PropertyValue::Optional(inner) => {
            if let Some(v) = inner.as_ref().as_ref() {
                collect_graph_links(v, graphs, out);
            }
        }
        PropertyValue::Map(entries) => {
            for (k, v) in entries {
                collect_graph_links(k, graphs, out);
                collect_graph_links(v, graphs, out);
            }
        }
        _ => {}
    }
}

/// Every clip in this BIN that is dead on paper but never loads.
pub fn never_loaded(tree: &BinTree) -> HashSet<String> {
    let mesh = mesh_animations(tree);
    let unlinked = unlinked_graph_animations(tree);
    if !mesh.is_empty() || !unlinked.is_empty() {
        tracing::debug!(
            "never loaded: {} mesh-particle clip(s), {} unlinked-graph clip(s)",
            mesh.len(),
            unlinked.len()
        );
    }
    let mut out = mesh;
    out.extend(unlinked);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::bin::{BinObject, BinProperty};
    use hematite_types::hash::{FieldHash, PathHash, TypeHash};
    use indexmap::IndexMap;

    fn prop(field: u32, value: PropertyValue) -> (u32, BinProperty) {
        (
            field,
            BinProperty {
                name_hash: FieldHash(field),
                value,
            },
        )
    }

    fn strings(values: &[&str]) -> PropertyValue {
        PropertyValue::Container(
            values
                .iter()
                .map(|v| PropertyValue::String(v.to_string()))
                .collect(),
        )
    }

    fn object(class: u32, key: u32, props: Vec<(u32, BinProperty)>) -> BinObject {
        BinObject {
            class_hash: TypeHash(class),
            path_hash: PathHash(key),
            properties: props.into_iter().collect::<IndexMap<_, _>>(),
        }
    }

    fn tree_of(objects: Vec<BinObject>) -> BinTree {
        BinTree {
            objects: objects
                .into_iter()
                .map(|o| (o.path_hash.0, o))
                .collect::<IndexMap<_, _>>(),
            ..Default::default()
        }
    }

    fn mesh_def(props: Vec<(u32, BinProperty)>) -> PropertyValue {
        PropertyValue::Embedded(hematite_types::bin::StructValue {
            class_hash: TypeHash(VFX_MESH_DEFINITION_DATA),
            properties: props.into_iter().collect::<IndexMap<_, _>>(),
        })
    }

    /// Every constant here is a number no one can check by eye, and a wrong one fails
    /// silently forever: the suppressor simply never matches and the false positive stays.
    #[test]
    fn every_hash_matches_the_name_it_claims() {
        let h = crate::strings::fnv1a_hash;
        assert_eq!(h("animationgraphdata"), ANIMATION_GRAPH_DATA);
        assert_eq!(h("vfxmeshdefinitiondata"), VFX_MESH_DEFINITION_DATA);
        assert_eq!(h("mmeshname"), FIELD_MESH_NAME);
        assert_eq!(h("mmeshskeletonname"), FIELD_MESH_SKELETON);
        assert_eq!(h("manimationname"), FIELD_ANIMATION_NAME);
        assert_eq!(h("manimationvariants"), FIELD_ANIMATION_VARIANTS);
        assert_eq!(h("manimationnames"), FIELD_ANIMATION_NAMES);
    }

    // ---- mesh particle animations -------------------------------------------------

    #[test]
    fn a_mesh_particles_clip_is_exempt() {
        let def = mesh_def(vec![
            prop(FIELD_MESH_NAME, PropertyValue::String("x/m.skn".into())),
            prop(
                FIELD_MESH_SKELETON,
                PropertyValue::String("x/m.skl".into()),
            ),
            prop(
                FIELD_ANIMATION_NAME,
                PropertyValue::String("X/Anim.ANM".into()),
            ),
        ]);
        let tree = tree_of(vec![object(1, 10, vec![prop(99, def)])]);
        assert_eq!(
            mesh_animations(&tree),
            ["x/anim.anm".to_string()].into_iter().collect()
        );
    }

    /// Every entry of a variant list is a clip for the same mesh.
    #[test]
    fn every_variant_of_a_mesh_particle_is_exempt() {
        let def = mesh_def(vec![
            prop(FIELD_MESH_NAME, PropertyValue::String("x/m.skn".into())),
            prop(FIELD_MESH_SKELETON, PropertyValue::String("x/m.skl".into())),
            prop(FIELD_ANIMATION_VARIANTS, strings(&["x/a.anm", "x/b.anm"])),
            prop(FIELD_ANIMATION_NAMES, strings(&["x/c.anm"])),
        ]);
        let tree = tree_of(vec![object(1, 10, vec![prop(99, def)])]);
        let found = mesh_animations(&tree);
        assert_eq!(found.len(), 3);
        assert!(found.contains("x/b.anm"));
        assert!(found.contains("x/c.anm"));
    }

    /// Without both siblings this is not a mesh particle, and the reasoning does not hold.
    #[test]
    fn a_definition_missing_its_skeleton_is_not_exempt() {
        let def = mesh_def(vec![
            prop(FIELD_MESH_NAME, PropertyValue::String("x/m.skn".into())),
            prop(
                FIELD_ANIMATION_NAME,
                PropertyValue::String("x/anim.anm".into()),
            ),
        ]);
        let tree = tree_of(vec![object(1, 10, vec![prop(99, def)])]);
        assert!(mesh_animations(&tree).is_empty());
    }

    /// A field carrying the same hash on some other class must not exempt anything.
    #[test]
    fn an_animation_field_outside_a_mesh_definition_is_not_exempt() {
        let tree = tree_of(vec![object(
            1,
            10,
            vec![prop(
                FIELD_ANIMATION_NAME,
                PropertyValue::String("x/anim.anm".into()),
            )],
        )]);
        assert!(mesh_animations(&tree).is_empty());
    }

    // ---- unlinked graphs ----------------------------------------------------------

    fn graph(key: u32, clips: &[&str]) -> BinObject {
        object(
            ANIMATION_GRAPH_DATA,
            key,
            vec![prop(0x1111, strings(clips))],
        )
    }

    #[test]
    fn a_clip_only_in_an_unlinked_graph_is_suppressed() {
        let tree = tree_of(vec![
            graph(100, &["x/live.anm"]),
            graph(200, &["x/orphan.anm"]),
            object(0xabcd, 300, vec![prop(0x2222, PropertyValue::Link(100))]),
        ]);
        assert_eq!(
            unlinked_graph_animations(&tree),
            ["x/orphan.anm".to_string()].into_iter().collect()
        );
    }

    /// A clip both graphs name still loads through the linked one.
    #[test]
    fn a_clip_the_linked_graph_also_names_is_kept() {
        let tree = tree_of(vec![
            graph(100, &["x/shared.anm"]),
            graph(200, &["x/shared.anm", "x/orphan.anm"]),
            object(0xabcd, 300, vec![prop(0x2222, PropertyValue::Link(100))]),
        ]);
        let suppressed = unlinked_graph_animations(&tree);
        assert!(!suppressed.contains("x/shared.anm"));
        assert!(suppressed.contains("x/orphan.anm"));
    }

    /// One graph is the common case and there is nothing to scope out.
    #[test]
    fn a_single_graph_suppresses_nothing() {
        let tree = tree_of(vec![
            graph(100, &["x/a.anm"]),
            object(0xabcd, 300, vec![prop(0x2222, PropertyValue::Link(100))]),
        ]);
        assert!(unlinked_graph_animations(&tree).is_empty());
    }

    /// Not knowing which graph is live must not silence anything.
    #[test]
    fn no_identifiable_link_suppresses_nothing() {
        let tree = tree_of(vec![graph(100, &["x/a.anm"]), graph(200, &["x/b.anm"])]);
        assert!(unlinked_graph_animations(&tree).is_empty());
    }

    /// A link to something that is not a graph tells us nothing about the graphs.
    #[test]
    fn a_link_to_a_non_graph_is_not_a_graph_link() {
        let tree = tree_of(vec![
            graph(100, &["x/a.anm"]),
            graph(200, &["x/b.anm"]),
            object(0xabcd, 300, vec![prop(0x2222, PropertyValue::Link(999))]),
        ]);
        assert!(unlinked_graph_animations(&tree).is_empty());
    }
}
