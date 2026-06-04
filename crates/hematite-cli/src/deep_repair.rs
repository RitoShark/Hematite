//! Deep repair — make a mod self-contained by pulling missing dependencies
//! out of the base-game champion WAD.
//!
//! Modeled on the Celestial launcher's `deep_repair` module. Two capabilities
//! that the original single-pass `--game-wad` extraction lacked:
//!
//! 1. **Seed-BIN backfill** (Celestial step 4b). Asset-only mods (loose
//!    textures/meshes with no character BIN) have nothing whose dependency
//!    chain we can follow. We discover `(champion, skin_no)` seeds from the
//!    mod's file paths, compute the canonical skin BIN path for each, and —
//!    when that BIN is absent from the mod — pull it from the game WAD. That
//!    foundation BIN then feeds the recursive resolver below.
//!
//! 2. **Transitive dependency closure** (Celestial steps 5a–5c). The single
//!    pull pass only reached files *directly* referenced by the mod's own
//!    BINs. Here we run a fixpoint loop: every BIN newly pulled from the game
//!    WAD is parsed, its referenced asset paths AND its `.linked` dependency
//!    BINs are collected, and any of those still missing are pulled too —
//!    repeating until a full pass adds nothing new (closure reached). A
//!    `seen` set of every attempted path guards against infinite loops.
//!
//! This module lives in the CLI crate (not `hematite-core`) because it must
//! import `hematite_ltk::wad_adapter::WadFile`, and `hematite-core` is
//! deliberately LTK-free.

use anyhow::{Context, Result};
use hematite_core::repath as repath_core;
use hematite_core::seeds::{self, SkinSeed};
use hematite_core::traits::{BinProvider, HashProvider};
use hematite_ltk::wad_adapter::{resolve_wad_hash_for, wad_path_hash, WadFile};
use hematite_types::repath::RepathOptions;
use std::collections::HashSet;
use std::path::Path;

/// Outcome of a [`resolve_from_game_wad`] run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeepRepairStats {
    /// Seed skin BINs pulled from the game WAD because the mod didn't ship them.
    pub seed_bins_added: u32,
    /// Total files (any type) pulled from the game WAD, including seed BINs.
    pub files_pulled: u32,
    /// Referenced paths that could not be located in the game WAD.
    pub missing_unresolved: u32,
    /// Number of closure iterations executed (>= 1 when any work was done).
    pub iterations: u32,
}

/// Candidate canonical skin-BIN paths for a discovered seed.
///
/// The primary form matches what [`hematite_core::seeds`] recognises and what
/// [`SkinSeed::bin_path`] emits:
///
/// ```text
///   data/characters/{champion}/skins/skin{N}.bin
/// ```
///
/// An `assets/`-rooted alternate is also returned because some shipped WADs
/// resolve under that root. Both are lower-cased.
///
/// Note: the legacy `data/{champion}/skins/skin{N}.bin` (no `characters/`
/// segment) layout is *not* a pattern this codebase's seed parser recognises
/// (`seeds::discover_seeds` only matches `data/characters/` and
/// `assets/characters/`), so it is intentionally not produced here — emitting
/// it would never round-trip a real discovered seed.
pub fn canonical_seed_bin_paths(seed: &SkinSeed) -> Vec<String> {
    let primary = seed.bin_path(); // data/characters/{champ}/skins/skin{N}.bin
    let assets = format!(
        "assets/characters/{}/skins/skin{}.bin",
        seed.champion, seed.skin_no
    );
    vec![primary, assets]
}

