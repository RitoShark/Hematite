//! WAD-level fix pipeline — file operations before BIN parsing.
//!
//! This pipeline handles file-level operations that don't require parsing BIN files:
//! - File removal based on version/format checks
//! - File format conversions (DDS→TEX, SCO→SCB)
//! - File renaming/path transformations
//!
//! ## Architecture
//! ```text
//! WAD file → extract file list
//!          → for each file:
//!             - check WadDetectionRule
//!             - if match: apply WadTransformAction
//!          → return modified file list
//! ```
//!
//! ## Modules
//! - [`detect`] — WAD-level detection (extension, binary headers)
//! - [`transform`] — WAD-level actions (remove, convert, rename)
//! - [`converters`] — File format converters registry

pub mod converters;
pub mod detect;
pub mod transform;

use crate::traits::HashProvider;
use anyhow::Result;
use hematite_types::config::{FixConfig, WadDetectionRule, WadFixRule, WadTransformAction};

/// Result of applying a single WAD-level fix.
#[derive(Debug, Clone)]
pub struct WadFixResult {
    pub fix_id: String,
    pub fix_name: String,
    pub files_affected: u32,
}

/// Apply WAD-level fixes to a list of files.
///
/// Returns a list of file operations to perform (remove, convert, rename).
pub fn apply_wad_fixes(
    files: &[(u64, String, Vec<u8>)],
    config: &FixConfig,
    selected_fix_ids: &[String],
    hash_provider: &dyn HashProvider,
) -> Result<WadFixOutput> {
    let mut output = WadFixOutput::default();

    for fix_id in selected_fix_ids {
        let Some(fix_rule) = config.wad_fixes.get(fix_id) else {
            continue;
        };

        if !fix_rule.enabled {
            continue;
        }

        let result = apply_single_fix(files, fix_rule, fix_id, hash_provider)?;
        output.merge(result);
    }

    Ok(output)
}

/// Output of WAD-level fix pipeline.
#[derive(Debug, Default, Clone)]
pub struct WadFixOutput {
    /// Files to remove (by path)
    pub files_to_remove: Vec<String>,
    /// Files to convert (path, from_ext, to_ext, converter_name)
    pub files_to_convert: Vec<FileConversion>,
    /// Files to transform in place (path, converter_name).
    pub files_to_transform: Vec<InPlaceTransform>,
    /// Files to rename (old_path, new_path)
    pub files_to_rename: Vec<(String, String)>,
    /// New files to inject (path, asset_name). Bytes are resolved via the
    /// CLI's asset registry — that keeps `hematite-core` free of any
    /// hard-coded blob list.
    pub files_to_add: Vec<FileAddition>,
    /// Applied fixes summary
    pub applied_fixes: Vec<WadFixResult>,
}

#[derive(Debug, Clone)]
pub struct FileConversion {
    pub path: String,
    pub from_ext: String,
    pub to_ext: String,
    pub converter: String,
}

#[derive(Debug, Clone)]
pub struct InPlaceTransform {
    pub path: String,
    pub converter: String,
}

#[derive(Debug, Clone)]
pub struct FileAddition {
    /// Logical asset name (matches the registry key).
    pub asset: String,
    /// Target WAD path.
    pub path: String,
    /// Skip when the WAD already contains a file at `path`.
    pub only_if_missing: bool,
}

impl WadFixOutput {
    fn merge(&mut self, other: WadFixOutput) {
        self.files_to_remove.extend(other.files_to_remove);
        self.files_to_convert.extend(other.files_to_convert);
        self.files_to_transform.extend(other.files_to_transform);
        self.files_to_rename.extend(other.files_to_rename);
        self.files_to_add.extend(other.files_to_add);
        self.applied_fixes.extend(other.applied_fixes);
    }
}

fn apply_single_fix(
    files: &[(u64, String, Vec<u8>)],
    fix_rule: &WadFixRule,
    fix_id: &str,
    _hash_provider: &dyn HashProvider,
) -> Result<WadFixOutput> {
    let mut output = WadFixOutput::default();
    let mut files_affected = 0u32;

    // Build set of all file paths in WAD for fast lookup
    let file_paths: std::collections::HashSet<String> = files
        .iter()
        .map(|(_, path, _)| path.to_lowercase())
        .collect();

    // Whole-WAD actions: detection is implicit, we just emit instructions.
    match (&fix_rule.detect, &fix_rule.apply) {
        (WadDetectionRule::Always, WadTransformAction::AddFiles { assets }) => {
            for inj in assets {
                if inj.only_if_missing && file_paths.contains(&inj.path.to_lowercase()) {
                    continue;
                }
                output.files_to_add.push(FileAddition {
                    asset: inj.asset.clone(),
                    path: inj.path.clone(),
                    only_if_missing: inj.only_if_missing,
                });
                files_affected += 1;
            }
            if files_affected > 0 {
                output.applied_fixes.push(WadFixResult {
                    fix_id: fix_id.to_string(),
                    fix_name: fix_rule.name.clone(),
                    files_affected,
                });
            }
            return Ok(output);
        }
        (WadDetectionRule::Always, _) => {
            // Other actions paired with `Always` don't make sense at the
            // WAD level. Skip silently rather than failing the run.
            tracing::warn!(
                "WAD fix '{}' uses `Always` detection but action {:?} isn't whole-WAD aware",
                fix_id,
                std::mem::discriminant(&fix_rule.apply)
            );
            return Ok(output);
        }
        _ => {}
    }

    for (_, path, bytes) in files {
        // Check if this file matches the detection rule
        if detect::check_file(path, bytes, &fix_rule.detect)? {
            // Apply the transform action
            let action_result = transform::apply_action(path, bytes, &fix_rule.apply)?;

            match action_result {
                transform::ActionResult::RemoveFile => {
                    output.files_to_remove.push(path.clone());
                    files_affected += 1;
                }
                transform::ActionResult::ConvertFile {
                    from_ext,
                    to_ext,
                    converter,
                } => {
                    // Check if converted file already exists in WAD - if so, skip.
                    // Same-extension transforms (no rename) are routed via
                    // `TransformInPlace` instead — see below.
                    let converted_path =
                        path.replace(&format!(".{}", from_ext), &format!(".{}", to_ext));

                    if from_ext != to_ext && file_paths.contains(&converted_path.to_lowercase()) {
                        tracing::debug!(
                            "Skipping conversion (target already exists): {} → {}",
                            path,
                            converted_path
                        );
                    } else {
                        output.files_to_convert.push(FileConversion {
                            path: path.clone(),
                            from_ext,
                            to_ext,
                            converter,
                        });
                        files_affected += 1;
                    }
                }
                transform::ActionResult::TransformInPlace { converter } => {
                    output.files_to_transform.push(InPlaceTransform {
                        path: path.clone(),
                        converter,
                    });
                    files_affected += 1;
                }
                transform::ActionResult::RenameFile { new_path } => {
                    output.files_to_rename.push((path.clone(), new_path));
                    files_affected += 1;
                }
                transform::ActionResult::NoOp => {}
            }
        }
    }

    if files_affected > 0 {
        output.applied_fixes.push(WadFixResult {
            fix_id: fix_id.to_string(),
            fix_name: fix_rule.name.clone(),
            files_affected,
        });
    }

    Ok(output)
}
