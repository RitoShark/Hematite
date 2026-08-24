//! Detect-only mode: report what is wrong with a mod without changing anything.
//!
//! This is the half of the engine that answers "is this mod broken", as opposed to
//! silently rewriting it. It runs the *same* rules the fix pipeline runs, through the
//! same [`detect_issue`] dispatch, and turns each firing rule into a [`Diagnostic`]
//! instead of (or as well as) a transform. One rule set, two modes.
//!
//! ## What makes a rule reportable
//! A rule contributes a diagnostic only when it declares a `reason` (see the
//! `[reasons.*]` catalog in `fix_config.toml`). Rules without one are fix-only, which is
//! correct for structural normalisations that are not defects a player should be warned
//! about. Mis-tagging a structural rule as a defect is worse than leaving it untagged,
//! so the default is silence.
//!
//! ## Per-target reporting
//! Most rules are one detection producing one diagnostic. The migration rule is not: it
//! covers many properties whose failures differ in kind, so [`retype_file::detect_hits`]
//! finds *which* property is unmigrated and each target may name its own reason. That is
//! what lets one rule report a crash for an animation path and a warning for an icon.

pub mod asset_ratio;
pub mod loose_textures;
pub mod required_texture;
pub mod signature_vfx;
pub mod vfx_ratio;

use crate::context::FixContext;
use crate::detect::detect_issue;
use crate::transform::retype_file;
use hematite_types::config::{DetectionRule, FixConfig, FixRule};
use hematite_types::diagnostic::{CheckReport, Diagnostic, SkipReason};

/// Why a rule cannot run against this context, if it cannot.
///
/// A rule whose prerequisites are missing does not detect nothing: it detects *nothing
/// knowable*. Reporting that as a pass is the worst failure a checker has, because the
/// user reads a clean result and ships a mod that crashes. Every precondition that
/// silently disabled a rule now surfaces as a skip instead.
pub fn skip_reason(rule: &FixRule, ctx: &FixContext) -> Option<SkipReason> {
    match &rule.detect {
        // Needs the installed list of valid shaders to know which references are dead.
        DetectionRule::InvalidShaderReference { .. } if ctx.shader_validator.is_none() => {
            Some(SkipReason::NoShaderList)
        }
        // Both compare the mod against the base game; with no install there is nothing
        // to compare against.
        DetectionRule::ReplacedBinEntryDiff { .. } | DetectionRule::DeadEntryLink { .. }
            if ctx.game.is_none() =>
        {
            Some(SkipReason::NoGameDir)
        }
        // Needs the installed game's shader set. Absent means the links cannot be judged
        // at all: reporting them as fine would hide the crash this rule exists to catch.
        DetectionRule::DeadShaderLink if !crate::detect::shader_link::can_validate(ctx) => {
            Some(if ctx.game.is_none() {
                SkipReason::NoGameDir
            } else {
                SkipReason::NoShaderList
            })
        }
        _ if !ctx.hashes.is_loaded() && needs_hashes(&rule.detect) => {
            Some(SkipReason::NoHashDictionary)
        }
        _ => None,
    }
}

/// Whether the BIN under inspection belongs to a summoned form rather than a playable
/// champion.
///
/// A trap, an egg or a turret has its own character folder and its own skin BINs, so
/// structurally it is indistinguishable from a champion. Only the champion list
/// separates them.
///
/// ## Why this reads the BIN's contents, not its path
/// The obvious test is the file path, and it fails on exactly the mods that need it: a
/// repathed mod's BINs carry custom paths no dictionary can resolve, so the path is a
/// bare hash and names no character at all. The character is recoverable from the BIN
/// itself, whose linked dependency list points at `.../characters/<name>/<name>.bin`.
/// The path is kept only as a fallback for BINs with no linked list.
///
/// ## Unknown characters are treated as summoned forms
/// The champion list is complete and ships in the same config as the rules, so a
/// character absent from it is far more likely a summoned form than a champion: most
/// forms are not enumerated anywhere (`JhinTrap` appears in no list), while every
/// champion is. The cost is that a champion released before the config updates goes
/// unchecked for these rules, which self-heals on the next config refresh. The
/// alternative fails the other way and reports a defect on every unlisted form, which is
/// the bug this gate exists to remove.
pub(crate) fn is_subcharacter(ctx: &FixContext) -> bool {
    is_summoned_form(&ctx.tree.linked, &ctx.file_path, ctx.champions)
}

