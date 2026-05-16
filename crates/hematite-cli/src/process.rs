//! File processing orchestration.
//!
//! Routes input files to the appropriate processing pipeline based on file type.

use anyhow::{Context, Result};
use hematite_core::context::FixContext;
use hematite_core::pipeline::apply_fixes;
use hematite_core::repath as repath_core;
use hematite_core::traits::{BinProvider, HashProvider};
use hematite_core::wad_pipeline::converters::ConverterRegistry;
use hematite_ltk::{
    bin_adapter::LtkBinProvider, hash_adapter::TxtHashProvider,
    lmdb_hash_adapter::LmdbHashProvider, mesh_converter, texture_converter,
    wad_adapter::wad_path_hash,
};
use hematite_types::champion::CharacterRelations;
use hematite_types::config::FixConfig;
use hematite_types::repath::RepathOptions;
use hematite_types::result::{CheckInfo, ProcessResult};
use std::path::Path;
use std::sync::Arc;
use walkdir::WalkDir;

/// Session-level parameters shared by every file processing function.
///
/// Bundles together the options that are constant for the entire run so that
/// individual `process_*` functions stay within Clippy's argument-count limit.
struct ProcessContext<'a> {
    config: &'a FixConfig,
    selected_fixes: &'a [String],
    champions: &'a CharacterRelations,
    dry_run: bool,
    check: bool,
    repath_opts: Option<&'a RepathOptions>,
    /// Live progress reporter. Silent under `-v verbose|trace`, `--json`,
    /// and `-v quiet` — for those flows the existing tracing output or
    /// JSON pipe is the user-facing surface.
    ui: crate::ui::UiReporter,
}

/// Load hash provider with LMDB fallback to TXT.
///
/// Takes the UI reporter so user-visible status (cache redownload,
/// fallback warnings) can be surfaced via the progress bar instead of
/// raw tracing emits that visually collide with the spinner line.
fn load_hash_provider(ui: &crate::ui::UiReporter) -> Result<Arc<dyn HashProvider>> {
    // Auto-download LMDB if missing (skip version check if already exists)
    if let Err(e) = crate::hash_downloader::ensure_hashes_available(false) {
        tracing::warn!("Failed to auto-download hash database: {}", e);
        tracing::info!("Will attempt to use existing files");
    }

    // Try LMDB first
    match LmdbHashProvider::load_from_appdata() {
        Ok(provider) => {
            tracing::info!("Using LMDB hash provider");
            return Ok(Arc::new(provider));
        }
        Err(e) => {
            tracing::warn!("LMDB hash provider unavailable: {}", e);
            // Cache contents we can't parse are worse than no cache.
            // Wipe and re-attempt the download in-process so this run
            // gets WAD lookups instead of silently degrading to slow
            // TXT for the rest of the session.
            crate::hash_downloader::invalidate_cache();
            ui.note("Hash cache was stale — re-downloading the bundle…");
            tracing::info!("Re-downloading fresh hash bundle…");
            match crate::hash_downloader::ensure_hashes_available(true) {
                Ok(()) => match LmdbHashProvider::load_from_appdata() {
                    Ok(provider) => {
                        ui.fix_applied("Hash cache refreshed", None);
                        tracing::info!("Using LMDB hash provider (redownloaded)");
                        return Ok(Arc::new(provider));
                    }
                    Err(e2) => {
                        ui.note(&format!(
                            "Still no usable LMDB after redownload — falling back to slow TXT path ({e2})"
                        ));
                        tracing::warn!(
                            "LMDB still unavailable after redownload: {} — falling back to TXT",
                            e2
                        );
                    }
                },
                Err(e2) => {
                    ui.note(&format!(
                        "Hash redownload failed ({e2}) — falling back to slow TXT path"
                    ));
                    tracing::warn!(
                        "Failed to redownload hash bundle: {} — falling back to TXT",
                        e2
                    );
                }
            }
        }
    }

    // Fallback to TXT
    let txt_provider = TxtHashProvider::load_from_appdata()
        .context("Failed to load hash dictionaries (both LMDB and TXT failed)")?;
    Ok(Arc::new(txt_provider))
}

/// Process input (file or directory).
///
/// The argument list is intentionally flat — these are all session-level
/// settings already validated by clap and the caller. Bundling into a
/// struct would just shuffle the same values one level deeper without
/// genuinely improving call-site clarity, so the lint is suppressed.
#[allow(clippy::too_many_arguments)]
pub fn process_input(
    input: &Path,
    config: &FixConfig,
    selected_fixes: &[String],
    champions: &CharacterRelations,
    dry_run: bool,
    check: bool,
    repath_opts: Option<&RepathOptions>,
    ui: crate::ui::UiReporter,
) -> Result<ProcessResult> {
    // Load hash provider once for all files. The bar shows a stage
    // label so the user sees something happen during the (slow) LMDB
    // load on first run; the reporter is also passed in so the
    // loader can surface its own status (cache redownload etc.) via
    // the same channel instead of raw tracing emits that visually
    // collide with the spinner.
    ui.stage("Loading hash dictionary…");
    let hash_provider = load_hash_provider(&ui)?;

    let ctx = ProcessContext {
        config,
        selected_fixes,
        champions,
        dry_run,
        check,
        repath_opts,
        ui,
    };

    let mut total_result = ProcessResult::default();

    if input.is_dir() {
        for entry in WalkDir::new(input) {
            let entry = entry.context("Failed to read directory entry")?;
            let path = entry.path();

            if path.is_file() && is_supported_file(path) {
                let result = process_file_with_hashes(path, &ctx, &hash_provider)?;
                total_result.merge(result);
            }
        }
    } else {
        total_result = process_file_with_hashes(input, &ctx, &hash_provider)?;
    }

    ctx.ui.finish();
    Ok(total_result)
}

