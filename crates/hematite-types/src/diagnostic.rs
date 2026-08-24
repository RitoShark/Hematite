//! Diagnostic types: what the engine *reports*, as distinct from what it fixes.
//!
//! Hematite historically only rewrote mods. `detect_issue` returned a bool used to gate
//! a transform, and `apply_transform` returned a change count. Nothing could answer the
//! question a launcher actually needs to ask: *is this mod broken, and how badly?*
//!
//! This module is that answer. A [`Diagnostic`] is one finding about one mod. A
//! [`CheckReport`] is every finding plus, importantly, every check that could **not**
//! run. See [`CheckReport::skipped`] for why that second half matters.
//!
//! ## Reasons are data, not Rust
//! The catalog of *what can be wrong* lives in `config/reasons.toml`, not in an enum
//! here. Adding a crash class, retitling one, changing its severity or its remedy text
//! is a config edit with no rebuild. That is the point of Hematite, and baking the
//! catalog into Rust would defeat it.
//!
//! What stays in Rust is only [`Severity`], because it is a fixed four-value semantic
//! scale that the engine itself branches on, and because it orders.
//!
//! ## Severity is resolved, not fixed
//! A reason carries a *default* severity in the catalog, but the detecting check can
//! override it. This is normal rather than exceptional: a dead animation link is a crash
//! when the bin is reachable from a loaded skin and merely latent when it is not, and a
//! migration rule is a crash on an animation path but a warning on a HUD asset.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// How badly a diagnostic affects the player.
///
/// Ordered worst-first, so `Iterator::min` finds the most severe finding and
/// [`CheckReport::worst`] falls out of comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The game crashes: on load, on champion select, or when the asset is used.
    Crash,
    /// No crash, but the champion cannot be played normally.
    Unplayable,
    /// Playable but visibly degraded.
    Warning,
    /// Informational. Not a defect.
    Info,
}

impl Severity {
    /// Whether this counts as a defect worth surfacing.
    pub fn is_defect(self) -> bool {
        !matches!(self, Severity::Info)
    }

    /// Stable lowercase label for UI and JSON.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Crash => "crash",
            Severity::Unplayable => "unplayable",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

/// One entry in the reason catalog, loaded from `config/reasons.toml`.
///
/// ```toml
/// [reasons.dead_gear_link]
/// severity = "crash"
/// title    = "Dead gear link"
/// explain  = "A gear upgrade link points at an entry that does not exist."
/// remedy   = "Run Deep Repair to pull the missing entries."
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasonDef {
    /// Severity applied when the detecting check does not override it.
    pub severity: Severity,
    /// Short human-readable title.
    pub title: String,
    /// One sentence describing what is wrong, for a tooltip.
    pub explain: String,
    /// What the user can do. Absent when there is no in-launcher remedy, which is
    /// itself meaningful: it tells the UI not to offer a repair button.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remedy: Option<String>,
}

impl ReasonDef {
    /// Whether the launcher can offer to repair this.
    pub fn fixable(&self) -> bool {
        self.remedy.is_some()
    }
}

/// The full set of known reasons, keyed by stable id (e.g. `"dead_gear_link"`).
///
/// Serialises as the bare map so it can sit under `[reasons.*]` in the same
/// `fix_config.toml` that carries the rules. One config file, per Hematite's design.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReasonCatalog {
    pub reasons: HashMap<String, ReasonDef>,
}

impl ReasonCatalog {
    pub fn get(&self, id: &str) -> Option<&ReasonDef> {
        self.reasons.get(id)
    }

    /// Default severity for a reason id.
    ///
    /// Falls back to [`Severity::Warning`] for an unknown id rather than panicking: an
    /// unrecognised reason from a newer config should degrade to a visible warning, not
    /// take the launcher down or vanish silently.
    pub fn severity_of(&self, id: &str) -> Severity {
        self.get(id).map(|r| r.severity).unwrap_or(Severity::Warning)
    }

    /// Ids referenced by a set of diagnostics but absent from the catalog. Used to fail
    /// config validation loudly at load rather than at render.
    pub fn unknown_ids<'a>(&self, ids: impl Iterator<Item = &'a str>) -> Vec<String> {
        let mut missing: Vec<String> = ids
            .filter(|id| !self.reasons.contains_key(*id))
            .map(str::to_owned)
            .collect();
        missing.sort();
        missing.dedup();
        missing
    }
}

