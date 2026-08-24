//! RegexReplace + RegexRenameField transforms.
//!
//! ## RegexReplace
//! Pattern-based string replacement in string fields.
//! Uses PropertyWalker with `visit_string` for recursive scanning.
//!
//! ## RegexRenameField
//! Regex-based field rename with capture group support.
//! Computes new FNV-1a hash from the renamed field name.

use crate::context::FixContext;
use crate::strings::fnv1a_hash;
use crate::walk::{walk_tree, PropertyVisitor, VisitResult};
use hematite_types::hash::FieldHash;
use regex::Regex;
use std::collections::HashSet;

struct RegexReplacer {
    pattern: Regex,
    matching_hashes: Option<HashSet<u32>>,
    replacement: String,
}

impl PropertyVisitor for RegexReplacer {
    fn visit_string(&mut self, value: &str, hash: FieldHash) -> VisitResult {
        if let Some(ref hashes) = self.matching_hashes {
            if !hashes.contains(&hash.0) {
                return VisitResult::Skip;
            }
        }

        if self.pattern.is_match(value) {
            let new_val = self
                .pattern
                .replace_all(value, &*self.replacement)
                .to_string();
            if new_val != value {
                return VisitResult::Mutate(new_val);
            }
        }

        VisitResult::Skip
    }
}

pub fn apply_replace(
    ctx: &mut FixContext,
    pattern_str: &str,
    replacement: &str,
    field_filter: Option<&str>,
) -> u32 {
    let Ok(pattern) = Regex::new(pattern_str) else {
        return 0;
    };

    let matching_hashes = if let Some(filter_str) = field_filter {
        let Ok(filter) = Regex::new(filter_str) else {
            return 0;
        };

        let mut hashes = HashSet::new();
        for obj in ctx.tree.objects.values() {
            for hash in obj.properties.keys() {
                if let Some(field_name) = ctx.hashes.resolve_field(FieldHash(*hash)) {
                    if filter.is_match(&field_name) {
                        hashes.insert(*hash);
                    }
                }
            }
        }
        Some(hashes)
    } else {
        None
    };

    let mut visitor = RegexReplacer {
        pattern,
        matching_hashes,
        replacement: replacement.to_string(),
    };

    walk_tree(&mut ctx.tree, &mut visitor)
}

pub fn apply_rename(ctx: &mut FixContext, pattern_str: &str, replacement: &str) -> u32 {
    let Ok(pattern) = Regex::new(pattern_str) else {
        return 0;
    };

    let mut changes = 0u32;

    for obj in ctx.tree.objects.values_mut() {
        let renames: Vec<(u32, u32, String)> = obj
            .properties
            .keys()
            .filter_map(|&hash| {
                let field_name = ctx.hashes.resolve_field(FieldHash(hash))?;
                if pattern.is_match(&field_name) {
                    let new_name = pattern.replace(&field_name, replacement).to_string();
                    let new_hash = fnv1a_hash(&new_name);
                    Some((hash, new_hash, new_name))
                } else {
                    None
                }
            })
            .collect();

        for (old_hash, new_hash, _new_name) in renames {
            if let Some(mut prop) = obj.properties.swap_remove(&old_hash) {
                prop.name_hash = FieldHash(new_hash);
                obj.properties.insert(new_hash, prop);
                changes += 1;
            }
        }
    }

    changes
}
