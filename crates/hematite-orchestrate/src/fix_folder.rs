//! The folder-level fix pipeline: extract → detect → fix → recover → rebuild.
//!
//! Lifted verbatim from the CLI's `process_wad_folder` so both the CLI and
//! embedders (Flint) drive the exact same orchestration. The CLI's folder
//! branch is now a thin adapter that builds [`FixOptions`] + a
//! [`ProgressSink`] and calls [`fix_folder`].

use crate::live_provider::LiveGameProvider;
use crate::options::FixOptions;
use crate::progress::ProgressSink;
use anyhow::{Context, Result};
use hematite_core::context::FixContext;
use hematite_core::pipeline::apply_fixes;
use hematite_core::repath as repath_core;
use hematite_core::traits::{BinProvider, GameProvider, HashProvider};
use hematite_core::wad_pipeline::converters::ConverterRegistry;
use hematite_file::{
    bin_adapter::FileBinProvider, mesh_converter, texture_converter, wad_adapter::wad_path_hash,
};
use hematite_types::champion::CharacterRelations;
use hematite_types::config::FixConfig;
use hematite_types::repath::RepathOptions;
use hematite_types::result::{CheckInfo, ProcessResult};
use std::path::Path;
use std::sync::Arc;
use walkdir::WalkDir;

