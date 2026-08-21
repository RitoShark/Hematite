//! SkinLite clone tree — faithful port of Celestial's `clone_skin_bin`
//! (`noskin/mod.rs`): SkinLite dupes skin0.bin into slots 1..99, patching
//! only the SCDP entry key, the ResourceResolver entry key, and the SCDP's
//! mResourceResolver field. The port must stay byte-faithful (including the
//! shift_remove + insert-at-end rekey order) — detection compares our clone
//! of skin0 against the mod's slot bins byte-for-byte.

use crate::strings::fnv1a_hash;
use hematite_types::bin::{BinProperty, BinTree, PropertyValue};
use hematite_types::hash::{FieldHash, PathHash};

fn rekey(tree: &mut BinTree, old: u32, new: u32) -> Option<()> {
    if old == new {
        return Some(());
    }
    let mut obj = tree.objects.shift_remove(&old)?;
    obj.path_hash = PathHash(new);
    tree.objects.insert(new, obj);
    Some(())
}

/// Clone a skin0 tree into the given skin slot. `None` when the tree has no
/// SkinCharacterDataProperties entry (not a skin BIN).
pub fn clone_skin_tree(source: &BinTree, champ: &str, slot: u32) -> Option<BinTree> {
    let scdp_type = fnv1a_hash("skincharacterdataproperties");
    let rr_type = fnv1a_hash("resourceresolver");
    let mrr_field = fnv1a_hash("mresourceresolver");

    let base_scdp = source
        .objects
        .iter()
        .find_map(|(k, o)| (o.class_hash.0 == scdp_type).then_some(*k))?;
    let base_rr = source
        .objects
        .iter()
        .find_map(|(k, o)| (o.class_hash.0 == rr_type).then_some(*k));

    let mut tree = source.clone();

    let new_scdp = fnv1a_hash(&format!("characters/{champ}/skins/skin{slot}"));
    rekey(&mut tree, base_scdp, new_scdp)?;

    let mut new_rr = None;
    if let Some(rr_old) = base_rr {
        let h = fnv1a_hash(&format!("characters/{champ}/skins/skin{slot}/resources"));
        rekey(&mut tree, rr_old, h)?;
        new_rr = Some(h);
    }

    if let Some(obj) = tree.objects.get_mut(&new_scdp) {
        if let Some(prop) = obj.properties.get_mut(&mrr_field) {
            if let Some(rr) = new_rr {
                prop.value = match prop.value {
                    PropertyValue::String(_) => PropertyValue::String(format!(
                        "Characters/{champ}/Skins/Skin{slot}/Resources"
                    )),
                    _ => PropertyValue::Link(rr),
                };
            }
        } else if let Some(rr) = new_rr {
            obj.properties.insert(
                mrr_field,
                BinProperty {
                    name_hash: FieldHash(mrr_field),
                    value: PropertyValue::Link(rr),
                },
            );
        }
    }

    Some(tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::bin::BinObject;
    use indexmap::IndexMap;

    fn skin0_tree(champ: &str, with_rr: bool, mrr_as_string: bool) -> BinTree {
        let scdp_key = fnv1a_hash(&format!("characters/{champ}/skins/skin0"));
        let rr_key = fnv1a_hash(&format!("characters/{champ}/skins/skin0/resources"));
        let mrr = fnv1a_hash("mresourceresolver");

        let mut properties = IndexMap::new();
        properties.insert(
            0x11,
            BinProperty {
                name_hash: FieldHash(0x11),
                value: PropertyValue::U32(1),
            },
        );
        if with_rr {
            properties.insert(
                mrr,
                BinProperty {
                    name_hash: FieldHash(mrr),
                    value: if mrr_as_string {
                        PropertyValue::String(format!("Characters/{champ}/Skins/Skin0/Resources"))
                    } else {
                        PropertyValue::Link(rr_key)
                    },
                },
            );
        }

        let mut objects = IndexMap::new();
        objects.insert(
            scdp_key,
            BinObject {
                class_hash: hematite_types::hash::TypeHash(fnv1a_hash(
                    "skincharacterdataproperties",
                )),
                path_hash: PathHash(scdp_key),
                properties,
            },
        );
        if with_rr {
            objects.insert(
                rr_key,
                BinObject {
                    class_hash: hematite_types::hash::TypeHash(fnv1a_hash("resourceresolver")),
                    path_hash: PathHash(rr_key),
                    properties: IndexMap::new(),
                },
            );
        }
        BinTree {
            objects,
            ..Default::default()
        }
    }

    #[test]
    fn clone_rekeys_scdp_and_rr_and_patches_link() {
        let tree = skin0_tree("kayn", true, false);
        let clone = clone_skin_tree(&tree, "kayn", 5).expect("clone");

        let scdp5 = fnv1a_hash("characters/kayn/skins/skin5");
        let rr5 = fnv1a_hash("characters/kayn/skins/skin5/resources");
        assert!(clone.objects.contains_key(&scdp5));
        assert!(clone.objects.contains_key(&rr5));
        assert!(!clone
            .objects
            .contains_key(&fnv1a_hash("characters/kayn/skins/skin0")));

        let obj = clone.objects.get(&scdp5).unwrap();
        assert_eq!(obj.path_hash.0, scdp5);
        match &obj
            .properties
            .get(&fnv1a_hash("mresourceresolver"))
            .unwrap()
            .value
        {
            PropertyValue::Link(h) => assert_eq!(*h, rr5),
            other => panic!("expected link, got {other:?}"),
        }
    }

    #[test]
    fn clone_patches_string_form_mrr() {
        let tree = skin0_tree("kayn", true, true);
        let clone = clone_skin_tree(&tree, "kayn", 12).expect("clone");
        let obj = clone
            .objects
            .get(&fnv1a_hash("characters/kayn/skins/skin12"))
            .unwrap();
        match &obj
            .properties
            .get(&fnv1a_hash("mresourceresolver"))
            .unwrap()
            .value
        {
            PropertyValue::String(s) => {
                assert_eq!(s, "Characters/kayn/Skins/Skin12/Resources")
            }
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[test]
    fn clone_without_rr_leaves_mrr_alone() {
        let tree = skin0_tree("kayn", false, false);
        let clone = clone_skin_tree(&tree, "kayn", 3).expect("clone");
        let obj = clone
            .objects
            .get(&fnv1a_hash("characters/kayn/skins/skin3"))
            .unwrap();
        assert!(!obj
            .properties
            .contains_key(&fnv1a_hash("mresourceresolver")));
    }

    #[test]
    fn no_scdp_means_no_clone() {
        let tree = BinTree::default();
        assert!(clone_skin_tree(&tree, "kayn", 1).is_none());
    }

    #[test]
    fn rekey_moves_object_to_end_like_celestial() {
        // Celestial's replace_object_key = shift_remove + insert, which moves
        // the rekeyed object to the END of the map. Byte-equality detection
        // depends on replicating that exactly.
        let tree = skin0_tree("kayn", true, false);
        let clone = clone_skin_tree(&tree, "kayn", 7).expect("clone");
        let keys: Vec<u32> = clone.objects.keys().copied().collect();
        assert_eq!(
            keys,
            vec![
                fnv1a_hash("characters/kayn/skins/skin7"),
                fnv1a_hash("characters/kayn/skins/skin7/resources"),
            ]
        );
    }
}