/// Check if a file is a supported type.
fn is_supported_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_lowercase())
        .unwrap_or_default();

    ext == "bin" || ext == "fantome" || ext == "zip" || file_name.ends_with(".wad.client")
}

/// Process a single file based on its type (with hash provider).
fn process_file_with_hashes(
    file: &Path,
    ctx: &ProcessContext<'_>,
    hash_provider: &Arc<dyn HashProvider>,
) -> Result<ProcessResult> {
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let file_name = file
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_lowercase())
        .unwrap_or_default();

    if ext == "bin" {
        process_bin_file(file, ctx, hash_provider)
    } else if file_name.ends_with(".wad.client") {
        process_wad_file(file, ctx, hash_provider)
    } else if ext == "fantome" || ext == "zip" {
        process_fantome_file(file, ctx, hash_provider)
    } else {
        anyhow::bail!("Unsupported file type: {}", file.display());
    }
}

/// Process a single .bin file.
fn process_bin_file(
    file: &Path,
    ctx: &ProcessContext<'_>,
    hash_provider: &Arc<dyn HashProvider>,
) -> Result<ProcessResult> {
    let (config, selected_fixes, champions, dry_run, check) = (
        ctx.config,
        ctx.selected_fixes,
        ctx.champions,
        ctx.dry_run,
        ctx.check,
    );
    let ui = ctx.ui.clone();
    ui.stage(&format!(
        "Processing {}…",
        file.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("BIN")
    ));
    tracing::info!("Processing BIN: {}", file.display());

    // Initialize BIN provider
    let bin_provider = LtkBinProvider;

    // Read BIN file
    let bytes = std::fs::read(file).context("Failed to read BIN file")?;
    let tree = bin_provider
        .parse_bytes(&bytes)
        .context("Failed to parse BIN file")?;

    // Standalone BIN has no WAD context
    struct NullWadProvider;
    impl hematite_core::traits::WadProvider for NullWadProvider {
        fn has_path(&self, _path: &str) -> bool {
            false
        }
        fn has_hash(&self, _hash: u64) -> bool {
            false
        }
    }
    let null_wad = NullWadProvider;

    // Load shader validator (optional, graceful if unavailable)
    let shader_validator = hematite_core::detect::shader::ShaderValidator::load()
        .ok()
        .filter(|v| v.is_available());

    // Create fix context
    let mut ctx = FixContext {
        tree,
        hashes: hash_provider.as_ref(),
        wad: &null_wad,
        champions,
        files_to_remove: Vec::new(),
        file_path: file.to_string_lossy().to_string(),
        linked_trees: std::collections::HashMap::new(),
        shader_validator: shader_validator.as_ref(),
        additional_bins: Vec::new(),
    };

    // Run fixes
    let mut result = apply_fixes(&mut ctx, config, selected_fixes, dry_run);

    // In check mode, populate CheckInfo from detected issues
    if check {
        let detected: Vec<String> = result
            .applied_fixes
            .iter()
            .map(|f| f.fix_name.clone())
            .collect();
        result.check_info = Some(CheckInfo {
            champion: None,
            skin_number: None,
            is_binless: true, // standalone BIN = no WAD context
            detected_issues: detected,
        });
    }

    // Write back if changes were made and not dry-run
    if !dry_run && result.fixes_applied > 0 {
        let modified_bytes = bin_provider
            .write_bytes(&ctx.tree)
            .context("Failed to write modified BIN file")?;

        // Write to output file (original.bin → original.fixed.bin)
        let output_path = file.with_extension("fixed.bin");
        std::fs::write(&output_path, &modified_bytes)
            .context("Failed to save modified BIN file")?;

        for fix in &result.applied_fixes {
            ui.fix_applied(&fix.fix_name, Some(fix.changes_count));
        }
        ui.fix_applied(&format!("Wrote {}", output_path.display()), None);

        tracing::info!("✓ Wrote fixed BIN to: {}", output_path.display());
        for fix in &result.applied_fixes {
            tracing::info!(
                "  ✓ {} ({} changes)",
                fix.fix_name,
                fix.changes_count
            );
        }
        tracing::info!(
            "  Total: {} fixes, {} bytes written",
            result.fixes_applied,
            modified_bytes.len()
        );
    }

    Ok(result)
}