/// Extract a single asset path from the game WAD, trying the literal path,
/// extension alternates (`.dds`↔`.tex`, `.sco`↔`.scb`), and Riot's
/// suffix-stripped rename (via [`resolve_wad_hash_for`]).
///
/// On success the bytes are appended to `all_files` under the *requested*
/// path's hash (so the mod ships it at the path its BINs reference) and the
/// path the caller asked for is returned for downstream dependency parsing.
///
/// Returns `Ok(None)` when the file (and all its alternates) is absent from
/// the game WAD.
fn pull_one(
    requested: &str,
    game_wad: &mut WadFile,
    game_hashes: &HashSet<u64>,
    all_files: &mut Vec<(u64, String, Vec<u8>)>,
) -> Result<Option<(String, Vec<u8>)>> {
    let mut candidates = vec![requested.to_string()];
    if let Some(stem) = requested.strip_suffix(".dds") {
        candidates.push(format!("{}.tex", stem));
    } else if let Some(stem) = requested.strip_suffix(".tex") {
        candidates.push(format!("{}.dds", stem));
    } else if let Some(stem) = requested.strip_suffix(".sco") {
        candidates.push(format!("{}.scb", stem));
    } else if let Some(stem) = requested.strip_suffix(".scb") {
        candidates.push(format!("{}.sco", stem));
    }

    for candidate in candidates {
        let bin_hash = wad_path_hash(&candidate);
        let Some(resolved_hash) = resolve_wad_hash_for(&candidate, bin_hash, game_hashes) else {
            continue;
        };
        match game_wad.extract_chunk_by_hash(resolved_hash) {
            Ok(Some(bytes)) => {
                all_files.push((bin_hash, candidate.clone(), bytes.clone()));
                tracing::debug!("  + pulled from game: {}", candidate);
                return Ok(Some((candidate, bytes)));
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("Failed to extract game WAD chunk for {}: {}", candidate, e);
            }
        }
    }
    Ok(None)
}

/// Collect the asset paths AND linked-BIN dependency paths a freshly-pulled
/// BIN refers to. Non-BIN bytes yield an empty list.
fn dependencies_of(bytes: &[u8], bin_provider: &dyn BinProvider, skip_vo: bool) -> Vec<String> {
    if !repath_core::looks_like_bin(bytes) {
        return Vec::new();
    }
    match bin_provider.parse_bytes(bytes) {
        Ok(tree) => {
            let mut deps = repath_core::collect_bin_asset_paths(&tree, skip_vo);
            // `.linked` dependency BINs aren't asset references, but the game
            // resolves them — pull them too so the chain stays intact.
            deps.extend(tree.linked.iter().cloned());
            deps
        }
        Err(e) => {
            tracing::debug!("Skipping dependency parse (BIN parse failed): {}", e);
            Vec::new()
        }
    }
}

