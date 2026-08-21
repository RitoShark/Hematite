//! Fix orchestration: detect → transform → result.
//!
//! This is the main entry point for the fix engine. Given a `FixContext` and
//! a set of selected fix rules, it:
//! 1. Runs detection for each rule
//! 2. If detected, applies the corresponding transform
//! 3. Collects results (applied fixes, failures, change counts)
//!
//! ## Flow
//! ```text
//! for each fix_id in selected_fixes:
//!     rule = config.fixes[fix_id]
//!     if detect::detect_issue(&rule.detect, &ctx):
//!         changes = transform::apply_transform(&rule.apply, &mut ctx)
//!         track result
//! ```

use crate::context::FixContext;
use crate::detect::detect_issue;
use crate::transform::apply_transform;
use hematite_types::config::{DetectionRule, FixConfig, FixPhase};
use hematite_types::result::{AppliedFix, ProcessResult};

/// Run the standard-phase fixes against a BIN tree.
///
/// Returns the modified BinTree (inside the context) and a result summary.
/// Rules marked `"phase": "post_repath"` are skipped here — run them via
/// [`apply_fixes_in_phase`] after the repath stage.
pub fn apply_fixes(
    ctx: &mut FixContext<'_>,
    config: &FixConfig,
    selected_fix_ids: &[String],
    dry_run: bool,
) -> ProcessResult {
    apply_fixes_in_phase(ctx, config, selected_fix_ids, dry_run, FixPhase::Standard)
}

/// Run the selected fixes belonging to one pipeline phase.
pub fn apply_fixes_in_phase(
    ctx: &mut FixContext<'_>,
    config: &FixConfig,
    selected_fix_ids: &[String],
    dry_run: bool,
    phase: FixPhase,
) -> ProcessResult {
    let mut result = ProcessResult {
        files_processed: 1,
        ..Default::default()
    };

    for fix_id in selected_fix_ids {
        let Some(fix_rule) = config.fixes.get(fix_id) else {
            // WAD-level fix IDs (e.g. bnk_remover, anm_remover) are handled
            // separately by the WAD pipeline — skip them silently here.
            if phase == FixPhase::Standard && !config.wad_fixes.contains_key(fix_id) {
                result
                    .errors
                    .push(format!("Fix rule not found: {}", fix_id));
            }
            continue;
        };

        if !config.is_fix_enabled(fix_id) || fix_rule.phase != phase {
            continue;
        }

        let detected = detect_issue(&fix_rule.detect, ctx);

        if detected {
            if dry_run {
                result.fixes_applied += 1;
                result.applied_fixes.push(AppliedFix {
                    fix_id: fix_id.clone(),
                    fix_name: fix_rule.name.clone(),
                    changes_count: 0,
                    file_path: ctx.file_path.clone(),
                });
            } else {
                let entry_type = extract_entry_type(&fix_rule.detect);
                let changes = apply_transform(&fix_rule.apply, ctx, entry_type);

                if changes > 0 {
                    result.fixes_applied += 1;
                    result.applied_fixes.push(AppliedFix {
                        fix_id: fix_id.clone(),
                        fix_name: fix_rule.name.clone(),
                        changes_count: changes,
                        file_path: ctx.file_path.clone(),
                    });
                } else {
                    result.fixes_failed += 1;
                    result
                        .errors
                        .push(format!("Fix '{}' detected but no changes applied", fix_id));
                }
            }
        }
    }

    result.files_removed = ctx.files_to_remove.len() as u32;
    result
}

/// Extract entry_type from a detection rule (if it has one).
///
/// Some detection rules target specific object types and include an entry_type field.
/// Object-specific transforms (EnsureField, VfxShapeFix) need this to filter objects.
fn extract_entry_type(rule: &DetectionRule) -> Option<&str> {
    match rule {
        DetectionRule::MissingOrWrongField { entry_type, .. }
        | DetectionRule::FieldHashExists { entry_type, .. }
        | DetectionRule::StringExtensionNotInWad { entry_type, .. }
        | DetectionRule::VfxShapeNeedsFix { entry_type, .. } => Some(entry_type.as_str()),
        DetectionRule::InvalidShaderReference {
            shader_def_type, ..
        } => Some(shader_def_type.as_str()),
        DetectionRule::UnreferencedEntryOfType {
            main_entry_type, ..
        }
        | DetectionRule::DeadEntryLink {
            main_entry_type, ..
        } => Some(main_entry_type.as_str()),
        DetectionRule::RecursiveStringExtensionNotInWad { .. }
        | DetectionRule::EntryTypeExistsAny { .. }
        | DetectionRule::BnkVersionNotIn { .. }
        | DetectionRule::ClassFieldIsString { .. } => None,
    }
}