/// Testable core of [`is_subcharacter`].
fn is_summoned_form(
    linked: &[String],
    file_path: &str,
    champions: &hematite_types::champion::CharacterRelations,
) -> bool {
    match bin_character(linked, file_path) {
        Some(name) => !champions.is_champion(&name),
        None => false,
    }
}

/// Character a BIN belongs to, from its linked dependencies first and its path second.
fn bin_character(linked: &[String], file_path: &str) -> Option<String> {
    for dep in linked {
        if let Some(name) = crate::seeds::character_of(dep) {
            return Some(name);
        }
    }
    crate::seeds::character_of(file_path)
}

/// Whether a detection cannot work without a loaded hash dictionary.
fn needs_hashes(rule: &DetectionRule) -> bool {
    matches!(
        rule,
        DetectionRule::MissingOrWrongField { .. }
            | DetectionRule::FieldHashExists { .. }
            | DetectionRule::StringExtensionNotInWad { .. }
            | DetectionRule::EntryTypeExistsAny { .. }
            | DetectionRule::UnreferencedEntryOfType { .. }
            | DetectionRule::InvalidShaderReference { .. }
            | DetectionRule::VfxShapeNeedsFix { .. }
    )
}

/// Diagnostics for a rule that has ALREADY been detected as firing.
///
/// Split from detection so the fix pipeline, which has just evaluated the rule, does not
/// pay for a second tree walk. Migration rules are the exception and re-walk regardless,
/// because a whole-rule bool cannot say which property is at fault.
///
/// Returns empty when the rule declares no reason.
pub fn diagnose_fired_rule(
    fix_id: &str,
    rule: &FixRule,
    config: &FixConfig,
    ctx: &FixContext,
) -> Vec<Diagnostic> {
    let catalog = &config.reasons;

    // Some properties exist only on the playable champion, and its summoned forms
    // legitimately lack them. Reporting those as defects turns one subcharacter into
    // dozens of false findings.
    if rule.main_character_only && is_subcharacter(ctx) {
        return Vec::new();
    }

    // A BIN the mod never loads cannot crash anything inside it.
    if !ctx.scope.should_inspect(rule.loaded_bins_only) {
        return Vec::new();
    }

    if let DetectionRule::ClassFieldIsString { targets } = &rule.detect {
        let hits = retype_file::detect_hits(&ctx.tree, targets);
        let mut out = Vec::new();
        for target in targets {
            let Some(sample) = hits.get(&retype_file::target_key(target)) else {
                continue;
            };
            // Target reason wins, then the rule's own. Neither means this target is
            // fix-only, which lets one rule carry reported and silent targets together.
            let Some(reason) = target.reason.as_deref().or(rule.reason.as_deref()) else {
                continue;
            };
            out.push(
                Diagnostic::new(catalog, reason, fix_id)
                    .with_entry(ctx.file_path.clone())
                    .with_field(format!("{}.{}", target.class, target.field))
                    .with_detail(sample.clone()),
            );
        }
        return out;
    }

    if let DetectionRule::ReplacedBinEntryDiff { targets } = &rule.detect {
        let mut out = Vec::new();
        for hit in crate::detect::replaced_bin::detect_hits(ctx, targets) {
            let target = &targets[hit.target_index];
            let Some(reason) = target.reason.as_deref().or(rule.reason.as_deref()) else {
                continue;
            };
            out.push(
                Diagnostic::new(catalog, reason, fix_id)
                    .with_entry(ctx.file_path.clone())
                    .with_detail(hit.describe(target)),
            );
        }
        return out;
    }

    if let DetectionRule::DeadAssetReference {
        extensions,
        path_prefixes,
        downgrade_markers,
        downgrade_reason,
        latent_reason,
        suppress_never_loaded,
    } = &rule.detect
    {
        let dead = crate::detect::dead_asset::dead_refs(
            ctx,
            extensions,
            path_prefixes,
            *suppress_never_loaded,
        );
        if dead.is_empty() {
            return Vec::new();
        }

        // Split before summarising. A map mod's missing minion mesh and its missing
        // nexus mesh are different findings at different severities, so collapsing them
        // into one line would have to pick a severity and be wrong for the other half.
        let (downgraded, primary): (Vec<String>, Vec<String>) =
            if downgrade_reason.is_some() && !downgrade_markers.is_empty() {
                dead.into_iter().partition(|p| {
                    crate::detect::dead_asset::is_map_character(p, downgrade_markers)
                })
            } else {
                (Vec::new(), dead)
            };

        // One diagnostic per group rather than per path: a mod missing an animation set
        // is missing dozens at once, and a line each would drown every other result.
        let summarise = |paths: &[String]| {
            if paths.len() == 1 {
                paths[0].clone()
            } else {
                format!("{} missing, including {}", paths.len(), paths[0])
            }
        };

        // A reference in a BIN nothing loads is conditional, whatever it points at.
        let reached = crate::detect::dead_asset::is_reached_in_use(ctx);
        let primary_reason = if reached {
            rule.reason.as_deref()
        } else {
            latent_reason.as_deref().or(rule.reason.as_deref())
        };

        let mut out = Vec::new();
        if !primary.is_empty() {
            if let Some(reason) = primary_reason {
                out.push(
                    Diagnostic::new(catalog, reason, fix_id)
                        .with_entry(ctx.file_path.clone())
                        .with_detail(summarise(&primary)),
                );
            }
        }
        if !downgraded.is_empty() {
            if let Some(reason) = downgrade_reason.as_deref() {
                out.push(
                    Diagnostic::new(catalog, reason, fix_id)
                        .with_entry(ctx.file_path.clone())
                        .with_detail(summarise(&downgraded)),
                );
            }
        }
        return out;
    }

    if let DetectionRule::StaleCharacterRecord {
        entry_type,
        fields,
        critical_field,
        critical_reason,
    } = &rule.detect
    {
        let missing = crate::detect::stale_character::missing_fields(ctx, entry_type, fields);
        if missing.is_empty() {
            return Vec::new();
        }

        // Losing the ability binding is a different defect from losing a stat field, so
        // the critical one decides the verdict outright rather than being counted
        // alongside the rest.
        let critical_missing = critical_field
            .as_deref()
            .is_some_and(|c| missing.iter().any(|m| m.eq_ignore_ascii_case(c)));

        let (reason, detail) = if critical_missing {
            match critical_reason.as_deref() {
                Some(r) => (
                    r,
                    format!("missing {}", critical_field.as_deref().unwrap_or("?")),
                ),
                None => return Vec::new(),
            }
        } else {
            match rule.reason.as_deref() {
                Some(r) => {
                    let names: Vec<&str> = missing.iter().take(4).map(|s| s.as_str()).collect();
                    (r, format!("{} missing: {}", missing.len(), names.join(", ")))
                }
                None => return Vec::new(),
            }
        };

        return vec![Diagnostic::new(catalog, reason, fix_id)
            .with_entry(ctx.file_path.clone())
            .with_detail(detail)];
    }

    match rule.reason.as_deref() {
        Some(reason) => vec![Diagnostic::new(catalog, reason, fix_id).with_entry(ctx.file_path.clone())],
        None => Vec::new(),
    }
}