/// Make a mod self-contained by pulling everything it transitively references
/// out of the base-game champion WAD at `game_wad_path`.
///
/// Steps:
/// 1. **Seed backfill** — discover `(champion, skin)` seeds from the mod's
///    paths; for each, pull the canonical skin BIN if the mod lacks it.
/// 2. **Transitive closure** — collect every referenced + linked path from the
///    mod's BINs (now including any backfilled seed BINs), pull the missing
///    ones, parse what was pulled, enqueue *their* deps, and repeat until a
///    pass adds nothing.
///
/// Newly pulled files are appended to `all_files`. The caller is responsible
/// for dry-run gating (this function always performs real extraction when
/// invoked).
pub fn resolve_from_game_wad(
    game_wad_path: &Path,
    all_files: &mut Vec<(u64, String, Vec<u8>)>,
    bin_provider: &dyn BinProvider,
    _hash_provider: &dyn HashProvider,
    opts: &RepathOptions,
) -> Result<DeepRepairStats> {
    tracing::info!(
        "Deep repair: opening game WAD {} (lazy mode)",
        game_wad_path.display()
    );

    let mut game_wad = WadFile::open(game_wad_path)
        .with_context(|| format!("Failed to open game WAD: {}", game_wad_path.display()))?;
    let game_hashes = game_wad.chunk_hash_set();

    let mut stats = DeepRepairStats::default();

    // `seen` tracks every path we've already *attempted* to pull (success or
    // not) plus every path the mod already ships, so the closure terminates.
    let mut seen: HashSet<String> = HashSet::new();

    // Index of what the mod currently ships (refreshed when we add files).
    let rebuild_index = |files: &[(u64, String, Vec<u8>)]| {
        repath_core::WadIndex::from_entries(files.iter().map(|(h, p, _)| (*h, p.clone())))
    };

    // Pre-seed `seen` with the mod's existing paths so we never try to pull
    // something it already has.
    for (_, p, _) in all_files.iter() {
        seen.insert(p.to_lowercase().replace('\\', "/"));
    }

    // -- Step 4b: seed-BIN backfill -----------------------------------------
    {
        let mod_paths: Vec<String> = all_files.iter().map(|(_, p, _)| p.clone()).collect();
        let discovered: Vec<SkinSeed> = seeds::discover_seeds(mod_paths.iter());
        if discovered.is_empty() {
            tracing::debug!(
                "Deep repair: no champion/skin seeds discovered from mod paths \
                 (asset-only mod with no inferrable champion?) — skipping seed backfill"
            );
        }
        for seed in &discovered {
            let mut index = rebuild_index(all_files);
            let candidates = canonical_seed_bin_paths(seed);
            // If the mod already ships any canonical form, nothing to backfill.
            if candidates.iter().any(|c| index.has(c, wad_path_hash)) {
                continue;
            }
            for cand in &candidates {
                let lower = cand.to_lowercase();
                if !seen.insert(lower) {
                    continue;
                }
                if let Some((pulled_path, _)) =
                    pull_one(cand, &mut game_wad, &game_hashes, all_files)?
                {
                    stats.seed_bins_added += 1;
                    stats.files_pulled += 1;
                    tracing::info!(
                        "Deep repair: backfilled seed BIN {} for {}/skin{}",
                        pulled_path,
                        seed.champion,
                        seed.skin_no
                    );
                    index = rebuild_index(all_files);
                    let _ = index; // index is rebuilt fresh each seed iteration
                    break;
                }
            }
        }
    }

    // -- Steps 5a–5c: transitive dependency closure -------------------------
    loop {
        stats.iterations += 1;

        // Collect every asset/linked path referenced by BINs the mod now ships.
        let mut referenced: HashSet<String> = HashSet::new();
        for (_, path, bytes) in all_files.iter() {
            let is_bin =
                path.to_lowercase().ends_with(".bin") || repath_core::looks_like_bin(bytes);
            if !is_bin {
                continue;
            }
            if let Ok(tree) = bin_provider.parse_bytes(bytes) {
                referenced.extend(repath_core::collect_bin_asset_paths(&tree, opts.skip_vo));
                referenced.extend(tree.linked.iter().cloned());
            }
        }

        // Which of those are missing AND not yet attempted?
        let index = rebuild_index(all_files);
        let to_pull: Vec<String> = referenced
            .into_iter()
            .filter(|p| {
                let lower = p.to_lowercase().replace('\\', "/");
                !seen.contains(&lower) && !index.has(p, wad_path_hash)
            })
            .collect();

        if to_pull.is_empty() {
            // Closure reached — no new dependency surfaced this pass.
            stats.iterations -= 1; // this pass did nothing; don't count it
            break;
        }

        tracing::info!(
            "Deep repair: iteration {} — {} new dependency path(s) to resolve",
            stats.iterations,
            to_pull.len()
        );

        let mut pulled_this_pass = 0u32;
        // Parse newly-pulled BINs to discover their deps next iteration; we
        // don't need to enqueue here because the next loop pass re-scans
        // `all_files` from scratch (which now contains the new BINs).
        for path in to_pull {
            let lower = path.to_lowercase().replace('\\', "/");
            if !seen.insert(lower) {
                continue;
            }
            match pull_one(&path, &mut game_wad, &game_hashes, all_files)? {
                Some((pulled_path, bytes)) => {
                    stats.files_pulled += 1;
                    pulled_this_pass += 1;
                    // Eagerly surface dependency counts in the log; the actual
                    // re-scan happens next iteration over `all_files`.
                    let deps = dependencies_of(&bytes, bin_provider, opts.skip_vo);
                    if !deps.is_empty() {
                        tracing::debug!(
                            "    {} references {} further dependency path(s)",
                            pulled_path,
                            deps.len()
                        );
                    }
                }
                None => {
                    stats.missing_unresolved += 1;
                    tracing::debug!("  - not found in game WAD: {}", path);
                }
            }
        }

        // If a whole pass pulled nothing new (all to_pull were unresolved),
        // the next pass's `to_pull` will be empty anyway — but break early to
        // avoid a redundant full re-scan.
        if pulled_this_pass == 0 {
            break;
        }
    }

    tracing::info!(
        "Deep repair: {} file(s) pulled from game WAD ({} seed BIN(s), {} unresolved, {} iteration(s))",
        stats.files_pulled,
        stats.seed_bins_added,
        stats.missing_unresolved,
        stats.iterations
    );

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_core::seeds::SkinSeed;

    #[test]
    fn canonical_seed_paths_match_seed_bin_path() {
        let seed = SkinSeed {
            champion: "yone".into(),
            skin_no: 0,
        };
        let paths = canonical_seed_bin_paths(&seed);
        assert_eq!(paths[0], "data/characters/yone/skins/skin0.bin");
        assert_eq!(paths[0], seed.bin_path());
        assert_eq!(paths[1], "assets/characters/yone/skins/skin0.bin");
    }

    #[test]
    fn canonical_seed_paths_nonzero_skin() {
        let seed = SkinSeed {
            champion: "jinx".into(),
            skin_no: 27,
        };
        let paths = canonical_seed_bin_paths(&seed);
        assert_eq!(paths[0], "data/characters/jinx/skins/skin27.bin");
        assert_eq!(paths[1], "assets/characters/jinx/skins/skin27.bin");
    }

    #[test]
    fn canonical_seed_paths_lowercased() {
        // SkinSeed.champion is already lower-cased by discover_seeds, but the
        // helper must not introduce any upper-case regardless.
        let seed = SkinSeed {
            champion: "kaisa".into(),
            skin_no: 3,
        };
        for p in canonical_seed_bin_paths(&seed) {
            assert_eq!(p, p.to_lowercase());
        }
    }

    /// In-memory BIN provider that recognises a tiny synthetic format so we
    /// can exercise the closure loop without a real LTK BIN or game WAD.
    ///
    /// Encoding: bytes are the BIN magic `PROP` followed by a UTF-8 list of
    /// dependency paths separated by `\n`. `parse_bytes` returns a `BinTree`
    /// with those paths in `.linked` (so the closure follows them).
    struct FakeBinProvider;

    impl BinProvider for FakeBinProvider {
        fn parse_bytes(&self, data: &[u8]) -> anyhow::Result<hematite_types::bin::BinTree> {
            anyhow::ensure!(data.len() >= 4 && &data[..4] == b"PROP", "not a fake BIN");
            let body = std::str::from_utf8(&data[4..]).unwrap_or("");
            let linked: Vec<String> = body
                .split('\n')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            Ok(hematite_types::bin::BinTree {
                objects: Default::default(),
                linked,
            })
        }
        fn write_bytes(&self, _tree: &hematite_types::bin::BinTree) -> anyhow::Result<Vec<u8>> {
            unimplemented!("not needed for closure test")
        }
    }

    fn fake_bin(deps: &[&str]) -> Vec<u8> {
        let mut v = b"PROP".to_vec();
        v.extend(deps.join("\n").into_bytes());
        v
    }

    /// The closure-walk logic, factored out of `resolve_from_game_wad` so it
    /// can run against an in-memory dependency graph (no WAD). Mirrors the
    /// real loop's termination: re-scan all files, pull unseen+missing deps
    /// from `graph`, stop when a pass adds nothing.
    fn run_closure(
        all_files: &mut Vec<(u64, String, Vec<u8>)>,
        provider: &dyn BinProvider,
        graph: &std::collections::HashMap<String, Vec<u8>>,
    ) -> u32 {
        let mut seen: HashSet<String> = all_files
            .iter()
            .map(|(_, p, _)| p.to_lowercase())
            .collect();
        let mut iterations = 0u32;
        loop {
            let mut referenced: HashSet<String> = HashSet::new();
            for (_, _p, bytes) in all_files.iter() {
                if let Ok(tree) = provider.parse_bytes(bytes) {
                    referenced.extend(tree.linked);
                }
            }
            let present: HashSet<String> =
                all_files.iter().map(|(_, p, _)| p.to_lowercase()).collect();
            let to_pull: Vec<String> = referenced
                .into_iter()
                .filter(|p| !seen.contains(&p.to_lowercase()) && !present.contains(&p.to_lowercase()))
                .collect();
            if to_pull.is_empty() {
                break;
            }
            iterations += 1;
            let mut pulled = 0;
            for p in to_pull {
                if !seen.insert(p.to_lowercase()) {
                    continue;
                }
                if let Some(bytes) = graph.get(&p) {
                    all_files.push((0, p.clone(), bytes.clone()));
                    pulled += 1;
                }
            }
            if pulled == 0 {
                break;
            }
        }
        iterations
    }

    #[test]
    fn closure_resolves_transitive_chain_and_terminates() {
        // A -> B, C ; B -> D ; C -> D (diamond) ; D -> (nothing)
        let mut graph = std::collections::HashMap::new();
        graph.insert("b.bin".to_string(), fake_bin(&["d.bin"]));
        graph.insert("c.bin".to_string(), fake_bin(&["d.bin"]));
        graph.insert("d.bin".to_string(), fake_bin(&[]));

        // Mod ships only the root A, which references B and C.
        let mut all_files = vec![(0u64, "a.bin".to_string(), fake_bin(&["b.bin", "c.bin"]))];

        let iters = run_closure(&mut all_files, &FakeBinProvider, &graph);

        let pulled: HashSet<String> =
            all_files.iter().map(|(_, p, _)| p.clone()).collect();
        assert!(pulled.contains("b.bin"));
        assert!(pulled.contains("c.bin"));
        assert!(pulled.contains("d.bin"));
        // A + B + C + D = 4 files total.
        assert_eq!(all_files.len(), 4);
        // Diamond D must only be pulled once.
        assert_eq!(all_files.iter().filter(|(_, p, _)| p == "d.bin").count(), 1);
        // Pass 1 pulls B,C; pass 2 pulls D; pass 3 adds nothing → 2 iterations.
        assert_eq!(iters, 2);
    }

    #[test]
    fn closure_terminates_on_cycle() {
        // A -> B ; B -> A (cycle). Must not loop forever.
        let mut graph = std::collections::HashMap::new();
        graph.insert("b.bin".to_string(), fake_bin(&["a.bin"]));

        let mut all_files = vec![(0u64, "a.bin".to_string(), fake_bin(&["b.bin"]))];
        let iters = run_closure(&mut all_files, &FakeBinProvider, &graph);

        assert_eq!(all_files.len(), 2); // A + B, A is not re-pulled
        assert_eq!(iters, 1);
    }

    #[test]
    fn closure_noop_when_self_contained() {
        // A -> B and the mod already ships B. Nothing to pull.
        let graph = std::collections::HashMap::new();
        let mut all_files = vec![
            (0u64, "a.bin".to_string(), fake_bin(&["b.bin"])),
            (0u64, "b.bin".to_string(), fake_bin(&[])),
        ];
        let iters = run_closure(&mut all_files, &FakeBinProvider, &graph);
        assert_eq!(all_files.len(), 2);
        assert_eq!(iters, 0);
    }
}
