//! Stale champion record: a mod shipping a character root older than the live schema.
//!
//! A mod that replaces `data/characters/<champ>/<champ>.bin` pins that champion's record
//! to whatever the client looked like when the mod was built. Riot keeps adding fields,
//! so an old record is missing bindings the current client expects. What that costs
//! depends entirely on which field is gone:
//!
//! - no `spells` binding: the champion has no abilities at all and cannot be played;
//! - missing stat fields: the champion casts fine but carries wrong base stats.
//!
//! Same detection, two very different verdicts, which is why the discriminating field is
//! named in config rather than inferred from how many fields are absent.
//!
//! ## Differential, not absolute
//! A field is only missing if the LIVE record has it and the mod's does not. Comparing
//! against a fixed expected list would turn every Riot schema removal into a finding on
//! every mod at once.
//!
//! ## Fail-open
//! No game install, no counterpart record, or an unreadable BIN on either side means no
//! comparison is possible and nothing is reported.

use crate::context::FixContext;
use crate::strings::resolve_hash_token;
use hematite_types::bin::BinTree;
use std::collections::HashSet;

/// Which declared fields the live record has and the mod's does not.
///
/// `entry_type` and `fields` accept names or `0x…` hex hashes. Returns an empty vector
/// whenever the comparison cannot be made, which callers must treat as "no finding"
/// rather than "nothing missing".
pub fn missing_fields<'a>(
    ctx: &FixContext,
    entry_type: &str,
    fields: &'a [String],
) -> Vec<&'a String> {
    let Some(game) = ctx.game else {
        return Vec::new();
    };
    // Only a BIN the game also ships is a replacement of a live record; anything else is
    // the mod's own content and has no schema to be behind.
    let Some(vanilla) = game.game_bin(&ctx.file_path) else {
        return Vec::new();
    };

    let class = resolve_hash_token(entry_type);
    let (Some(mod_fields), Some(game_fields)) = (
        record_fields(&ctx.tree, class),
        record_fields(&vanilla, class),
    ) else {
        return Vec::new();
    };

    fields
        .iter()
        .filter(|f| {
            let h = resolve_hash_token(f);
            game_fields.contains(&h) && !mod_fields.contains(&h)
        })
        .collect()
}

/// Top-level property hashes of the first object of `class`, or `None` if absent.
fn record_fields(tree: &BinTree, class: u32) -> Option<HashSet<u32>> {
    tree.objects
        .values()
        .find(|o| o.class_hash.0 == class)
        .map(|o| o.properties.keys().copied().collect())
}

/// Whether a BIN is a character's root record, e.g. `data/characters/jhin/jhin.bin`.
///
/// The record lives at a path named after its own character, which distinguishes it from
/// the skin and animation BINs in the same folder.
pub fn is_character_root(file_path: &str) -> bool {
    let lower = file_path.to_lowercase().replace('\\', "/");
    let Some(rest) = lower
        .strip_prefix("data/characters/")
        .or_else(|| lower.strip_prefix("assets/characters/"))
    else {
        return false;
    };
    let mut parts = rest.split('/');
    let (Some(character), Some(file), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !character.is_empty() && file == format!("{character}.bin")
}

/// Boolean verdict for the detection dispatch.
pub fn detect(ctx: &FixContext, entry_type: &str, fields: &[String]) -> bool {
    is_character_root(&ctx.file_path) && !missing_fields(ctx, entry_type, fields).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::bin::BinObject;
    use hematite_types::hash::{PathHash, TypeHash};
    use indexmap::IndexMap;

    fn tree_with(class: u32, field_hashes: &[u32]) -> BinTree {
        let mut props = IndexMap::new();
        for h in field_hashes {
            props.insert(
                *h,
                hematite_types::bin::BinProperty {
                    name_hash: hematite_types::hash::FieldHash(*h),
                    value: hematite_types::bin::PropertyValue::U32(0),
                },
            );
        }
        let mut objects = IndexMap::new();
        objects.insert(
            1,
            BinObject {
                class_hash: TypeHash(class),
                path_hash: PathHash(1),
                properties: props,
            },
        );
        BinTree {
            objects,
            linked: Vec::new(),
            trailing: Vec::new(),
            trailer_files: Default::default(),
        }
    }

    #[test]
    fn recognises_a_character_root() {
        assert!(is_character_root("data/characters/jhin/jhin.bin"));
        assert!(is_character_root("DATA/Characters/Jhin/Jhin.bin"));
    }

    /// Skin and animation BINs live in the same folder and must not be mistaken for the
    /// record, or every skin would be compared against a record it is not.
    #[test]
    fn rejects_skin_and_animation_bins() {
        assert!(!is_character_root("data/characters/jhin/skins/skin0.bin"));
        assert!(!is_character_root("data/characters/jhin/animations/skin0.bin"));
        assert!(!is_character_root("data/characters/jhin/other.bin"));
        assert!(!is_character_root("00df3e4432caeaa8"));
    }

    #[test]
    fn a_field_is_missing_only_when_the_live_record_has_it() {
        const CLASS: u32 = 0x23ea_1915;
        const SPELLS: u32 = 0x74f5_d6ce;
        const SPEED: u32 = 0xe62d_9d92;

        let game = record_fields(&tree_with(CLASS, &[SPELLS, SPEED]), CLASS).unwrap();
        let modded = record_fields(&tree_with(CLASS, &[SPEED]), CLASS).unwrap();
        assert!(game.contains(&SPELLS) && !modded.contains(&SPELLS));
        // Present on both sides, so not drift.
        assert!(game.contains(&SPEED) && modded.contains(&SPEED));
    }

    /// A field the live record also lacks is not drift: otherwise a Riot removal would
    /// flag every mod in existence at once.
    #[test]
    fn a_field_absent_from_both_is_not_missing() {
        const CLASS: u32 = 0x23ea_1915;
        const GONE: u32 = 0xdead_beef;
        let game = record_fields(&tree_with(CLASS, &[]), CLASS).unwrap();
        let modded = record_fields(&tree_with(CLASS, &[]), CLASS).unwrap();
        assert!(!(game.contains(&GONE) && !modded.contains(&GONE)));
    }

    #[test]
    fn no_record_of_that_class_yields_nothing() {
        assert!(record_fields(&tree_with(0x1111, &[0x2222]), 0x23ea_1915).is_none());
    }
}