/// Run every enabled rule against one BIN tree in detect-only mode.
///
/// The tree is never mutated. Used by consumers that want only a report and are not
/// running the fix pipeline; the pipeline itself calls [`diagnose_fired_rule`] directly.
pub fn check_tree(config: &FixConfig, ctx: &FixContext) -> CheckReport {
    let mut report = CheckReport::default();

    for (id, rule) in &config.fixes {
        if !config.is_fix_enabled(id) {
            continue;
        }
        if let Some(why) = skip_reason(rule, ctx) {
            report.mark_skipped(id, why);
            continue;
        }
        if detect_issue(&rule.detect, ctx) {
            for d in diagnose_fired_rule(id, rule, config, ctx) {
                report.push(d);
            }
        }
        report.mark_ran(id);
    }

    report.attach_catalog(&config.reasons);
    report
}

/// Reason ids referenced by rules or migration targets that the catalog does not define.
///
/// A rule naming a missing reason would silently degrade to a generic warning at runtime.
/// Surfacing it at config-load time turns a quiet mis-tagging into a loud startup
/// failure, which is the only point at which anyone would notice.
pub fn unknown_reason_ids(config: &FixConfig) -> Vec<String> {
    let mut referenced: Vec<&str> = Vec::new();
    for rule in config.fixes.values() {
        if let Some(r) = rule.reason.as_deref() {
            referenced.push(r);
        }
        match &rule.detect {
            DetectionRule::ClassFieldIsString { targets } => {
                referenced.extend(targets.iter().filter_map(|t| t.reason.as_deref()));
            }
            DetectionRule::ReplacedBinEntryDiff { targets } => {
                referenced.extend(targets.iter().filter_map(|t| t.reason.as_deref()));
            }
            _ => {}
        }
    }
    config.reasons.unknown_ids(referenced.into_iter())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::config::ClassFieldTarget;
    use hematite_types::diagnostic::{ReasonCatalog, ReasonDef, Severity};
    use std::collections::HashMap;

    fn catalog() -> ReasonCatalog {
        let mut reasons = HashMap::new();
        reasons.insert(
            "unmigrated_animation_path".to_string(),
            ReasonDef {
                severity: Severity::Crash,
                title: "Animation path uses the old format".into(),
                explain: "Stored as text, unreadable by this patch.".into(),
                remedy: Some("Run Repair.".into()),
                author: None,
            },
        );
        reasons.insert(
            "unmigrated_hud_asset".to_string(),
            ReasonDef {
                severity: Severity::Warning,
                title: "Interface asset uses the old format".into(),
                explain: "Stored as text, does not resolve.".into(),
                remedy: Some("Run Repair.".into()),
                author: None,
            },
        );
        ReasonCatalog { reasons }
    }

    fn target(class: &str, field: &str, reason: Option<&str>) -> ClassFieldTarget {
        ClassFieldTarget {
            class: class.into(),
            field: field.into(),
            reason: reason.map(str::to_owned),
        }
    }

    /// The requirement that drove per-target reasons: one rule, two severities.
    #[test]
    fn per_target_reasons_resolve_to_different_severities() {
        let c = catalog();
        assert_eq!(
            Diagnostic::new(&c, "unmigrated_animation_path", "file_ref_migration").severity,
            Severity::Crash
        );
        assert_eq!(
            Diagnostic::new(&c, "unmigrated_hud_asset", "file_ref_migration").severity,
            Severity::Warning
        );
    }

    fn champions() -> hematite_types::champion::CharacterRelations {
        let mut r = hematite_types::champion::CharacterRelations::default();
        r.champions = ["jhin", "gragas"].iter().map(|s| s.to_string()).collect();
        r
    }

    /// A playable champion is checked normally.
    #[test]
    fn champion_bin_is_not_a_summoned_form() {
        assert!(!is_summoned_form(
            &["DATA/Characters/Jhin/Jhin.bin".into()],
            "data/characters/jhin/skins/skin0.bin",
            &champions()
        ));
    }

    /// The regression this gate exists for: JhinTrap carries a
    /// `SkinCharacterDataProperties` but no health bar style, on purpose. Reporting it
    /// produced 37 false findings on one mod.
    #[test]
    fn summoned_form_is_recognised() {
        assert!(is_summoned_form(
            &["DATA/Characters/JhinTrap/JhinTrap.bin".into()],
            "data/characters/jhintrap/skins/skin0.bin",
            &champions()
        ));
    }

    /// The reason the gate reads the BIN and not the path: a repathed mod's BINs have
    /// custom paths no dictionary resolves, so `file_path` is a bare chunk hash that
    /// names no character. The linked list still does.
    #[test]
    fn linked_list_identifies_the_character_when_the_path_cannot() {
        assert!(is_summoned_form(
            &["DATA/Characters/JhinTrap/JhinTrap.bin".into()],
            "00df3e4432caeaa8",
            &champions()
        ));
    }

    /// A character absent from the list is treated as a summoned form, because most
    /// forms are listed nowhere while every champion is listed. The tradeoff is that a
    /// champion released before the config updates goes unchecked for gated rules.
    #[test]
    fn unlisted_character_is_treated_as_a_summoned_form() {
        assert!(is_summoned_form(
            &["DATA/Characters/SomeNewChamp/SomeNewChamp.bin".into()],
            "data/characters/somenewchamp/skins/skin0.bin",
            &champions()
        ));
    }

    /// A BIN naming no character at all must not be suppressed.
    #[test]
    fn unidentifiable_bin_is_not_suppressed() {
        assert!(!is_summoned_form(&[], "00df3e4432caeaa8", &champions()));
    }

    #[test]
    fn target_key_is_stable_across_name_and_hex_forms() {
        let by_name = target("AnimationResourceData", "mAnimationFilePath", None);
        let key = retype_file::target_key(&by_name);
        let by_hex = target(&format!("0x{:08x}", key.0), &format!("0x{:08x}", key.1), None);
        assert_eq!(retype_file::target_key(&by_hex), key);
    }
}