/// Run the full fix pipeline over one `.wad.client` folder.
///
/// Walks the directory, extracts every file, runs the combo-bin relocation,
/// WAD-level and BIN-level fix pipelines, optional restore-anm + repath
/// recovery, then rebuilds the folder — unless `opts.dry_run` or
/// `opts.detect_only` is set, in which case nothing is written to disk.
///
/// In `detect_only` mode every fired fix is still recorded in
/// `ProcessResult.applied_fixes` (with its detection count) and `check_info`
/// is populated, but no WAD is rebuilt and no file is written or removed —
/// the same disk-neutral contract the CLI's `--check` mode has today.
pub fn fix_folder(
    folder: &Path,
    config: &FixConfig,
    selected_fixes: &[String],
    champions: &CharacterRelations,
    hash_provider: &Arc<dyn HashProvider>,
    opts: &FixOptions<'_>,
    progress: &dyn ProgressSink,
) -> Result<ProcessResult> {
    // Detect-only is the CLI's `--check` contract lifted into the library: it
    // detects + reports counts + populates CheckInfo, but writes nothing. That
    // is exactly `dry_run` (skip every mutation branch) plus `check` (populate
    // CheckInfo), so we fold it in here. `dry_run` alone keeps its exact prior
    // behaviour when `detect_only` is false.
    let dry_run = opts.dry_run || opts.detect_only;
    let check = opts.detect_only;
    let repath_opts = opts.repath;
    let live = opts.live;

    use hematite_core::wad_pipeline;
    use hematite_file::wad_adapter::FileWadProvider;

    tracing::info!("Processing WAD folder: {}", folder.display());

    let bin_provider = FileBinProvider;

    // Extract all files from the WAD folder
    let mut all_files = Vec::new();

    for entry in WalkDir::new(folder) {
        let entry = entry.context("Failed to read directory entry in WAD folder")?;
        let path = entry.path();
        if path.is_file() {
            let rel_path = path
                .strip_prefix(folder)
                .context("Failed to strip prefix from WAD folder path")?;
            let rel_path_str = rel_path.to_string_lossy().replace('\\', "/");
            let hash = wad_path_hash(&rel_path_str);
            let bytes = std::fs::read(path).context("Failed to read file in WAD folder")?;
            all_files.push((hash, rel_path_str, bytes));
        }
    }

    // Original on-disk relative paths, snapshotted before any rename/convert —
    // used by the in-place writer to delete files that were renamed away or
    // removed (so `.dds` originals don't linger next to their new `.tex`).
    let original_paths: std::collections::HashSet<String> =
        all_files.iter().map(|(_, p, _)| p.clone()).collect();

    // === COMBO-BIN RELOCATION ===
    // Must run before the BIN fix loop (and before the WAD-provider hash
    // set is built, so downstream `has_path`/`has_hash` checks see the
    // relocated location) and before repath/restore-anm. Order vs. seed
    // discovery doesn't matter: combo bins are never seeds.
    let mut combo_bins_relocated = 0u32;
    if opts.relocate_combo_bins || selected_fixes.iter().any(|f| f == "combo_bin_relocate") {
        if dry_run {
            tracing::info!("[dry-run] Would relocate legacy combo-bin WAD entries");
        } else {
            combo_bins_relocated = run_combo_bin_relocate(&mut all_files, opts.game_wad, live);
            if combo_bins_relocated > 0 {
                progress.fix_applied(
                    "Relocated combo-bin(s) to Riot's multi_skins path",
                    Some(combo_bins_relocated),
                );
            }
        }
    }

    // === PRE-APPLIED SKINLITE ===
    // Verified skin0 clones leave the working set here and are regenerated
    // from the fixed skin0 just before the rebuild — fixing one BIN instead
    // of ~99 identical ones.
    let skinlite_sets = crate::skinlite::detect_and_strip(&mut all_files, &bin_provider);
    for set in &skinlite_sets {
        progress.note(&format!(
            "SkinLite detected on {}: fixing skin0 once, recloning {} slot(s) after",
            set.champ,
            set.slots.len()
        ));
    }

    let path_hashes: std::collections::HashSet<u64> =
        all_files.iter().map(|(h, _, _)| *h).collect();
    let wad_provider = FileWadProvider::from_hashes(path_hashes);

    // Identify BIN entries by content magic, not just by path extension
    let bin_chunks: Vec<_> = all_files
        .iter()
        .filter(|(_h, path, bytes)| {
            path.to_lowercase().ends_with(".bin") || repath_core::looks_like_bin(bytes)
        })
        .cloned()
        .collect();

    tracing::info!(
        "WAD folder has {} total entries, {} BIN file(s)",
        all_files.len(),
        bin_chunks.len()
    );

    // Discover champion/skin seeds from the resolved TOC.
    {
        let seeds =
            hematite_core::seeds::discover_seeds(all_files.iter().map(|(_, p, _)| p.as_str()));
        if seeds.is_empty() {
            tracing::debug!("Seed discovery: no skin BINs found in TOC (binless mod?)");
        } else {
            let unique_champs: std::collections::HashSet<&str> =
                seeds.iter().map(|s| s.champion.as_str()).collect();
            tracing::info!(
                "Seed discovery: {} skin(s) across {} champion(s)",
                seeds.len(),
                unique_champs.len()
            );
            for seed in &seeds {
                tracing::debug!("  seed → {} (skin{})", seed.champion, seed.skin_no);
            }
            if unique_champs.len() > 1 {
                let mut names: Vec<&str> = unique_champs.iter().copied().collect();
                names.sort();
                progress.note(&format!(
                    "WAD folder contains subchampion forms: {}",
                    names.join(", ")
                ));
                tracing::info!(
                    "WAD folder contains subchampion forms: {}",
                    names.join(", ")
                );
            }

            if let Some(live) = live {
                prime_champion_wads(live, champions, unique_champs.iter().copied());
            }
        }
    }

    let mut total_result = ProcessResult::default();
    total_result.fixes_applied += combo_bins_relocated;
    let mut shared_files_to_remove = Vec::new();

    // === WAD-LEVEL PIPELINE ===
    progress.stage("Detecting WAD-level issues…");
    tracing::debug!("Running WAD-level pipeline...");
    let referenced = collect_referenced_assets(&all_files, &bin_provider);
    let wad_output = wad_pipeline::apply_wad_fixes(
        &all_files,
        config,
        selected_fixes,
        hash_provider.as_ref(),
        &referenced,
    )?;

    shared_files_to_remove.extend(wad_output.files_to_remove.clone());
    // Drop removed entries NOW: repath renames paths later, and a rename
    // would make the write-time path filter silently miss the removal.
    if !dry_run && !wad_output.files_to_remove.is_empty() {
        let removed: std::collections::HashSet<&String> =
            wad_output.files_to_remove.iter().collect();
        all_files.retain(|(_, p, _)| !removed.contains(p));
    }

    for wad_fix in &wad_output.applied_fixes {
        progress.fix_applied(&wad_fix.fix_name, Some(wad_fix.files_affected));
        tracing::info!(
            "WAD-level fix '{}' affected {} files",
            wad_fix.fix_name,
            wad_fix.files_affected
        );
        total_result.fixes_applied += wad_fix.files_affected;
    }

    // Perform file format conversions
    let mut converter_registry = ConverterRegistry::new();
    converter_registry.register("dds_to_tex", texture_converter::dds_to_tex);
    converter_registry.register("sco_to_scb", mesh_converter::sco_to_scb);
    converter_registry.register(
        "strip_mipmaps",
        hematite_file::strip_mipmaps::strip_mipmaps_auto,
    );
    converter_registry.register(
        "fix_tex_dims",
        hematite_file::fix_dimensions::fix_dimensions_auto,
    );

    // The actual byte conversions only run on a real fix. On a detect-only
    // scan the detection counts already came from `wad_output.applied_fixes`
    // above; doing (and logging) the conversions here would be wasted work and
    // misleading "✓ Converted/renamed" output for a pass that writes nothing.
    let mut conversion_count = 0u32;
    if !dry_run && !wad_output.files_to_convert.is_empty() {
        tracing::info!(
            "Converting {} file formats...",
            wad_output.files_to_convert.len()
        );

        for conversion in &wad_output.files_to_convert {
            if let Some((hash, path, bytes)) =
                all_files.iter_mut().find(|(_, p, _)| p == &conversion.path)
            {
                match converter_registry.convert(&conversion.converter, bytes) {
                    Ok(converted_bytes) => {
                        let old_size = bytes.len();
                        *bytes = converted_bytes;
                        conversion_count += 1;

                        if conversion.from_ext != conversion.to_ext {
                            let old_path = path.clone();
                            let new_path = path.replace(
                                &format!(".{}", conversion.from_ext),
                                &format!(".{}", conversion.to_ext),
                            );
                            *path = new_path.clone();
                            *hash = wad_path_hash(&new_path);
                            tracing::info!(
                                "✓ Converted {} from .{} to .{} ({} → {} bytes) and renamed to {}",
                                old_path,
                                conversion.from_ext,
                                conversion.to_ext,
                                old_size,
                                bytes.len(),
                                new_path
                            );
                        } else {
                            tracing::info!(
                                "✓ Converted {} from .{} to .{} ({} → {} bytes)",
                                conversion.path,
                                conversion.from_ext,
                                conversion.to_ext,
                                old_size,
                                bytes.len()
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "✗ Converter '{}' failed for {}: {}",
                            conversion.converter,
                            conversion.path,
                            e
                        );
                    }
                }
            }
        }
        total_result.fixes_applied += conversion_count;
    }

    // In-place byte transforms run even on dry_run (without mutating) so the
    // reported counts reflect files the converter actually changed, not mere
    // extension matches.
    if !wad_output.files_to_transform.is_empty() {
        let mut per_fix: std::collections::BTreeMap<String, (String, u32)> = Default::default();
        for op in &wad_output.files_to_transform {
            if let Some((_, _, bytes)) = all_files.iter_mut().find(|(_, p, _)| p == &op.path) {
                match converter_registry.convert(&op.converter, bytes) {
                    Ok(new_bytes) => {
                        if new_bytes != *bytes {
                            tracing::info!(
                                "✓ Transformed {} via {} ({} → {} bytes)",
                                op.path,
                                op.converter,
                                bytes.len(),
                                new_bytes.len()
                            );
                            if !dry_run {
                                *bytes = new_bytes;
                            }
                            per_fix
                                .entry(op.fix_id.clone())
                                .or_insert_with(|| (op.fix_name.clone(), 0))
                                .1 += 1;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "✗ In-place transform '{}' failed for {}: {}",
                            op.converter,
                            op.path,
                            e
                        );
                    }
                }
            }
        }
        for (fix_name, count) in per_fix.values() {
            progress.fix_applied(fix_name, Some(*count));
            total_result.fixes_applied += count;
        }
    }

    // Append injected files
    let mut added_count = 0u32;
    if !wad_output.files_to_add.is_empty() {
        tracing::info!(
            "Injecting {} fallback asset(s)...",
            wad_output.files_to_add.len()
        );
        let mut paths_in_wad: std::collections::HashSet<String> =
            all_files.iter().map(|(_, p, _)| p.to_lowercase()).collect();
        for addition in &wad_output.files_to_add {
            let lower = addition.path.to_lowercase();
            if addition.only_if_missing && paths_in_wad.contains(&lower) {
                continue;
            }
            let Some(bytes) = hematite_core::assets::get(&addition.asset) else {
                continue;
            };
            let hash = wad_path_hash(&addition.path);
            all_files.push((hash, addition.path.clone(), bytes.to_vec()));
            paths_in_wad.insert(lower);
            added_count += 1;
        }
        total_result.fixes_applied += added_count;
    }

    // === LINKED BIN RESOLUTION (BFS) ===
    let mut parsed_bins: std::collections::HashMap<String, hematite_types::bin::BinTree> =
        std::collections::HashMap::new();
    let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();

    for (_hash, path, bytes) in &bin_chunks {
        match bin_provider.parse_bytes(bytes) {
            Ok(tree) => {
                for linked_path in &tree.linked {
                    if !parsed_bins.contains_key(linked_path) {
                        queue.push_back(linked_path.clone());
                    }
                }
                parsed_bins.insert(path.clone(), tree);
            }
            Err(e) => {
                tracing::debug!("Failed to parse BIN {path}: {e}");
            }
        }
    }

    while let Some(linked_path) = queue.pop_front() {
        if parsed_bins.contains_key(&linked_path) {
            continue;
        }
        if let Some((_, _, bytes)) = all_files.iter().find(|(_, p, _)| *p == linked_path) {
            match bin_provider.parse_bytes(bytes) {
                Ok(tree) => {
                    for dep in &tree.linked {
                        if !parsed_bins.contains_key(dep) {
                            queue.push_back(dep.clone());
                        }
                    }
                    parsed_bins.insert(linked_path, tree);
                }
                Err(e) => {
                    tracing::debug!("Failed to parse linked BIN {}: {}", linked_path, e);
                }
            }
        }
    }

    let primary_bin_paths: std::collections::HashSet<String> =
        bin_chunks.iter().map(|(_, p, _)| p.clone()).collect();
    let linked_only: std::collections::HashMap<String, hematite_types::bin::BinTree> = parsed_bins
        .iter()
        .filter(|(k, _)| !primary_bin_paths.contains(*k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // === BIN-LEVEL PIPELINE ===
    let shader_validator = hematite_core::detect::shader::ShaderValidator::load()
        .ok()
        .filter(|v| v.is_available());

    let ui = progress;
    ui.stage("Applying fixes…");
    ui.set_length(bin_chunks.len() as u64);

    for (_, path, _) in &bin_chunks {
        let Some(tree) = parsed_bins.remove(path) else {
            ui.tick();
            continue;
        };

        let mut ctx = FixContext {
            tree,
            hashes: hash_provider.as_ref(),
            wad: &wad_provider,
            champions,
            files_to_remove: Vec::new(),
            file_path: path.clone(),
            linked_trees: linked_only.clone(),
            shader_validator: shader_validator.as_ref(),
            game: live.map(|l| l as &dyn GameProvider),
            additional_bins: Vec::new(),
        };

        let result = apply_fixes(&mut ctx, config, selected_fixes, dry_run);

        if result.fixes_applied > 0 {
            for fix in &result.applied_fixes {
                ui.fix_applied(&fix.fix_name, Some(fix.changes_count));
            }
        }
        ui.tick();

        let fixes_applied = result.fixes_applied;
        total_result.merge(result);

        if !dry_run && fixes_applied > 0 {
            match bin_provider.write_bytes(&ctx.tree) {
                Ok(modified_bytes) => {
                    if let Some((_, _, file_bytes)) =
                        all_files.iter_mut().find(|(_, p, _)| p == path)
                    {
                        *file_bytes = modified_bytes;
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to write modified BIN {}: {}", path, e);
                }
            }
        }

        shared_files_to_remove.extend(ctx.files_to_remove);

        if !dry_run && !ctx.additional_bins.is_empty() {
            for (new_path, new_tree) in &ctx.additional_bins {
                match bin_provider.write_bytes(new_tree) {
                    Ok(bytes) => {
                        let hash = wad_path_hash(new_path);
                        if let Some((_, _, existing)) =
                            all_files.iter_mut().find(|(h, _, _)| *h == hash)
                        {
                            *existing = bytes;
                        } else {
                            all_files.push((hash, new_path.clone(), bytes));
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to serialize split-BIN output {}: {}", new_path, e);
                    }
                }
            }
        }
    }

    total_result.files_removed = shared_files_to_remove.len() as u32;

    // === RESTORE-ANM PIPELINE ===
    // See process_wad_file's identical step for rationale — independent
    // of --repath, must run before the WAD rebuild.
    if opts.restore_anm {
        if dry_run {
            tracing::info!("[dry-run] Would restore missing .anm references from the game");
        } else {
            ui.stage("Restoring missing animations…");
            let restored = run_restore_anm(&mut all_files, &bin_provider, opts.game_wad, live);
            if restored > 0 {
                total_result.fixes_applied += restored;
            }
        }
    }

    // === REPATH PIPELINE ===
    if let Some(opts) = repath_opts {
        if !dry_run {
            ui.stage(&format!("Repathing assets (prefix “{}”)…", opts.prefix));

            let deduped = dedupe_stacked_prefixes(&mut all_files, &opts.prefix, &bin_provider);
            if deduped > 0 {
                ui.fix_applied("Collapsed stacked repath prefix", Some(deduped));
                total_result.fixes_applied += 1;
            }

            let mut game_files_added = 0u32;
            if let Some(ref game_wad_path) = opts.game_wad {
                game_files_added = extract_missing_from_game_wad(
                    game_wad_path,
                    &mut all_files,
                    &bin_provider,
                    hash_provider.as_ref(),
                    opts,
                )?;
            } else if let Some(live) = live {
                game_files_added = extract_missing_from_live(
                    live,
                    &mut all_files,
                    &bin_provider,
                    hash_provider.as_ref(),
                    opts,
                )?;
            }

            let index = repath_core::WadIndex::from_entries(
                all_files.iter().map(|(h, p, _)| (*h, p.clone())),
            );

            let mut combined_mapping: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let mut repath_bin_count = 0u32;
            let mut bins_touched = 0u32;

            for (_h, path, bytes) in all_files.iter_mut() {
                let is_bin =
                    path.to_lowercase().ends_with(".bin") || repath_core::looks_like_bin(bytes);
                if !is_bin {
                    continue;
                }
                let mut tree = match bin_provider.parse_bytes(bytes) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::debug!("Skipping BIN at {}: parse failed: {}", path, e);
                        continue;
                    }
                };
                let r = repath_core::repath_bin_strings(&mut tree, opts, &index, wad_path_hash);
                if r.strings_repathed == 0 {
                    continue;
                }
                match bin_provider.write_bytes(&tree) {
                    Ok(new_bytes) => {
                        repath_bin_count += r.strings_repathed;
                        bins_touched += 1;
                        for (k, v) in r.mapping {
                            combined_mapping.entry(k).or_insert(v);
                        }
                        *bytes = new_bytes;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to write repathed BIN {}: {}", path, e)
                    }
                }
            }

            let hash_mapping: std::collections::HashMap<u64, String> = combined_mapping
                .iter()
                .map(|(orig, new)| (wad_path_hash(orig), new.clone()))
                .collect();

            let mut repath_wad_count = 0u32;
            let mut new_path_set: Vec<String> = Vec::new();
            let mut seen_dest: std::collections::HashMap<String, u32> =
                std::collections::HashMap::new();
            let mut hash_renames: std::collections::HashMap<u64, (u64, String)> =
                std::collections::HashMap::new();

            let repathed: Vec<(u64, String, Vec<u8>)> = all_files
                .drain(..)
                .map(|(hash, path, bytes)| {
                    let lower = path.to_lowercase();

                    let new_path_opt: Option<String> = combined_mapping
                        .get(&lower)
                        .cloned()
                        .or_else(|| hash_mapping.get(&hash).cloned())
                        .or_else(|| repath_core::repath_wad_path(&path, &opts.prefix, opts.layout));

                    let final_path = match new_path_opt {
                        Some(np) => {
                            let np_lower = np.to_lowercase();
                            let suffix = seen_dest
                                .entry(np_lower.clone())
                                .and_modify(|c| *c += 1)
                                .or_insert(0);
                            if *suffix == 0 {
                                np
                            } else if let Some(dot) = np.rfind('.') {
                                format!("{}_{}{}", &np[..dot], suffix, &np[dot..])
                            } else {
                                format!("{}_{}", np, suffix)
                            }
                        }
                        None => path.clone(),
                    };

                    if final_path != path {
                        repath_wad_count += 1;
                        new_path_set.push(final_path.to_lowercase());
                        let new_hash = wad_path_hash(&final_path);
                        hash_renames.insert(hash, (new_hash, final_path.clone()));
                        (new_hash, final_path, bytes)
                    } else {
                        (hash, path, bytes)
                    }
                })
                .collect();
            all_files = repathed;

            let file_hashes_rewritten =
                rewrite_file_hash_refs(&mut all_files, &hash_renames, &bin_provider);
            if file_hashes_rewritten > 0 {
                ui.fix_applied(
                    "Updated file-hash reference(s) to repathed chunks",
                    Some(file_hashes_rewritten),
                );
            }

            tracing::info!(
                "  {} string(s) + {} file hash(es) in {} BIN(s) repathed; \
                 {} WAD entry/entries renamed; {} pulled from game WAD",
                repath_bin_count,
                file_hashes_rewritten,
                bins_touched,
                repath_wad_count,
                game_files_added
            );

            if repath_bin_count == 0 && repath_wad_count == 0 {
                // Warning
            } else {
                total_result.fixes_applied += 1;
            }

            if opts.invis_texture && !new_path_set.is_empty() {
                let existing: std::collections::HashSet<String> =
                    all_files.iter().map(|(_, p, _)| p.to_lowercase()).collect();
                let placeholders = repath_core::missing_placeholders(
                    &existing,
                    &new_path_set,
                    &opts.placeholder_rules,
                );
                if !placeholders.is_empty() {
                    for (path, bytes) in placeholders {
                        let hash = wad_path_hash(&path);
                        all_files.push((hash, path, bytes));
                    }
                }
            }
        }
    }

    // === POST-REPATH BIN PHASE ===
    // Rules marked `"phase": "post_repath"` (e.g. the string→file retype) run
    // after repath because they replace the very strings repath rewrites.
    {
        let post_result = apply_post_repath_fixes(
            &mut all_files,
            config,
            selected_fixes,
            champions,
            hash_provider,
            dry_run,
            progress,
        );
        total_result.merge(post_result);
    }

    // === SKINLITE RECLONE ===
    if !dry_run && !skinlite_sets.is_empty() {
        let recloned = crate::skinlite::reclone(&mut all_files, &skinlite_sets, &bin_provider);
        tracing::info!("SkinLite: recloned {recloned} slot bin(s) from fixed skin0");
    }

    if check {
        use hematite_core::detect::skin::SkinDetector;

        let all_paths: Vec<String> = all_files.iter().map(|(_, p, _)| p.clone()).collect();
        let detector = SkinDetector::new();
        let skin_info = detector.detect_from_paths(&all_paths);

        let detected: Vec<String> = total_result
            .applied_fixes
            .iter()
            .map(|f| f.fix_name.clone())
            .collect();

        let skin_number = skin_info.primary_skin();
        let is_binless = skin_info.is_binless;
        let champion = if skin_info.champion.is_empty() {
            None
        } else {
            Some(skin_info.champion)
        };

        total_result.check_info = Some(CheckInfo {
            champion,
            skin_number,
            is_binless,
            detected_issues: detected,
        });
    }

    // === WAD FOLDER WRITING ===
    if !dry_run && (total_result.fixes_applied > 0 || !shared_files_to_remove.is_empty()) {
        // The set of relative paths the run wants on disk (post-rename), minus
        // anything explicitly removed.
        let final_paths: std::collections::HashSet<&String> = all_files
            .iter()
            .map(|(_, p, _)| p)
            .filter(|p| !shared_files_to_remove.contains(*p))
            .collect();

        let output_path = if opts.in_place {
            folder.to_path_buf()
        } else {
            fixed_wad_output_path(folder)
        };
        std::fs::create_dir_all(&output_path).context("Failed to create output WAD folder")?;

        ui.stage("Updating WAD folder…");
        tracing::info!(
            "Writing modified WAD folder ({})...",
            if opts.in_place {
                "in place"
            } else {
                "fixed copy"
            }
        );

        let mut files_written = 0;
        for (_, path, bytes) in &all_files {
            if !shared_files_to_remove.contains(path) {
                let dest_file_path = output_path.join(path);
                if let Some(parent) = dest_file_path.parent() {
                    std::fs::create_dir_all(parent)
                        .context("Failed to create parent directory for file in WAD folder")?;
                }
                std::fs::write(&dest_file_path, bytes)
                    .context("Failed to write file in WAD folder")?;
                files_written += 1;
            }
        }

        // In place: delete original files that were renamed away or removed, so
        // stale `.dds` (etc.) don't linger beside their new `.tex`. Only touch
        // paths we originally READ from this folder — never anything else.
        let mut files_deleted = 0;
        if opts.in_place {
            for orig in &original_paths {
                if !final_paths.contains(orig) {
                    let stale = output_path.join(orig);
                    if stale.is_file() && std::fs::remove_file(&stale).is_ok() {
                        files_deleted += 1;
                    }
                }
            }
        }

        let is_intermediate = output_path.starts_with(std::env::temp_dir());
        if !is_intermediate {
            ui.fix_applied(&format!("Wrote WAD folder {}", output_path.display()), None);
        }
        tracing::info!("✓ Wrote WAD folder to: {}", output_path.display());
        tracing::info!(
            "  {} files written, {} removed, {} stale deleted",
            files_written,
            shared_files_to_remove.len(),
            files_deleted
        );
    } else if !dry_run {
        ui.note("No changes detected — WAD folder not modified.");
        tracing::info!("No changes detected - WAD folder not modified");
    }

    Ok(total_result)
}

/// Everything the mod's BINs reference, for `remove_file` rules with
/// `unless_referenced`: lowercased path strings (assets + `linked:` deps,
/// VO included) and xxh64 `file` hash values.
pub fn collect_referenced_assets(
    all_files: &[(u64, String, Vec<u8>)],
    bin_provider: &FileBinProvider,
) -> hematite_core::wad_pipeline::ReferencedAssets {
    use hematite_core::traits::BinProvider;
    let mut out = hematite_core::wad_pipeline::ReferencedAssets::default();
    for (_, path, bytes) in all_files {
        let is_bin = path.to_lowercase().ends_with(".bin") || repath_core::looks_like_bin(bytes);
        if !is_bin {
            continue;
        }
        let Ok(tree) = bin_provider.parse_bytes(bytes) else {
            continue;
        };
        out.paths
            .extend(repath_core::collect_bin_asset_paths(&tree, false));
        out.hashes
            .extend(repath_core::collect_bin_asset_hashes(&tree));
    }
    out
}

/// Rewrite xxh64 `file` references in every BIN after WAD entries moved.
/// `renames` maps `old_entry_hash` → `(new_entry_hash, new_path)`. This is
/// the hash-typed twin of the string repath: hand-migrated mods reference
/// chunks by `file` hash, and moving the chunk without updating the hash
/// leaves the reference dangling (the "repathed animations crash" bug).
pub fn rewrite_file_hash_refs(
    all_files: &mut [(u64, String, Vec<u8>)],
    renames: &std::collections::HashMap<u64, (u64, String)>,
    bin_provider: &FileBinProvider,
) -> u32 {
    use hematite_core::traits::BinProvider;
    if renames.is_empty() {
        return 0;
    }
    let mut total = 0u32;
    for (_, path, bytes) in all_files.iter_mut() {
        let is_bin = path.to_lowercase().ends_with(".bin") || repath_core::looks_like_bin(bytes);
        if !is_bin {
            continue;
        }
        let Ok(mut tree) = bin_provider.parse_bytes(bytes) else {
            continue;
        };
        let n = repath_core::rewrite_bin_file_hashes(&mut tree, renames);
        if n == 0 {
            continue;
        }
        match bin_provider.write_bytes(&tree) {
            Ok(new_bytes) => {
                *bytes = new_bytes;
                total += n;
            }
            Err(e) => tracing::warn!("Failed to write hash-rewritten BIN {}: {}", path, e),
        }
    }
    if total > 0 {
        tracing::info!("Rewrote {total} file-hash reference(s) to renamed chunks");
    }
    total
}

/// Repair pre-idempotency double-fix damage: collapse stacked repath
/// prefixes (`assets/hematite/hematite/…`) on chunk paths and BIN strings
/// back to the single-prefix form, so the `file` hashes retyped on the first
/// fix resolve again. A stacked chunk whose collapse target already exists
/// keeps its path (and its strings stay consistent with it). Returns the
/// number of renamed chunks + rewritten strings.
pub fn dedupe_stacked_prefixes(
    all_files: &mut [(u64, String, Vec<u8>)],
    prefix: &str,
    bin_provider: &FileBinProvider,
) -> u32 {
    use hematite_core::walk::{walk_tree, PropertyVisitor, VisitResult};

    if prefix.is_empty() {
        return 0;
    }

    let existing: std::collections::HashSet<String> = all_files
        .iter()
        .map(|(_, p, _)| p.to_lowercase().replace('\\', "/"))
        .collect();

    let mut rename: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut targets: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (_, p, _) in all_files.iter() {
        if let Some(np) = repath_core::collapse_stacked_prefix(p, prefix) {
            let np_lower = np.to_lowercase();
            if !existing.contains(&np_lower) && targets.insert(np_lower) {
                rename.insert(p.to_lowercase().replace('\\', "/"), np);
            }
        }
    }

    let mut final_paths: std::collections::HashSet<String> = existing
        .iter()
        .filter(|p| !rename.contains_key(*p))
        .cloned()
        .collect();
    final_paths.extend(targets);

    struct Collapser<'a> {
        prefix: &'a str,
        final_paths: &'a std::collections::HashSet<String>,
        changes: u32,
    }
    impl Collapser<'_> {
        fn collapse(&mut self, value: &str) -> Option<String> {
            let collapsed = repath_core::collapse_stacked_prefix(value, self.prefix)?;
            if self.final_paths.contains(&value.to_lowercase()) {
                return None;
            }
            self.changes += 1;
            Some(collapsed)
        }
    }
    impl PropertyVisitor for Collapser<'_> {
        fn visit_string(
            &mut self,
            value: &str,
            _f: hematite_types::hash::FieldHash,
        ) -> VisitResult {
            match self.collapse(value) {
                Some(new) => VisitResult::Mutate(new),
                None => VisitResult::Skip,
            }
        }
    }

    let mut changes = 0u32;
    for (_, path, bytes) in all_files.iter_mut() {
        let is_bin = path.to_lowercase().ends_with(".bin") || repath_core::looks_like_bin(bytes);
        if !is_bin {
            continue;
        }
        let Ok(mut tree) = bin_provider.parse_bytes(bytes) else {
            continue;
        };
        let mut visitor = Collapser {
            prefix,
            final_paths: &final_paths,
            changes: 0,
        };
        walk_tree(&mut tree, &mut visitor);
        for link in tree.linked.iter_mut() {
            if let Some(new) = visitor.collapse(link) {
                *link = new;
            }
        }
        if visitor.changes > 0 {
            match bin_provider.write_bytes(&tree) {
                Ok(new_bytes) => {
                    *bytes = new_bytes;
                    changes += visitor.changes;
                }
                Err(e) => tracing::warn!("Failed to write prefix-deduped BIN {path}: {e}"),
            }
        }
    }

    for (hash, path, _) in all_files.iter_mut() {
        if let Some(new) = rename.get(&path.to_lowercase().replace('\\', "/")) {
            tracing::debug!("Deduped stacked prefix: {} -> {}", path, new);
            *path = new.clone();
            *hash = wad_path_hash(new);
            changes += 1;
        }
    }

    let hash_renames: std::collections::HashMap<u64, (u64, String)> = rename
        .iter()
        .map(|(old, new)| (wad_path_hash(old), (wad_path_hash(new), new.clone())))
        .collect();
    changes += rewrite_file_hash_refs(all_files, &hash_renames, bin_provider);

    if changes > 0 {
        tracing::info!("Collapsed stacked repath prefix on {changes} path(s)/string(s)");
    }
    changes
}