/// One finding about one mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Catalog id, e.g. `"dead_gear_link"`.
    pub reason: String,
    /// Resolved severity. May differ from the catalog default when the check or a config
    /// target overrode it.
    pub severity: Severity,
    /// Which rule or check produced this, for provenance when a finding is disputed.
    pub rule_id: String,
    /// Entry path or key the finding sits on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// Field name the finding sits on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Names the specific asset, so the UI can say *which* animation is missing rather
    /// than only that one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Diagnostic {
    /// A finding whose severity comes from the catalog.
    pub fn new(catalog: &ReasonCatalog, reason: impl Into<String>, rule_id: impl Into<String>) -> Self {
        let reason = reason.into();
        let severity = catalog.severity_of(&reason);
        Self {
            reason,
            severity,
            rule_id: rule_id.into(),
            entry: None,
            field: None,
            detail: None,
        }
    }

    /// A finding with an explicit severity, for reachability-gated and config-targeted
    /// findings that do not use the catalog default.
    pub fn with_resolved_severity(
        reason: impl Into<String>,
        severity: Severity,
        rule_id: impl Into<String>,
    ) -> Self {
        Self {
            reason: reason.into(),
            severity,
            rule_id: rule_id.into(),
            entry: None,
            field: None,
            detail: None,
        }
    }

    pub fn with_entry(mut self, entry: impl Into<String>) -> Self {
        self.entry = Some(entry.into());
        self
    }

    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Why a check did not run.
///
/// A skipped check is **not** a pass, and conflating the two is the failure mode this
/// type exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum SkipReason {
    /// No hash dictionary loaded, so hashes could not be resolved to paths.
    NoHashDictionary,
    /// No game install available to compare against.
    NoGameDir,
    /// The list of valid shaders is not installed, so shader references cannot be
    /// validated. Distinct from [`SkipReason::NoHashDictionary`] because the remedy is
    /// different: this one needs a specific data file, not the hash database.
    NoShaderList,
    /// The mod contains nothing this check applies to.
    NotApplicable,
    /// The check errored. Carries the message.
    Failed(String),
}

/// Everything one check run produced.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CheckReport {
    pub diagnostics: Vec<Diagnostic>,
    /// Ids of checks that ran to completion.
    pub ran: Vec<String>,
    /// Checks that did not run, and why. Rendering a report without this makes a skipped
    /// check indistinguishable from a clean pass.
    pub skipped: Vec<(String, SkipReason)>,
    /// The catalog entries for reasons actually present, so a consumer can render titles
    /// and remedies without shipping its own copy of the catalog. Populated by
    /// [`CheckReport::attach_catalog`].
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub reasons: HashMap<String, ReasonDef>,
}

impl CheckReport {
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub fn mark_ran(&mut self, id: impl Into<String>) {
        self.ran.push(id.into());
    }

    /// Record that a check could not run.
    ///
    /// Deduplicated: a rule is evaluated once per BIN, so a mod with 300 BINs would
    /// otherwise report the same missing prerequisite 300 times and bury everything else.
    /// The fact is "this check could not run", which is true once.
    pub fn mark_skipped(&mut self, id: impl Into<String>, why: SkipReason) {
        let id = id.into();
        if self.skipped.iter().any(|(i, w)| *i == id && *w == why) {
            return;
        }
        self.skipped.push((id, why));
    }

    /// Collapse findings that say the same thing about the same defect.
    ///
    /// Rules run per BIN, and a mod ships the same BIN many times over: one fixture
    /// carries 37 clones of a skin, each naming the identical two missing clips. That is
    /// one defect, not 37, and reporting it 37 times pushes everything else off screen.
    /// Two findings are the same when their reason, field and detail match; the entry
    /// path differs per clone and is deliberately not part of the identity.
    pub fn dedupe(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.diagnostics.retain(|d| {
            seen.insert((
                d.reason.clone(),
                d.severity,
                d.field.clone(),
                d.detail.clone(),
            ))
        });
    }

    /// Copy in just the catalog entries this report references, so the JSON is
    /// self-describing for Celestial, Quartz and Flint without them tracking the config.
    pub fn attach_catalog(&mut self, catalog: &ReasonCatalog) {
        for d in &self.diagnostics {
            if let Some(def) = catalog.get(&d.reason) {
                self.reasons.insert(d.reason.clone(), def.clone());
            }
        }
    }

    /// The most severe finding, or `None` when nothing was found.
    ///
    /// Callers must not read `None` as "the mod is fine" without also checking
    /// [`CheckReport::skipped`].
    pub fn worst(&self) -> Option<Severity> {
        self.diagnostics.iter().map(|d| d.severity).min()
    }

    /// Whether anything at or above the given severity was found.
    pub fn has_at_least(&self, severity: Severity) -> bool {
        self.diagnostics.iter().any(|d| d.severity <= severity)
    }

    /// Whether the mod crashes.
    pub fn crashes(&self) -> bool {
        self.has_at_least(Severity::Crash)
    }

