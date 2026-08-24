//! Which BINs a mod actually loads.
//!
//! A mod built by cloning a champion WAD carries skin BINs it never wires up. Those
//! orphans are never loaded, so a dead reference inside one cannot crash anything.
//! Reporting them produces findings for defects that can never occur, on mods that work
//! fine, which is the fastest way to make a crash check worthless.
//!
//! ## The graph is one level deep, deliberately
//! Roots are the skin BINs the WAD actually ships. Edges are each root's declared
//! dependency list, followed exactly once. There is no transitive closure: a dependency
//! of a dependency does not count as loaded. This mirrors what the client does when it
//! resolves a skin, and widening it would quietly re-admit the orphans this exists to
//! exclude.
//!
//! ## Fail open, always
//! [`compute`] returns `None` when it cannot establish what loads, and callers must then
//! scan everything. Reachability may only ever SHRINK the set of BINs considered, never
//! grow it and never by itself cause a finding. Getting this backwards turns an
//! unreadable mod into a clean bill of health.

use crate::traits::BinProvider;
use std::collections::{HashMap, HashSet};

/// Highest skin slot probed. Riot has never shipped anywhere near this many.
const MAX_SLOT: u32 = 200;

/// What a mod's shipped skins pull in.
#[derive(Debug, Clone, Default)]
pub struct Reachability {
    /// Chunk path hashes (xxh64 of the lowercased path) that load: every shipped skin
    /// root, plus each root's declared dependencies.
    ///
    /// A dependency's hash is recorded even when the WAD does not ship that chunk. The
    /// question this answers is "would this load", not "is this present".
    pub reachable: HashSet<u64>,
    /// Skin root chunk hash to its slot number, for roots actually present.
    ///
    /// Lets a finding say which skin selection triggers it instead of only that
    /// something is broken somewhere.
    pub skin_slot: HashMap<u64, u32>,
}

impl Reachability {
    /// Whether a BIN loads. Kept as a method so the fail-open case reads the same at
    /// every call site: `reach.map_or(true, |r| r.loads(h))`.
    pub fn loads(&self, chunk_hash: u64) -> bool {
        self.reachable.contains(&chunk_hash)
    }

    /// Slot number when this chunk is a skin root.
    pub fn slot_of(&self, chunk_hash: u64) -> Option<u32> {
        self.skin_slot.get(&chunk_hash).copied()
    }
}

/// Canonical animation BIN path for a champion slot.
///
/// Animation BINs are never roots, only dependencies, so a check that wants to inspect
/// one has to recognise it explicitly. Used for the animation exemption described on
/// [`animation_bin_slots`].
pub fn animation_bin_path(champion: &str, slot: u32) -> String {
    format!("data/characters/{champion}/animations/skin{slot}.bin")
}

/// Skin BIN path for a champion slot.
pub fn skin_bin_path(champion: &str, slot: u32) -> String {
    format!("data/characters/{champion}/skins/skin{slot}.bin")
}

/// Map of animation-BIN chunk hash to slot, for every slot of one champion.
///
/// An animation BIN reached by no loaded skin is *latent*: it cannot crash as the mod is
/// used, but selecting the original skin it belongs to would. That is a warning rather
/// than a crash, and distinguishing the two requires letting these BINs past the
/// reachability gate instead of dropping them.
pub fn animation_bin_slots(champion: &str, hash: impl Fn(&str) -> u64) -> HashMap<u64, u32> {
    (0..=MAX_SLOT)
        .map(|n| (hash(&animation_bin_path(champion, n)), n))
        .collect()
}

/// Determine what loads.
///
/// `present` answers whether the WAD ships a chunk hash. `dependencies` returns a BIN's
/// declared dependency paths, or `None` when it cannot be read. `hash` converts a path
/// to its chunk hash. Passing these in keeps this module free of WAD and BIN format
/// knowledge, matching the crate's no-format-imports rule.
///
/// Returns `None` when no skin root could be read at all, which callers must treat as
/// "scan everything".
pub fn compute(
    champions: &[&str],
    hash: impl Fn(&str) -> u64,
    present: impl Fn(u64) -> bool,
    dependencies: impl Fn(u64) -> Option<Vec<String>>,
) -> Option<Reachability> {
    let mut out = Reachability::default();
    let mut read_a_root = false;

    for champ in champions {
        // Probe slot 0 before sweeping 200 slots. This is not only a shortcut: a mod
        // shipping some other slot without skin0 is not a coherent skin for this
        // champion, and treating it as one would seed reachability from a BIN the client
        // would not load either.
        if !present(hash(&skin_bin_path(champ, 0))) {
            continue;
        }

        for n in 0..=MAX_SLOT {
            let root = hash(&skin_bin_path(champ, n));
            if !present(root) {
                continue;
            }

            // The root loads whether or not its contents can be read. Recording it only
            // on a successful read would drop a shipped skin from the loaded set because
            // of an unrelated parse failure.
            out.reachable.insert(root);
            out.skin_slot.insert(root, n);

            let Some(deps) = dependencies(root) else {
                continue;
            };
            read_a_root = true;
            for dep in deps {
                out.reachable.insert(hash(&dep));
            }
        }
    }

    // Roots were found but none could be read, so the dependency half of the graph is
    // missing entirely. A partial answer here would silently exclude every dependency
    // BIN, so discard it and let the caller scan everything.
    if !read_a_root {
        return None;
    }
    Some(out)
}