/// Output name for a fixed WAD: `Aatrox.wad.client` → `Aatrox.fixed.wad.client`.
/// The `.fixed` marker goes before the `.wad.client` suffix so the output is
/// still a recognizable WAD; non-WAD names just get `.fixed` appended.
pub fn fixed_wad_output_path(path: &Path) -> std::path::PathBuf {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let new_name = match name.to_lowercase().strip_suffix(".wad.client") {
        Some(_) => format!(
            "{}.fixed.wad.client",
            &name[..name.len() - ".wad.client".len()]
        ),
        None => format!("{name}.fixed"),
    };
    path.with_file_name(new_name)
}

/// Run the `post_repath`-phase BIN fixes over every BIN in `all_files`,
/// writing changed BINs back in place. Shared by the folder pipeline and the
/// CLI's mounted-WAD pipeline. In `dry_run` mode detections are still
/// recorded (for `--check`) but nothing is modified.
pub fn apply_post_repath_fixes(
    all_files: &mut [(u64, String, Vec<u8>)],
    config: &FixConfig,
    selected_fixes: &[String],
    champions: &CharacterRelations,
    hash_provider: &Arc<dyn HashProvider>,
    dry_run: bool,
    progress: &dyn ProgressSink,
) -> ProcessResult {
    use hematite_file::wad_adapter::FileWadProvider;
    use hematite_types::config::FixPhase;

    let has_post_rules = config.fixes.iter().any(|(id, r)| {
        config.is_fix_enabled(id) && r.phase == FixPhase::PostRepath && selected_fixes.contains(id)
    });
    if !has_post_rules {
        return ProcessResult::default();
    }

    let bin_provider = FileBinProvider;
    let wad_provider = FileWadProvider::from_hashes(all_files.iter().map(|(h, _, _)| *h).collect());

    let mut total = ProcessResult::default();
    for (_hash, path, bytes) in all_files.iter_mut() {
        let is_bin = path.to_lowercase().ends_with(".bin") || repath_core::looks_like_bin(bytes);
        if !is_bin {
            continue;
        }
        let Ok(tree) = bin_provider.parse_bytes(bytes) else {
            continue;
        };

        let mut ctx = FixContext {
            tree,
            hashes: hash_provider.as_ref(),
            wad: &wad_provider,
            champions,
            files_to_remove: Vec::new(),
            file_path: path.clone(),
            linked_trees: std::collections::HashMap::new(),
            shader_validator: None,
            game: None,
            additional_bins: Vec::new(),
        };

        let mut result = hematite_core::pipeline::apply_fixes_in_phase(
            &mut ctx,
            config,
            selected_fixes,
            dry_run,
            FixPhase::PostRepath,
        );
        result.files_processed = 0;

        if result.fixes_applied > 0 {
            for fix in &result.applied_fixes {
                progress.fix_applied(&fix.fix_name, Some(fix.changes_count));
            }
            if !dry_run {
                match bin_provider.write_bytes(&ctx.tree) {
                    Ok(new_bytes) => *bytes = new_bytes,
                    Err(e) => tracing::warn!("Failed to write post-phase BIN {}: {}", path, e),
                }
            }
        }
        total.merge(result);
    }
    total
}

