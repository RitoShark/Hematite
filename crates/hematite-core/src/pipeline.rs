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
use hematite_types::config::{DetectionRule, FixConfig, FixPhase, TransformAction};
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

        // A property that exists only on the playable champion must not be WRITTEN onto its
        // summoned forms either. This gate was on the reporting path alone, so a trap or an
        // egg was quietly given a champion's health-bar style and then not even mentioned:
        // the fix applied, the finding did not. Detecting and repairing have to agree about
        // what a rule is allowed to touch, or the disagreement is invisible by construction.
        if fix_rule.main_character_only && crate::check::is_subcharacter(ctx) {
            tracing::debug!(
                "Skipping '{}' on {}: a summoned form, not the champion",
                fix_id,
                ctx.file_path
            );
            continue;
        }

        // Without a game install, dead-link detection can't consult the game
        // closure — every game-defined entry looks "dead" and the pull can
        // never apply. Skip instead of reporting false errors.
        if ctx.game.is_none()
            && matches!(
                fix_rule.apply,
                hematite_types::config::TransformAction::PullEntriesFromGame { .. }
            )
        {
            tracing::debug!("Skipping '{}': no game install available", fix_id);
            continue;
        }

        // A rule whose prerequisites are missing detects nothing *knowable*, which is not
        // the same as detecting nothing. Recording the skip keeps "we checked and it is
        // fine" distinguishable from "we could not check".
        if let Some(why) = crate::check::skip_reason(fix_rule, ctx) {
            tracing::debug!("skipping '{}': {:?}", fix_id, why);
            // Only checks that could have REPORTED something count as gaps. A fix-only
            // rule that cannot run has repaired nothing, which is worth a log, but
            // listing it under "could not check" implies the mod went unverified in some
            // way it did not.
            if fix_rule.reason.is_some() {
                result.report.mark_skipped(fix_id.clone(), why);
            }
            continue;
        }

        let rule_started = std::time::Instant::now();
        let detected = detect_issue(&fix_rule.detect, ctx);
        crate::timing::record(&format!("rule:{fix_id}"), rule_started.elapsed());

        if detected {
            // Record the player-facing finding BEFORE the transform runs. Afterwards the
            // defect is gone from the tree and the evidence with it, so a repair run
            // could no longer say what it repaired.
            for diagnostic in crate::check::diagnose_fired_rule(fix_id, fix_rule, config, ctx) {
                result.report.push(diagnostic);
            }

            if dry_run {
                result.fixes_applied += 1;
                result.applied_fixes.push(AppliedFix {
                    fix_id: fix_id.clone(),
                    fix_name: fix_rule.name.clone(),
                    changes_count: 0,
                    file_path: ctx.file_path.clone(),
                });
            } else if matches!(fix_rule.apply, TransformAction::ReportOnly) {
                // Detected and reported. There is no repair to attempt, so this is
                // neither an applied fix nor a failure. Counting it either way would
                // misreport: as a fix it claims a repair that never happened, as a
                // failure it buries a correct finding in the error list.
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
    result.report.attach_catalog(&config.reasons);
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
        | DetectionRule::ClassFieldIsString { .. }
        // Compares whole entry sets across two trees rather than filtering objects of
        // one type, so there is no single entry type to extract.
        | DetectionRule::ReplacedBinEntryDiff { .. }
        | DetectionRule::DeadShaderLink
        | DetectionRule::DeadAssetReference { .. }
        | DetectionRule::StaleCharacterRecord { .. } => None,
    }
}

#[cfg(test)]
mod main_character_gate_tests {
    //! Detecting and repairing must agree about what a rule may touch.
    //!
    //! `main_character_only` gated the report and not the write, so a summoned form was
    //! given a champion's health-bar style and then not listed as having been changed. A
    //! fix that applies where it does not report is invisible twice over: the mod is
    //! modified and nothing says so.

    use hematite_types::champion::{CharacterRelations, ChampionList};

    #[test]
    fn a_summoned_form_is_not_the_champion() {
        let list = ChampionList {
            version: "test".into(),
            champions: vec!["jhin".into()],
            subchamps: [("jhin".to_string(), vec!["jhintrap".to_string()])]
                .into_iter()
                .collect(),
            healthbar_values: Default::default(),
            blacklist: Vec::new(),
            special_blacklists: Default::default(),
        };
        let champions = CharacterRelations::from_champion_list(&list);

        assert!(champions.is_champion("jhin"));
        assert!(
            !champions.is_champion("jhintrap"),
            "a trap is not a playable champion, which is what the gate turns on"
        );
    }
}