    /// Findings at exactly one severity.
    pub fn at(&self, severity: Severity) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(move |d| d.severity == severity)
    }

    /// Whether any check failed to run.
    pub fn incomplete(&self) -> bool {
        !self.skipped.is_empty()
    }

    pub fn merge(&mut self, other: CheckReport) {
        self.diagnostics.extend(other.diagnostics);
        self.ran.extend(other.ran);
        // Through mark_skipped so per-BIN repeats collapse across merges too, which is
        // where most of them come from: one result per BIN, all merged into one report.
        for (id, why) in other.skipped {
            self.mark_skipped(id, why);
        }
        self.reasons.extend(other.reasons);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> ReasonCatalog {
        let mut reasons = HashMap::new();
        reasons.insert(
            "dead_gear_link".to_string(),
            ReasonDef {
                severity: Severity::Crash,
                title: "Dead gear link".into(),
                explain: "Points at an entry that does not exist.".into(),
                remedy: Some("Run Deep Repair.".into()),
            },
        );
        reasons.insert(
            "bugged_hp_bar".to_string(),
            ReasonDef {
                severity: Severity::Warning,
                title: "Bugged health bar".into(),
                explain: "Renders in the wrong style.".into(),
                remedy: Some("Run Repair.".into()),
            },
        );
        reasons.insert(
            "mapgeo_outdated".to_string(),
            ReasonDef {
                severity: Severity::Crash,
                title: "Outdated map format".into(),
                explain: "Older than this patch understands.".into(),
                remedy: None,
            },
        );
        ReasonCatalog { reasons }
    }

    #[test]
    fn severity_orders_worst_first() {
        assert!(Severity::Crash < Severity::Unplayable);
        assert!(Severity::Unplayable < Severity::Warning);
        assert!(Severity::Warning < Severity::Info);
    }

    #[test]
    fn worst_picks_the_crash() {
        let c = catalog();
        let mut r = CheckReport::default();
        r.push(Diagnostic::new(&c, "bugged_hp_bar", "hp"));
        r.push(Diagnostic::new(&c, "dead_gear_link", "gear"));
        assert_eq!(r.worst(), Some(Severity::Crash));
        assert!(r.crashes());
    }

    #[test]
    fn clean_report_has_no_worst() {
        let r = CheckReport::default();
        assert_eq!(r.worst(), None);
        assert!(!r.crashes());
        assert!(!r.incomplete());
    }

    /// Per-target severity is the whole point: one migration rule must yield a crash on
    /// an animation path and a warning on a HUD asset.
    #[test]
    fn resolved_severity_overrides_the_catalog_default() {
        let c = catalog();
        let default = Diagnostic::new(&c, "dead_gear_link", "gear");
        assert_eq!(default.severity, Severity::Crash);

        let gated = Diagnostic::with_resolved_severity(
            "dead_gear_link",
            Severity::Warning,
            "gear",
        );
        assert_eq!(gated.severity, Severity::Warning);
    }

    /// An unknown id must degrade to a visible warning, never vanish and never panic.
    #[test]
    fn unknown_reason_degrades_to_warning() {
        let c = catalog();
        let d = Diagnostic::new(&c, "reason_from_a_newer_config", "future");
        assert_eq!(d.severity, Severity::Warning);
    }

    #[test]
    fn unknown_ids_are_reported_for_config_validation() {
        let c = catalog();
        let missing = c.unknown_ids(["dead_gear_link", "nope", "also_nope", "nope"].into_iter());
        assert_eq!(missing, vec!["also_nope", "nope"]);
    }

    #[test]
    fn attach_catalog_carries_only_referenced_reasons() {
        let c = catalog();
        let mut r = CheckReport::default();
        r.push(Diagnostic::new(&c, "bugged_hp_bar", "hp"));
        r.attach_catalog(&c);
        assert_eq!(r.reasons.len(), 1);
        assert!(r.reasons.contains_key("bugged_hp_bar"));
    }

    #[test]
    fn absent_remedy_means_not_fixable() {
        let c = catalog();
        assert!(!c.get("mapgeo_outdated").unwrap().fixable());
        assert!(c.get("bugged_hp_bar").unwrap().fixable());
    }

    #[test]
    fn skipped_is_tracked_separately_from_clean() {
        let mut r = CheckReport::default();
        r.mark_skipped("cac", SkipReason::NoHashDictionary);
        assert_eq!(r.worst(), None);
        assert!(r.incomplete());
    }

    /// Rules run once per BIN, so a mod with hundreds of BINs must not repeat the same
    /// missing prerequisite hundreds of times.
    #[test]
    fn repeated_skips_collapse() {
        let mut r = CheckReport::default();
        for _ in 0..50 {
            r.mark_skipped("shader", SkipReason::NoShaderList);
        }
        r.mark_skipped("gear", SkipReason::NoGameDir);
        assert_eq!(r.skipped.len(), 2);
    }

    #[test]
    fn merging_also_collapses_skips() {
        let mut a = CheckReport::default();
        a.mark_skipped("shader", SkipReason::NoShaderList);
        let mut b = CheckReport::default();
        b.mark_skipped("shader", SkipReason::NoShaderList);
        a.merge(b);
        assert_eq!(a.skipped.len(), 1);
    }
}