/// Prime the live `GameIndex` with each seed champion's base WAD plus any
/// related forms (e.g. Anivia → Egg, Annie → Tibbers) so downstream
/// live-game fixes (gear/CAC pull, dead-ref resolution, `--restore-anm`)
/// see the right champion data already loaded. Fails open per-champion —
/// a WAD that isn't found is just a debug log inside `GameIndex::add_champion`.
pub fn prime_champion_wads<'a>(
    live: &LiveGameProvider,
    champions: &CharacterRelations,
    seed_champions: impl Iterator<Item = &'a str>,
) {
    for champ in seed_champions {
        live.with_index(|idx| {
            idx.add_champion(champ);
        });
        if let Some(related) = champions.get_subchamps(champ) {
            for form in related {
                live.with_index(|idx| {
                    idx.add_champion(form);
                });
            }
        }
    }
}

/// Make the mod self-contained by pulling missing dependencies out of the
/// base-game `.wad.client` at `game_wad_path`.
///
/// Thin wrapper over [`crate::deep_repair::resolve_from_game_wad`], which
/// performs seed-BIN backfill (asset-only mods get a foundation skin BIN) and
/// a transitive dependency closure (recursively pull every referenced/linked
/// file until nothing new appears). Extracted files are appended to
/// `all_files`.
///
/// Returns the total number of files pulled from the game WAD.
pub fn extract_missing_from_game_wad(
    game_wad_path: &Path,
    all_files: &mut Vec<(u64, String, Vec<u8>)>,
    bin_provider: &FileBinProvider,
    hash_provider: &dyn HashProvider,
    opts: &RepathOptions,
) -> Result<u32> {
    let stats = crate::deep_repair::resolve_from_game_wad(
        game_wad_path,
        all_files,
        bin_provider,
        hash_provider,
        opts,
    )?;
    Ok(stats.files_pulled)
}

