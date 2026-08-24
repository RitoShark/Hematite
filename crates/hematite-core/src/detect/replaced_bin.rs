//! Replaced-BIN crash detection: the "stale or incomplete override" class.
//!
//! A mod can ship a BIN at a path the game already ships, replacing the stock file
//! wholesale instead of patching it. When that replacement was authored against an older
//! client, two things go wrong:
//!
//! - **Dropped entries.** The replacement omits by-key entries the live client still
//!   looks up. Most consumers dereference the result without checking, so the lookup
//!   miss is fatal. A few null-check it, and there the same drop only leaves the feature
//!   bugged. That difference is why severity belongs on the target, not the rule.
//! - **Added stale entries.** The replacement carries entries of a class whose layout
//!   the client has since changed, and the builder faults walking the old shape.
//!
//! This is the crash class behind most HUD and interface mods, and it is invisible to
//! every other rule in the engine: the affected BINs contain no champion or skin classes,
//! so nothing else even looks at them.
//!
//! ## Fails open, deliberately
//! No `GameProvider` means no vanilla BIN to diff against, and a BIN the game does not
//! ship is not a replacement at all. Both cases report nothing rather than guessing.
//! Reporting a crash that is not there would train users to ignore the checker.

use crate::context::FixContext;
use crate::strings::resolve_hash_token;
use hematite_types::bin::BinTree;
use hematite_types::config::{ReplacedBinMode, ReplacedBinTarget};
use std::collections::{BTreeMap, HashSet};

/// One target's finding: which class differed, and by which keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplacedBinHit {
    /// Index into the rule's `targets`, so the caller can recover the reason.
    pub target_index: usize,
    /// Entry keys responsible, worst-first is meaningless here so they stay sorted.
    pub keys: Vec<u32>,
}

impl ReplacedBinHit {
    /// Short human summary naming a few keys, for the diagnostic detail.
    pub fn describe(&self, target: &ReplacedBinTarget) -> String {
        let label = target
            .label
            .as_deref()
            .unwrap_or(target.class.as_str());
        let mode = match target.mode {
            ReplacedBinMode::Added => "added",
            ReplacedBinMode::Dropped => "dropped",
        };
        let sample: Vec<String> = self
            .keys
            .iter()
            .take(5)
            .map(|k| format!("{k:08x}"))
            .collect();
        format!(
            "{label}: {mode} {} entry key(s) [{}]",
            self.keys.len(),
            sample.join(", ")
        )
    }
}

/// Group a tree's entries by class: `class_hash -> {entry key}`.
fn entries_by_class(tree: &BinTree) -> BTreeMap<u32, HashSet<u32>> {
    let mut out: BTreeMap<u32, HashSet<u32>> = BTreeMap::new();
    for obj in tree.objects.values() {
        out.entry(obj.class_hash.0).or_default().insert(obj.path_hash.0);
    }
    out
}

/// Per-target detection, used both for the boolean verdict and for reporting.
///
/// Returns one hit per target whose class differs in the dangerous direction.
pub fn detect_hits(ctx: &FixContext, targets: &[ReplacedBinTarget]) -> Vec<ReplacedBinHit> {
    let Some(game) = ctx.game else {
        tracing::debug!("replaced_bin: no game provider, skipping {}", ctx.file_path);
        return Vec::new();
    };
    // Only a BIN the mod REPLACES is in scope. A file the game does not ship is the
    // mod's own content and has no vanilla counterpart to differ from.
    let Some(vanilla) = game.game_bin(&ctx.file_path) else {
        tracing::debug!(
            "replaced_bin: no vanilla counterpart for '{}', not a replacement",
            ctx.file_path
        );
        return Vec::new();
    };
    tracing::debug!(
        "replaced_bin: '{}' replaces a game BIN ({} vanilla entries vs {} mod entries)",
        ctx.file_path,
        vanilla.objects.len(),
        ctx.tree.objects.len()
    );

    let mod_by_class = entries_by_class(&ctx.tree);
    let game_by_class = entries_by_class(&vanilla);
    let empty = HashSet::new();

    let mut hits = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        let class = resolve_hash_token(&target.class);
        let mod_keys = mod_by_class.get(&class).unwrap_or(&empty);
        let game_keys = game_by_class.get(&class).unwrap_or(&empty);

        let mut keys: Vec<u32> = match target.mode {
            ReplacedBinMode::Added => mod_keys.difference(game_keys).copied().collect(),
            ReplacedBinMode::Dropped => game_keys.difference(mod_keys).copied().collect(),
        };

        if !target.lethal_keys.is_empty() {
            let lethal: HashSet<u32> = target
                .lethal_keys
                .iter()
                .map(|k| resolve_hash_token(k))
                .collect();
            keys.retain(|k| lethal.contains(k));
        }

        if keys.is_empty() {
            continue;
        }
        keys.sort_unstable();
        hits.push(ReplacedBinHit {
            target_index: index,
            keys,
        });
    }
    hits
}