/// Read a BIN's declared dependencies through a [`BinProvider`].
///
/// Convenience for callers that already hold parsed bytes; `compute` takes a closure so
/// it never needs a provider itself.
pub fn dependencies_of(bin: &dyn BinProvider, bytes: &[u8]) -> Option<Vec<String>> {
    bin.parse_bytes(bytes).ok().map(|tree| tree.linked)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-in hash: the path itself, so tests read as paths rather than numbers.
    fn h(path: &str) -> u64 {
        let mut acc: u64 = 1469598103934665603;
        for b in path.to_lowercase().bytes() {
            acc ^= b as u64;
            acc = acc.wrapping_mul(1099511628211);
        }
        acc
    }

    fn present_set(paths: &[&str]) -> HashSet<u64> {
        paths.iter().map(|p| h(p)).collect()
    }

    #[test]
    fn roots_and_their_dependencies_load() {
        let present = present_set(&["data/characters/jhin/skins/skin0.bin"]);
        let r = compute(
            &["jhin"],
            h,
            |x| present.contains(&x),
            |_| Some(vec!["data/characters/jhin/animations/skin0.bin".into()]),
        )
        .expect("a readable root");

        assert!(r.loads(h("data/characters/jhin/skins/skin0.bin")));
        assert!(r.loads(h("data/characters/jhin/animations/skin0.bin")));
        assert_eq!(r.slot_of(h("data/characters/jhin/skins/skin0.bin")), Some(0));
    }

    /// The orphan case this module exists for: a cloned skin the mod never ships as a
    /// root, referenced by nothing, must not load.
    #[test]
    fn unshipped_skins_do_not_load() {
        let present = present_set(&["data/characters/jhin/skins/skin0.bin"]);
        let r = compute(&["jhin"], h, |x| present.contains(&x), |_| Some(vec![])).unwrap();
        assert!(!r.loads(h("data/characters/jhin/skins/skin7.bin")));
    }

    /// One level only. A dependency's own dependencies are not followed, so widening the
    /// walk cannot quietly re-admit orphans.
    #[test]
    fn the_walk_is_one_level_deep() {
        let present = present_set(&["data/characters/jhin/skins/skin0.bin"]);
        let r = compute(
            &["jhin"],
            h,
            |x| present.contains(&x),
            |chunk| {
                if chunk == h("data/characters/jhin/skins/skin0.bin") {
                    Some(vec!["data/characters/jhin/first.bin".into()])
                } else {
                    Some(vec!["data/characters/jhin/second.bin".into()])
                }
            },
        )
        .unwrap();
        assert!(r.loads(h("data/characters/jhin/first.bin")));
        assert!(!r.loads(h("data/characters/jhin/second.bin")));
    }

    /// Without slot 0 the champion contributes nothing, even when other slots ship.
    #[test]
    fn a_champion_without_slot_zero_is_skipped() {
        let present = present_set(&["data/characters/jhin/skins/skin7.bin"]);
        assert!(compute(&["jhin"], h, |x| present.contains(&x), |_| Some(vec![])).is_none());
    }

    /// No readable root means no knowledge, which must read as "scan everything" rather
    /// than as an empty loaded set.
    #[test]
    fn unreadable_roots_fail_open() {
        let present = present_set(&["data/characters/jhin/skins/skin0.bin"]);
        assert!(compute(&["jhin"], h, |x| present.contains(&x), |_| None).is_none());
    }

    #[test]
    fn no_champions_means_no_knowledge() {
        assert!(compute(&[], h, |_| true, |_| Some(vec![])).is_none());
    }

    /// A root that ships but cannot be read still loads: an unrelated parse failure must
    /// not remove a shipped skin from the loaded set.
    #[test]
    fn an_unreadable_root_still_loads_itself() {
        let present = present_set(&[
            "data/characters/jhin/skins/skin0.bin",
            "data/characters/jhin/skins/skin1.bin",
        ]);
        let r = compute(
            &["jhin"],
            h,
            |x| present.contains(&x),
            |chunk| (chunk == h("data/characters/jhin/skins/skin0.bin")).then(Vec::new),
        )
        .unwrap();
        assert!(r.loads(h("data/characters/jhin/skins/skin1.bin")));
        assert_eq!(r.slot_of(h("data/characters/jhin/skins/skin1.bin")), Some(1));
    }

    #[test]
    fn animation_bins_are_enumerated_per_slot() {
        let map = animation_bin_slots("jhin", h);
        assert_eq!(map.get(&h("data/characters/jhin/animations/skin0.bin")), Some(&0));
        assert_eq!(map.get(&h("data/characters/jhin/animations/skin200.bin")), Some(&200));
        assert_eq!(map.len(), 201);
    }

    #[test]
    fn multiple_champions_are_walked_independently() {
        let present = present_set(&[
            "data/characters/jhin/skins/skin0.bin",
            "data/characters/lux/skins/skin0.bin",
        ]);
        let r = compute(&["jhin", "lux"], h, |x| present.contains(&x), |_| Some(vec![])).unwrap();
        assert!(r.loads(h("data/characters/jhin/skins/skin0.bin")));
        assert!(r.loads(h("data/characters/lux/skins/skin0.bin")));
    }
}