/// Make the mod self-contained by pulling missing dependencies out of the
/// auto-detected, multi-WAD live game index instead of a single explicit
/// `--game-wad` file. Same capabilities as [`extract_missing_from_game_wad`]
/// (seed-BIN backfill + transitive dependency closure) — only the byte
/// source differs.
///
/// Returns the total number of files pulled from the live index.
pub fn extract_missing_from_live(
    live: &LiveGameProvider,
    all_files: &mut Vec<(u64, String, Vec<u8>)>,
    bin_provider: &FileBinProvider,
    hash_provider: &dyn HashProvider,
    opts: &RepathOptions,
) -> Result<u32> {
    let stats =
        crate::deep_repair::resolve_from_live(live, all_files, bin_provider, hash_provider, opts)?;
    Ok(stats.files_pulled)
}

/// `--restore-anm` pipeline step: pull `.anm` animation references that the
/// mod's own BINs point at but doesn't ship, out of the live game (or an
/// explicit `--game-wad`), instead of leaving them dangling for
/// `anm_remover` to delete.
///
/// Runs independently of `--repath` — unlike deep repair's game-file pull
/// (which only fires inside the repath branch because it exists to make a
/// *repathed* mod self-contained), animation restoration is useful for any
/// mod, repathed or not. Source priority mirrors deep repair: an explicit
/// `--game-wad` file wins over the auto-detected live index. Fails open —
/// with neither source available this is an info log and a no-op, never a
/// hard error.
pub fn run_restore_anm(
    all_files: &mut Vec<(u64, String, Vec<u8>)>,
    bin_provider: &FileBinProvider,
    game_wad: Option<&Path>,
    live: Option<&LiveGameProvider>,
) -> u32 {
    use crate::anm_restore::restore_missing_anms;
    use crate::deep_repair::{LiveSource, WadFileSource};

    let stats = if let Some(game_wad_path) = game_wad {
        match WadFileSource::open(game_wad_path) {
            Ok(mut source) => Some(restore_missing_anms(all_files, bin_provider, &mut source)),
            Err(e) => {
                tracing::warn!(
                    "--restore-anm: failed to open --game-wad {}: {}",
                    game_wad_path.display(),
                    e
                );
                None
            }
        }
    } else if let Some(live) = live {
        let mut source = LiveSource::new(live);
        Some(restore_missing_anms(all_files, bin_provider, &mut source))
    } else {
        tracing::info!(
            "--restore-anm: no live game install detected and no --game-wad given — skipping"
        );
        None
    };

    match stats {
        Some(stats) => {
            tracing::info!(
                "Restored {} animation(s), {} unresolved ({} referenced)",
                stats.restored,
                stats.still_missing,
                stats.refs_found
            );
            stats.restored
        }
        None => 0,
    }
}