/// Process a .wad.client file.
///
/// Extracts files from the WAD, runs WAD-level and BIN-level fix pipelines,
/// and reports results. Writing modified files back is not yet supported.
fn process_wad_file(
    file: &Path,
    ctx: &ProcessContext<'_>,
    hash_provider: &Arc<dyn HashProvider>,
) -> Result<ProcessResult> {
    let (config, selected_fixes, champions, dry_run, check, repath_opts) = (
        ctx.config,
        ctx.selected_fixes,
        ctx.champions,
        ctx.dry_run,
        ctx.check,
        ctx.repath_opts,
    );
    use hematite_core::wad_pipeline;
    use hematite_ltk::wad_adapter::WadFile;

    tracing::info!("Processing WAD: {}", file.display());

    let bin_provider = LtkBinProvider;

    let mut wad_file = WadFile::open(file).context("Failed to open WAD file")?;

    let wad_provider = wad_file.build_provider();

    // Extract all files for WAD-level pipeline (mutable for conversions)
    let mut all_files = wad_file
        .extract_all_files(hash_provider.as_ref())
        .context("Failed to extract files from WAD")?;

    // Identify BIN entries by content magic, not just by path extension —
    // mods commonly ship BINs whose path-hash isn't in the dictionary, in
    // which case the resolved "path" is a hex string and the `.bin` filter
    // would skip them.  Magic detection catches both.
    let bin_chunks: Vec<_> = all_files
        .iter()
        .filter(|(_h, path, bytes)| {
            path.to_lowercase().ends_with(".bin") || repath_core::looks_like_bin(bytes)
        })
        .cloned()
        .collect();

    tracing::info!(
        "WAD has {} total entries, {} BIN file(s)",
        wad_provider.hash_count(),
        bin_chunks.len()
    );

    // Discover champion/skin seeds from the resolved TOC. Surfaces
    // subcharacters that ship alongside the primary champion (e.g.
    // jinxmine alongside jinx) so the user can see they're being
    // processed and downstream pipeline steps can iterate over them.
    {
        let seeds = hematite_core::seeds::discover_seeds(
            all_files.iter().map(|(_, p, _)| p.as_str()),
        );
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
            // Per-WAD subchampion note — surfaces via the live UI in
            // Normal mode so the user immediately knows secondary forms
            // are being handled, even though we hide everything else.
            if unique_champs.len() > 1 {
                let mut names: Vec<&str> = unique_champs.iter().copied().collect();
                names.sort();
                ctx.ui.note(&format!(
                    "WAD contains subchampion forms: {}",
                    names.join(", ")
                ));
                tracing::info!(
                    "WAD contains subchampion forms: {}",
                    names.join(", ")
                );
            }
        }
    }

    let mut total_result = ProcessResult::default();
    let mut shared_files_to_remove = Vec::new();

    // === WAD-LEVEL PIPELINE ===
    // Run file-level fixes (BNK removal, format conversions, etc.)
    ctx.ui.stage("Detecting WAD-level issues…");
    tracing::debug!("Running WAD-level pipeline...");
    let wad_output = wad_pipeline::apply_wad_fixes(&all_files, config, selected_fixes, hash_provider.as_ref())?;

    // Collect files to remove from WAD-level fixes
    shared_files_to_remove.extend(wad_output.files_to_remove.clone());

    // Track WAD-level fixes applied
    for wad_fix in &wad_output.applied_fixes {
        ctx.ui.fix_applied(&wad_fix.fix_name, Some(wad_fix.files_affected));
        tracing::info!(
            "WAD-level fix '{}' affected {} files",
            wad_fix.fix_name,
            wad_fix.files_affected
        );
        total_result.fixes_applied += wad_fix.files_affected;
    }

    // Perform file format conversions
    let mut converter_registry = ConverterRegistry::new();
    // Register LTK-based converters (override placeholders)
    converter_registry.register("dds_to_tex", texture_converter::dds_to_tex);
    converter_registry.register("sco_to_scb", mesh_converter::sco_to_scb);
    // In-place byte transforms — same registry, but addressed via
    // `WadTransformAction::TransformBytes` rules.
    converter_registry.register("strip_mipmaps", hematite_ltk::strip_mipmaps::strip_mipmaps_auto);
    converter_registry.register("fix_tex_dims", hematite_ltk::fix_dimensions::fix_dimensions_auto);

    let mut conversion_count = 0u32;
    if !wad_output.files_to_convert.is_empty() {
        tracing::info!(
            "Converting {} file formats...",
            wad_output.files_to_convert.len()
        );

        for conversion in &wad_output.files_to_convert {
            // Find the file in all_files
            if let Some((_, _, bytes)) = all_files.iter_mut().find(|(_, p, _)| p == &conversion.path) {
                match converter_registry.convert(&conversion.converter, bytes) {
                    Ok(converted_bytes) => {
                        let old_size = bytes.len();
                        *bytes = converted_bytes;
                        conversion_count += 1;
                        tracing::info!(
                            "✓ Converted {} from .{} to .{} ({} → {} bytes)",
                            conversion.path,
                            conversion.from_ext,
                            conversion.to_ext,
                            old_size,
                            bytes.len()
                        );
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

    // Perform in-place byte transforms (mipmap strip, dimension fix, ...).
    // Same converter registry as `files_to_convert`; the only difference is
    // we don't touch paths/extensions.
    let mut transform_count = 0u32;
    if !wad_output.files_to_transform.is_empty() {
        tracing::info!(
            "Applying {} in-place byte transforms...",
            wad_output.files_to_transform.len()
        );
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
                            *bytes = new_bytes;
                            transform_count += 1;
                        } else {
                            tracing::debug!(
                                "{} via {}: no change emitted (likely a no-op)",
                                op.path,
                                op.converter
                            );
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
        total_result.fixes_applied += transform_count;
    }

    // Append injected files (fallback textures, placeholder assets, ...).
    // Bytes are resolved through `hematite_core::assets` — the registry
    // keeps the blob list out of config and out of the pipeline.
    let mut added_count = 0u32;
    if !wad_output.files_to_add.is_empty() {
        tracing::info!(
            "Injecting {} fallback asset(s)...",
            wad_output.files_to_add.len()
        );
        // Build a fresh paths-in-WAD set so `only_if_missing` honours
        // anything we've already added during this loop.
        let mut paths_in_wad: std::collections::HashSet<String> = all_files
            .iter()
            .map(|(_, p, _)| p.to_lowercase())
            .collect();
        for addition in &wad_output.files_to_add {
            let lower = addition.path.to_lowercase();
            if addition.only_if_missing && paths_in_wad.contains(&lower) {
                tracing::debug!(
                    "Skipping injection of {} ({} already present)",
                    addition.asset,
                    addition.path
                );
                continue;
            }
            let Some(bytes) = hematite_core::assets::get(&addition.asset) else {
                tracing::warn!(
                    "Asset '{}' not registered; skipping injection at {}",
                    addition.asset,
                    addition.path
                );
                continue;
            };
            let hash = wad_path_hash(&addition.path);
            all_files.push((hash, addition.path.clone(), bytes.to_vec()));
            paths_in_wad.insert(lower);
            added_count += 1;
            tracing::info!(
                "✓ Injected asset '{}' at {} ({} bytes)",
                addition.asset,
                addition.path,
                bytes.len()
            );
        }
        total_result.fixes_applied += added_count;
    }

    // === LINKED BIN RESOLUTION (BFS) ===
    // Parse all BINs, resolve linked dependencies from WAD files
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
                // Not actionable for end users — a malformed BIN
                // inside the mod usually means a custom container
                // type the LTK parser doesn't recognise yet. Demote
                // to debug so it doesn't muddy the Normal-mode UI.
                tracing::debug!("Failed to parse BIN {path}: {e}");
            }
        }
    }

    // BFS: resolve linked dependencies that exist in the WAD
    while let Some(linked_path) = queue.pop_front() {
        if parsed_bins.contains_key(&linked_path) {
            continue;
        }
        // Try to find this linked BIN in the extracted files
        if let Some((_, _, bytes)) = all_files.iter().find(|(_, p, _)| *p == linked_path) {
            match bin_provider.parse_bytes(bytes) {
                Ok(tree) => {
                    for dep in &tree.linked {
                        if !parsed_bins.contains_key(dep) {
                            queue.push_back(dep.clone());
                        }
                    }
                    tracing::debug!("Resolved linked BIN: {}", linked_path);
                    parsed_bins.insert(linked_path, tree);
                }
                Err(e) => {
                    tracing::debug!("Failed to parse linked BIN {}: {}", linked_path, e);
                }
            }
        } else {
            tracing::debug!("Linked BIN not found in WAD: {}", linked_path);
        }
    }

    // Separate primary BINs (from bin_chunks) from linked-only trees
    let primary_bin_paths: std::collections::HashSet<String> =
        bin_chunks.iter().map(|(_, p, _)| p.clone()).collect();
    let linked_only: std::collections::HashMap<String, hematite_types::bin::BinTree> = parsed_bins
        .iter()
        .filter(|(k, _)| !primary_bin_paths.contains(*k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // === BIN-LEVEL PIPELINE ===

    // Load shader validator once for all BIN files
    let shader_validator = hematite_core::detect::shader::ShaderValidator::load()
        .ok()
        .filter(|v| v.is_available());

    // Snapshot the UI handle before the loop — the loop body shadows
    // `ctx` for the per-BIN FixContext, so the outer ProcessContext
    // ref would otherwise be inaccessible from inside.
    let ui = ctx.ui.clone();
    ui.stage("Applying fixes…");
    ui.set_length(bin_chunks.len() as u64);

    // Process primary BIN files
    for (_, path, _) in &bin_chunks {
        let Some(tree) = parsed_bins.remove(path) else {
            ui.tick();
            continue; // Already warned during parse
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
            additional_bins: Vec::new(),
        };

        let result = apply_fixes(&mut ctx, config, selected_fixes, dry_run);

        // Surface each applied fix individually via the UI bar (clean
        // green-tick line above the bar) plus a full developer log
        // line under -v verbose.
        if result.fixes_applied > 0 {
            for fix in &result.applied_fixes {
                ui.fix_applied(&fix.fix_name, Some(fix.changes_count));
            }
            tracing::info!("  {} - {} fixes applied:", path, result.fixes_applied);
            for fix in &result.applied_fixes {
                tracing::info!(
                    "    ✓ {} ({} changes)",
                    fix.fix_name,
                    fix.changes_count
                );
            }
        }
        ui.tick();

        let fixes_applied = result.fixes_applied;
        total_result.merge(result);

        // Write modified BIN back to all_files collection
        if !dry_run && fixes_applied > 0 {
            match bin_provider.write_bytes(&ctx.tree) {
                Ok(modified_bytes) => {
                    // Update the BIN bytes in all_files
                    if let Some((_, _, file_bytes)) = all_files.iter_mut().find(|(_, p, _)| p == path) {
                        let old_size = file_bytes.len();
                        *file_bytes = modified_bytes;
                        tracing::debug!(
                            "Updated BIN {} in WAD ({} → {} bytes)",
                            path,
                            old_size,
                            file_bytes.len()
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to write modified BIN {}: {}", path, e);
                }
            }
        }

        // Collect files marked for removal from this BIN context
        shared_files_to_remove.extend(ctx.files_to_remove);

        // Materialise BINs produced by SplitEntriesByType (and any other
        // transform that emits sibling BINs). These need to land in the
        // rebuilt WAD as standalone chunks.
        if !dry_run && !ctx.additional_bins.is_empty() {
            for (new_path, new_tree) in &ctx.additional_bins {
                match bin_provider.write_bytes(new_tree) {
                    Ok(bytes) => {
                        let hash = wad_path_hash(new_path);
                        // If a chunk already exists at this hash (re-running
                        // an already-split BIN, or template collision),
                        // overwrite the bytes so the latest split wins.
                        if let Some((_, _, existing)) =
                            all_files.iter_mut().find(|(h, _, _)| *h == hash)
                        {
                            *existing = bytes;
                            tracing::debug!(
                                source = %path,
                                new_bin = %new_path,
                                "Replaced existing chunk for split-BIN output"
                            );
                        } else {
                            tracing::info!(
                                source = %path,
                                new_bin = %new_path,
                                "Adding split-BIN output to WAD ({} bytes)",
                                bytes.len()
                            );
                            all_files.push((hash, new_path.clone(), bytes));
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to serialize split-BIN output {}: {}",
                            new_path,
                            e
                        );
                    }
                }
            }
        }
    }

    // Update total files removed count
    total_result.files_removed = shared_files_to_remove.len() as u32;

    // === REPATH PIPELINE ===
    // Must run AFTER all BIN fixes so fixes operate on original paths.
    if let Some(opts) = repath_opts {
        if !dry_run {
            ui.stage(&format!("Repathing assets (prefix “{}”)…", opts.prefix));
            tracing::info!(
                "Repathing assets with prefix \"{}\" (layout: {:?})...",
                opts.prefix,
                opts.layout
            );

            // 0. If --game-wad is provided, extract missing referenced files
            //    from the base-game WAD so the mod becomes fully self-contained.
            let mut game_files_added = 0u32;
            if let Some(ref game_wad_path) = opts.game_wad {
                game_files_added = extract_missing_from_game_wad(
                    game_wad_path,
                    &mut all_files,
                    &bin_provider,
                    hash_provider.as_ref(),
                    opts,
                )?;
            }

            // 1. Build a path+hash index of every WAD entry so BIN strings
            //    can match against custom-hashed entries (no resolved path).
            let index = repath_core::WadIndex::from_entries(
                all_files.iter().map(|(h, p, _)| (*h, p.clone())),
            );

            // 2. Walk every BIN in the WAD (identified by extension OR magic)
            //    and rewrite asset references that point to files we ship.
            //    The mapping returned tells us how to rename the WAD entries.
            let mut combined_mapping: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let mut repath_bin_count = 0u32;
            let mut bins_touched = 0u32;

            for (_h, path, bytes) in all_files.iter_mut() {
                let is_bin = path.to_lowercase().ends_with(".bin")
                    || repath_core::looks_like_bin(bytes);
                if !is_bin {
                    continue;
                }
                let mut tree = match bin_provider.parse_bytes(bytes) {
                    Ok(t) => t,
                    Err(e) => {
                        // Same reasoning as the upstream BIN-parse
                        // warning: not actionable for end users,
                        // suppress in Normal mode but keep under -v.
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

            // 3. Rename WAD entries.  Match priority:
            //    a) WAD entry's resolved path is the key in `combined_mapping`
            //       (BIN reference said "assets/foo" and the mod ships an
            //        entry whose dictionary path is "assets/foo"),
            //    b) WAD entry's PATH-HASH equals `xxhash64(orig)` of some BIN
            //       reference — this catches custom-hashed mods where the
            //       entry has no dictionary path,
            //    c) WAD entry's path itself is a recognisable asset path with
            //       no mapping → fall back to `repath_wad_path`.
            //
            //    Build the hash→new_path index once.
            let hash_mapping: std::collections::HashMap<u64, String> = combined_mapping
                .iter()
                .map(|(orig, new)| (wad_path_hash(orig), new.clone()))
                .collect();

            let mut repath_wad_count = 0u32;
            let mut new_path_set: Vec<String> = Vec::new();
            let mut seen_dest: std::collections::HashMap<String, u32> =
                std::collections::HashMap::new();

            let repathed: Vec<(u64, String, Vec<u8>)> = all_files
                .drain(..)
                .map(|(hash, path, bytes)| {
                    let lower = path.to_lowercase();

                    // Pick the new path in priority order: BIN mapping (by
                    // path or by hash) → wad path transform → no change.
                    let new_path_opt: Option<String> = combined_mapping
                        .get(&lower)
                        .cloned()
                        .or_else(|| hash_mapping.get(&hash).cloned())
                        .or_else(|| {
                            repath_core::repath_wad_path(&path, &opts.prefix, opts.layout)
                        });

                    let final_path = match new_path_opt {
                        Some(np) => {
                            // Topaz-style collision dedup: if we'd produce a
                            // duplicate destination, append _1, _2, ...
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
                        (new_hash, final_path, bytes)
                    } else {
                        (hash, path, bytes)
                    }
                })
                .collect();
            all_files = repathed;

            tracing::info!(
                "  {} string(s) in {} BIN(s) repathed; {} WAD entry/entries renamed; \
                 {} pulled from game WAD",
                repath_bin_count,
                bins_touched,
                repath_wad_count,
                game_files_added
            );

            if repath_bin_count == 0 && repath_wad_count == 0 {
                tracing::warn!(
                    "  Nothing was repathed. This usually means the mod is binless or \
                     ships pre-hashed paths the dictionary doesn't recognise. \
                     Try --game-wad <path/to/champion.wad.client> to provide reference paths."
                );
            } else {
                total_result.fixes_applied += 1;
            }

            // 4. Inject invisible placeholders for repathed texture references
            //    that have no real file backing them.
            if opts.invis_texture && !new_path_set.is_empty() {
                let existing: std::collections::HashSet<String> =
                    all_files.iter().map(|(_, p, _)| p.to_lowercase()).collect();
                let placeholders =
                    repath_core::missing_invis_placeholders(&existing, &new_path_set);
                if !placeholders.is_empty() {
                    tracing::info!(
                        "  Injecting {} invis placeholder(s)...",
                        placeholders.len()
                    );
                    for (path, bytes) in placeholders {
                        let hash = wad_path_hash(&path);
                        tracing::debug!("  + invis placeholder: {}", path);
                        all_files.push((hash, path, bytes));
                    }
                }
            }
        } else {
            tracing::info!(
                "[dry-run] Would repath assets with prefix \"{}\" (layout: {:?}){}",
                opts.prefix,
                opts.layout,
                if opts.invis_texture {
                    " + invis placeholders"
                } else {
                    ""
                }
            );
        }
    }

    // In check mode, populate CheckInfo with skin detection
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

    // === WAD REBUILDING ===
    // Write modified WAD if any changes were made and not dry-run
    if !dry_run && (total_result.fixes_applied > 0 || !shared_files_to_remove.is_empty()) {
        ui.stage("Rebuilding WAD…");
        tracing::info!("Building modified WAD...");

        let output_path = file.with_extension("fixed.wad.client");
        let mut output_file =
            std::fs::File::create(&output_path).context("Failed to create output WAD file")?;

        let chunks_included =
            hematite_ltk::wad_builder::build_wad(&all_files, &shared_files_to_remove, &mut output_file)
                .context("Failed to build output WAD")?;

        // Only surface the WAD path to the UI when it's the real
        // user-visible output. WAD writes inside a temp dir are
        // intermediate steps of the fantome repack flow — printing
        // them would tell the user to look at a path that gets
        // wiped seconds later. The fantome repack itself reports
        // the actual final path via ui.fix_applied.
        let is_intermediate = output_path.starts_with(std::env::temp_dir());
        if !is_intermediate {
            ui.fix_applied(
                &format!("Wrote {}", output_path.display()),
                None,
            );
        }
        tracing::info!("✓ Wrote fixed WAD to: {}", output_path.display());
        tracing::info!(
            "  {} chunks included, {} files removed",
            chunks_included,
            shared_files_to_remove.len()
        );
        tracing::info!("  {} total fixes applied", total_result.fixes_applied);
    } else if !dry_run {
        ui.note("No changes detected — WAD not modified.");
        tracing::info!("No changes detected - WAD not modified");
    }

    Ok(total_result)
}

/// Process a .fantome or .zip file.
///
/// Extracts WAD files from the ZIP archive and processes each one.
fn process_fantome_file(
    file: &Path,
    ctx: &ProcessContext<'_>,
    hash_provider: &Arc<dyn HashProvider>,
) -> Result<ProcessResult> {
    let dry_run = ctx.dry_run;
    tracing::info!("Processing Fantome: {}", file.display());

    let zip_file = std::fs::File::open(file).context("Failed to open fantome/zip file")?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(zip_file))
        .context("Failed to read ZIP archive")?;

    // Extract .wad.client files to temp dir
    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;

    // SECURITY: Limits to prevent DoS attacks (ZIP bombs, memory exhaustion)
    const MAX_ENTRIES: usize = 1000;
    const MAX_FILE_SIZE: u64 = 500 * 1024 * 1024; // 500MB per file
    const MAX_TOTAL_SIZE: u64 = 2 * 1024 * 1024 * 1024; // 2GB total

    if archive.len() > MAX_ENTRIES {
        anyhow::bail!(
            "ZIP archive contains too many entries ({} > {}). Possible ZIP bomb attack.",
            archive.len(),
            MAX_ENTRIES
        );
    }

    let mut total_extracted_size: u64 = 0;
    let mut wad_paths = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).context("Failed to read ZIP entry")?;

        let name = entry.name().to_lowercase();
        if name.ends_with(".wad.client") {
            // SECURITY: Validate ZIP entry path to prevent path traversal attacks
            let entry_name = entry.name();

            // Check for path traversal patterns
            if entry_name.contains("..") || std::path::Path::new(entry_name).is_absolute() {
                anyhow::bail!(
                    "Invalid ZIP entry path (potential path traversal): {}",
                    entry_name
                );
            }

            // Additional check: ensure no path component is exactly ".."
            if entry_name.split('/').any(|component| component == "..")
                || entry_name.split('\\').any(|component| component == "..")
            {
                anyhow::bail!(
                    "Invalid ZIP entry path (contains .. component): {}",
                    entry_name
                );
            }

            // SECURITY: Check uncompressed size before extraction
            let uncompressed_size = entry.size();
            if uncompressed_size > MAX_FILE_SIZE {
                anyhow::bail!(
                    "ZIP entry '{}' is too large ({} bytes > {} bytes limit). Possible ZIP bomb.",
                    entry_name,
                    uncompressed_size,
                    MAX_FILE_SIZE
                );
            }

            // SECURITY: Check total extracted size
            total_extracted_size = total_extracted_size.saturating_add(uncompressed_size);
            if total_extracted_size > MAX_TOTAL_SIZE {
                anyhow::bail!(
                    "Total extracted size exceeds limit ({} bytes > {} bytes). Possible ZIP bomb.",
                    total_extracted_size,
                    MAX_TOTAL_SIZE
                );
            }

            let dest = temp_dir.path().join(entry_name);

            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut out)?;
            wad_paths.push(dest);
        }
    }

    if wad_paths.is_empty() {
        tracing::warn!("No .wad.client files found in {}", file.display());
        return Ok(ProcessResult::default());
    }

    tracing::info!("Found {} WAD file(s) in archive", wad_paths.len());

    let mut total_result = ProcessResult::default();
    for wad_path in &wad_paths {
        let result = process_wad_file(wad_path, ctx, hash_provider)?;
        total_result.merge(result);
    }

    // === FANTOME REPACK ===
    // Rebuild the fantome ZIP with fixed WADs replacing the originals
    if !dry_run && total_result.fixes_applied > 0 {
        let output_path = file.with_extension("fixed.fantome");

        tracing::info!("Repacking fantome archive...");

        // Re-open the original ZIP to copy non-WAD entries
        let original_zip_file =
            std::fs::File::open(file).context("Failed to re-open original fantome")?;
        let mut original_archive =
            zip::ZipArchive::new(std::io::BufReader::new(original_zip_file))
                .context("Failed to re-read original ZIP")?;

        let output_file =
            std::fs::File::create(&output_path).context("Failed to create output fantome")?;
        let mut zip_writer = zip::ZipWriter::new(std::io::BufWriter::new(output_file));

        let zip_options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

        for i in 0..original_archive.len() {
            let mut entry = original_archive.by_index(i)?;
            let entry_name = entry.name().to_string();
            let is_wad = entry_name.to_lowercase().ends_with(".wad.client");

            if is_wad {
                // Use the fixed WAD if it exists, otherwise copy original
                let fixed_wad_path = temp_dir
                    .path()
                    .join(&entry_name)
                    .with_extension("fixed.wad.client");

                if fixed_wad_path.exists() {
                    let fixed_bytes = std::fs::read(&fixed_wad_path)
                        .context("Failed to read fixed WAD from temp")?;
                    zip_writer.start_file(&entry_name, zip_options)?;
                    std::io::Write::write_all(&mut zip_writer, &fixed_bytes)?;
                    tracing::debug!("Repacked fixed WAD: {}", entry_name);
                } else {
                    // No fixes applied to this WAD, copy original
                    zip_writer.start_file(&entry_name, zip_options)?;
                    std::io::copy(&mut entry, &mut zip_writer)?;
                    tracing::debug!("Repacked original WAD: {}", entry_name);
                }
            } else {
                // Copy non-WAD entries as-is (META/info.json, etc.)
                zip_writer.start_file(&entry_name, zip_options)?;
                std::io::copy(&mut entry, &mut zip_writer)?;
            }
        }

        zip_writer.finish()?;

        ctx.ui.fix_applied(
            &format!("Wrote {}", output_path.display()),
            None,
        );
        tracing::info!("✓ Wrote fixed fantome to: {}", output_path.display());
        tracing::info!("  {} total fixes applied", total_result.fixes_applied);
    } else if !dry_run {
        ctx.ui.note("No changes detected — fantome not modified.");
        tracing::info!("No changes detected - fantome not modified");
    }

    Ok(total_result)
}

/// Extract files referenced by BIN strings but missing from the mod WAD.
///
/// Opens the base-game `.wad.client` at `game_wad_path`, scans all BIN files
/// in `all_files` for asset-path strings, identifies which ones are missing
/// from the mod, and extracts them from the game WAD.  The extracted files are
/// appended to `all_files` so the subsequent repath step can repath everything.
///
/// Returns the number of files pulled from the game WAD.
fn extract_missing_from_game_wad(
    game_wad_path: &Path,
    all_files: &mut Vec<(u64, String, Vec<u8>)>,
    bin_provider: &LtkBinProvider,
    hash_provider: &dyn HashProvider,
    opts: &RepathOptions,
) -> Result<u32> {
    use hematite_ltk::wad_adapter::WadFile;

    tracing::info!(
        "Opening game WAD for missing file extraction: {}",
        game_wad_path.display()
    );

    // 1. Collect all asset paths referenced by BIN files in the mod.
    //    Identify BINs by extension OR magic so unresolved entries are caught.
    let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (_, path, bytes) in all_files.iter() {
        let is_bin =
            path.to_lowercase().ends_with(".bin") || repath_core::looks_like_bin(bytes);
        if !is_bin {
            continue;
        }
        if let Ok(tree) = bin_provider.parse_bytes(bytes) {
            let paths = repath_core::collect_bin_asset_paths(&tree, opts.skip_vo);
            referenced.extend(paths);
        }
    }

    // 2. Build a path+hash index of what the mod already ships.
    let mod_index = repath_core::WadIndex::from_entries(
        all_files.iter().map(|(h, p, _)| (*h, p.clone())),
    );

    // 3. Determine which referenced paths are missing (alternates + hash).
    let missing: Vec<String> = referenced
        .into_iter()
        .filter(|p| !mod_index.has(p, wad_path_hash))
        .collect();

    if missing.is_empty() {
        tracing::info!("  All referenced files already present in mod — nothing to pull");
        return Ok(0);
    }

    tracing::info!(
        "  {} referenced file(s) missing from mod, extracting from game WAD...",
        missing.len()
    );

    // 4. Open game WAD and build a hash→path lookup for extraction.
    let mut game_wad = WadFile::open(game_wad_path)
        .with_context(|| format!("Failed to open game WAD: {}", game_wad_path.display()))?;

    let game_files = game_wad
        .extract_all_files(hash_provider)
        .context("Failed to extract game WAD")?;

    // Build lowercased path → index lookup for the game WAD
    let game_lookup: std::collections::HashMap<String, usize> = game_files
        .iter()
        .enumerate()
        .map(|(i, (_, p, _))| (p.to_lowercase(), i))
        .collect();

    // 5. Pull each missing file from the game WAD.
    let mut added = 0u32;
    for missing_path in &missing {
        // Try exact match first, then extension alternates
        let found_idx = game_lookup.get(missing_path).copied().or_else(|| {
            // .dds ↔ .tex
            if let Some(stem) = missing_path.strip_suffix(".dds") {
                game_lookup.get(&format!("{}.tex", stem)).copied()
            } else if let Some(stem) = missing_path.strip_suffix(".tex") {
                game_lookup.get(&format!("{}.dds", stem)).copied()
            } else if let Some(stem) = missing_path.strip_suffix(".sco") {
                game_lookup.get(&format!("{}.scb", stem)).copied()
            } else if let Some(stem) = missing_path.strip_suffix(".scb") {
                game_lookup.get(&format!("{}.sco", stem)).copied()
            } else {
                None
            }
        });

        if let Some(idx) = found_idx {
            let (hash, path, bytes) = &game_files[idx];
            all_files.push((*hash, path.clone(), bytes.clone()));
            added += 1;
            tracing::debug!("  + pulled from game: {}", path);
        } else {
            tracing::debug!("  - not found in game WAD: {}", missing_path);
        }
    }

    tracing::info!(
        "  Pulled {} file(s) from game WAD ({} still missing)",
        added,
        missing.len() as u32 - added
    );

    Ok(added)
}