/// Boolean verdict for the detection dispatch.
pub fn detect(ctx: &FixContext, targets: &[ReplacedBinTarget]) -> bool {
    !detect_hits(ctx, targets).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::bin::BinObject;
    use hematite_types::hash::{PathHash, TypeHash};
    use indexmap::IndexMap;

    const UI_CLASS: u32 = 0xaf9f_3bac;

    fn tree_with(entries: &[(u32, u32)]) -> BinTree {
        let mut objects = IndexMap::new();
        for (i, (class, key)) in entries.iter().enumerate() {
            objects.insert(
                i as u32,
                BinObject {
                    class_hash: TypeHash(*class),
                    path_hash: PathHash(*key),
                    properties: IndexMap::new(),
                },
            );
        }
        BinTree {
            objects,
            linked: Vec::new(),
            trailing: Vec::new(),
            trailer_files: Default::default(),
        }
    }

    fn target(mode: ReplacedBinMode, lethal: &[&str]) -> ReplacedBinTarget {
        ReplacedBinTarget {
            class: format!("0x{UI_CLASS:08x}"),
            label: Some("OptionsTab".into()),
            mode,
            lethal_keys: lethal.iter().map(|s| s.to_string()).collect(),
            reason: None,
        }
    }

    #[test]
    fn grouping_collects_keys_per_class() {
        let t = tree_with(&[(UI_CLASS, 1), (UI_CLASS, 2), (0xdead, 3)]);
        let by_class = entries_by_class(&t);
        assert_eq!(by_class[&UI_CLASS].len(), 2);
        assert_eq!(by_class[&0xdead].len(), 1);
    }

    /// A dropped key is one the vanilla BIN has and the replacement does not.
    #[test]
    fn dropped_is_game_minus_mod() {
        let game = tree_with(&[(UI_CLASS, 1), (UI_CLASS, 2), (UI_CLASS, 3)]);
        let mod_tree = tree_with(&[(UI_CLASS, 1)]);
        let g = entries_by_class(&game);
        let m = entries_by_class(&mod_tree);
        let mut dropped: Vec<u32> = g[&UI_CLASS].difference(&m[&UI_CLASS]).copied().collect();
        dropped.sort_unstable();
        assert_eq!(dropped, vec![2, 3]);
    }

    /// An added key is one the replacement invents that vanilla never had.
    #[test]
    fn added_is_mod_minus_game() {
        let game = tree_with(&[(UI_CLASS, 1)]);
        let mod_tree = tree_with(&[(UI_CLASS, 1), (UI_CLASS, 9)]);
        let g = entries_by_class(&game);
        let m = entries_by_class(&mod_tree);
        let added: Vec<u32> = m[&UI_CLASS].difference(&g[&UI_CLASS]).copied().collect();
        assert_eq!(added, vec![9]);
    }

    /// Classes that only crash on specific keys must ignore every other difference,
    /// otherwise the real signal drowns in harmless drops.
    #[test]
    fn lethal_keys_restrict_the_diff() {
        let t = target(ReplacedBinMode::Dropped, &["0x00000002"]);
        let lethal: HashSet<u32> = t
            .lethal_keys
            .iter()
            .map(|k| resolve_hash_token(k))
            .collect();
        let mut keys = vec![2u32, 3, 4];
        keys.retain(|k| lethal.contains(k));
        assert_eq!(keys, vec![2]);
    }

    #[test]
    fn describe_names_the_class_and_direction() {
        let t = target(ReplacedBinMode::Dropped, &[]);
        let hit = ReplacedBinHit {
            target_index: 0,
            keys: vec![0x14e1_17aa, 0x889a_59de],
        };
        let text = hit.describe(&t);
        assert!(text.contains("OptionsTab"), "{text}");
        assert!(text.contains("dropped 2"), "{text}");
        assert!(text.contains("14e117aa"), "{text}");
    }
}