/// `combo_bin_relocate` pipeline step: re-key legacy
/// `data/<champ>_skins_<slots>.bin` WAD entries to Riot's relocated
/// `data/characters/<champ>/<champ>_multi_skins_<slots>.bin` path, when the
/// game confirms the new path exists and the mod ships no per-skin BINs.
///
/// Source priority mirrors `--restore-anm`/deep repair: an explicit
/// `--game-wad` file wins over the auto-detected live index. Fails open —
/// with neither source available this is an info log and a no-op.
pub fn run_combo_bin_relocate(
    all_files: &mut [(u64, String, Vec<u8>)],
    game_wad: Option<&Path>,
    live: Option<&LiveGameProvider>,
) -> u32 {
    use crate::deep_repair::{GamePullSource, LiveSource, WadFileSource};

    let game_hashes: Option<std::collections::HashSet<u64>> = if let Some(game_wad_path) = game_wad
    {
        match WadFileSource::open(game_wad_path) {
            Ok(source) => Some(source.hashes().clone()),
            Err(e) => {
                tracing::warn!(
                    "combo-bin relocation: failed to open --game-wad {}: {}",
                    game_wad_path.display(),
                    e
                );
                None
            }
        }
    } else if let Some(live) = live {
        Some(LiveSource::new(live).hashes().clone())
    } else {
        tracing::debug!(
            "combo-bin relocation: no live game install detected and no --game-wad given — skipping"
        );
        None
    };

    match game_hashes {
        Some(hashes) => crate::combo_relocate::relocate_combo_bins(all_files, &hashes),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{dedupe_stacked_prefixes, fixed_wad_output_path, rewrite_file_hash_refs};
    use hematite_core::traits::BinProvider;
    use hematite_file::bin_adapter::FileBinProvider;
    use hematite_file::wad_adapter::wad_path_hash;
    use hematite_types::bin::{BinObject, BinProperty, BinTree, PropertyValue};
    use hematite_types::hash::{FieldHash, PathHash, TypeHash};
    use indexmap::IndexMap;
    use std::path::Path;

    fn bin_with_strings(strings: &[&str]) -> Vec<u8> {
        let mut properties = IndexMap::new();
        for (i, s) in strings.iter().enumerate() {
            properties.insert(
                i as u32,
                BinProperty {
                    name_hash: FieldHash(i as u32),
                    value: PropertyValue::String((*s).to_string()),
                },
            );
        }
        let mut objects = IndexMap::new();
        objects.insert(
            1,
            BinObject {
                class_hash: TypeHash(2),
                path_hash: PathHash(1),
                properties,
            },
        );
        FileBinProvider
            .write_bytes(&BinTree {
                objects,
                ..Default::default()
            })
            .unwrap()
    }

    fn strings_of(bytes: &[u8]) -> Vec<String> {
        let tree = FileBinProvider.parse_bytes(bytes).unwrap();
        tree.objects
            .values()
            .flat_map(|o| o.properties.values())
            .filter_map(|p| match &p.value {
                PropertyValue::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn dedupe_repairs_double_fix_damage() {
        // The exact damage shape from the field: chunk + one string carry the
        // stacked prefix, while a file hash from the first fix still points
        // at the single-prefix path.
        let double = "assets/hematite/hematite/sirdexal/icons/travis_square.tex";
        let single = "assets/hematite/sirdexal/icons/travis_square.tex";
        let mut all_files = vec![
            (wad_path_hash(double), double.to_string(), vec![1, 2, 3]),
            (
                wad_path_hash("data/characters/x/skins/skin0.bin"),
                "data/characters/x/skins/skin0.bin".to_string(),
                bin_with_strings(&[double, "assets/characters/x/ok.tex"]),
            ),
        ];

        let changes = dedupe_stacked_prefixes(&mut all_files, "hematite", &FileBinProvider);
        assert_eq!(changes, 2, "one chunk rename + one string rewrite");
        assert_eq!(all_files[0].1, single);
        assert_eq!(all_files[0].0, wad_path_hash(single));
        assert_eq!(
            strings_of(&all_files[1].2),
            vec![single.to_string(), "assets/characters/x/ok.tex".to_string()]
        );
    }

    #[test]
    fn dedupe_keeps_stacked_chunk_when_target_exists() {
        let double = "assets/hematite/hematite/x.tex";
        let single = "assets/hematite/x.tex";
        let mut all_files = vec![
            (wad_path_hash(double), double.to_string(), vec![1]),
            (wad_path_hash(single), single.to_string(), vec![2]),
            (
                wad_path_hash("data/characters/x/skins/skin0.bin"),
                "data/characters/x/skins/skin0.bin".to_string(),
                bin_with_strings(&[double]),
            ),
        ];

        dedupe_stacked_prefixes(&mut all_files, "hematite", &FileBinProvider);
        assert_eq!(all_files[0].1, double, "collision target keeps its path");
        assert_eq!(
            strings_of(&all_files[2].2),
            vec![double.to_string()],
            "string stays consistent with the kept chunk"
        );
    }

    fn bin_with_file_hash(h: u64) -> Vec<u8> {
        let mut properties = IndexMap::new();
        properties.insert(
            1,
            BinProperty {
                name_hash: FieldHash(1),
                value: PropertyValue::WadHash(h),
            },
        );
        let mut objects = IndexMap::new();
        objects.insert(
            1,
            BinObject {
                class_hash: TypeHash(2),
                path_hash: PathHash(1),
                properties,
            },
        );
        FileBinProvider
            .write_bytes(&BinTree {
                objects,
                ..Default::default()
            })
            .unwrap()
    }

    fn file_hashes_of(bytes: &[u8]) -> Vec<u64> {
        let tree = FileBinProvider.parse_bytes(bytes).unwrap();
        hematite_core::repath::collect_bin_asset_hashes(&tree)
    }

    #[test]
    fn rewrite_file_hash_refs_follows_chunk_renames() {
        let old = "assets/rengar_custom/animations/attack1.anm";
        let new = "assets/hematite/rengar_custom/animations/attack1.anm";
        let mut all_files = vec![
            (
                wad_path_hash("data/characters/rengar/skins/skin32.bin"),
                "data/characters/rengar/skins/skin32.bin".to_string(),
                bin_with_file_hash(wad_path_hash(old)),
            ),
            (wad_path_hash(new), new.to_string(), vec![1, 2, 3]),
        ];
        let renames: std::collections::HashMap<u64, (u64, String)> =
            [(wad_path_hash(old), (wad_path_hash(new), new.to_string()))]
                .into_iter()
                .collect();

        let n = rewrite_file_hash_refs(&mut all_files, &renames, &FileBinProvider);
        assert_eq!(n, 1);
        assert_eq!(file_hashes_of(&all_files[0].2), vec![wad_path_hash(new)]);

        let n2 = rewrite_file_hash_refs(&mut all_files, &renames, &FileBinProvider);
        assert_eq!(n2, 0, "second run must be a no-op");
    }

    #[test]
    fn dedupe_rewrites_file_hashes_on_stacked_chunks() {
        let double = "assets/hematite/hematite/sirdexal/icons/travis_square.tex";
        let single = "assets/hematite/sirdexal/icons/travis_square.tex";
        let mut all_files = vec![
            (wad_path_hash(double), double.to_string(), vec![1, 2, 3]),
            (
                wad_path_hash("data/characters/x/skins/skin0.bin"),
                "data/characters/x/skins/skin0.bin".to_string(),
                bin_with_file_hash(wad_path_hash(double)),
            ),
        ];

        let changes = dedupe_stacked_prefixes(&mut all_files, "hematite", &FileBinProvider);
        assert_eq!(changes, 2, "one chunk rename + one hash rewrite");
        assert_eq!(all_files[0].1, single);
        assert_eq!(file_hashes_of(&all_files[1].2), vec![wad_path_hash(single)]);
    }

    #[test]
    fn fixed_wad_name_keeps_single_wad_client_suffix() {
        assert_eq!(
            fixed_wad_output_path(Path::new("mods/Aatrox.wad.client")),
            Path::new("mods/Aatrox.fixed.wad.client")
        );
        assert_eq!(
            fixed_wad_output_path(Path::new("Kayn.WAD.CLIENT")),
            Path::new("Kayn.fixed.wad.client")
        );
        assert_eq!(
            fixed_wad_output_path(Path::new("loose_folder")),
            Path::new("loose_folder.fixed")
        );
    }
}
