//! Processing and fix result types.
//!
//! These types track what happened during a fix session — how many files
//! were processed, which fixes were applied, and any errors encountered.

use std::time::Duration;

/// Aggregate result of processing one or more files.
#[derive(Debug, Clone, Default)]
pub struct ProcessResult {
    pub files_processed: u32,
    pub fixes_applied: u32,
    pub fixes_failed: u32,
    pub files_removed: u32,
    pub errors: Vec<String>,
    pub applied_fixes: Vec<AppliedFix>,
    pub duration: Option<Duration>,
    /// Check-mode info (populated when --check is used).
    pub check_info: Option<CheckInfo>,
    /// Player-facing findings: what is wrong, how badly, and what to do about it.
    ///
    /// Populated on every run, not only in check mode. A fix run already knows exactly
    /// which defects it found, so discarding that would force a second pass to ask the
    /// same question.
    pub report: crate::diagnostic::CheckReport,
}

/// Information gathered in check mode (detection-only).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CheckInfo {
    pub champion: Option<String>,
    pub skin_number: Option<u32>,
    pub is_binless: bool,
    pub detected_issues: Vec<String>,
}

impl ProcessResult {
    /// Merge another result into this one.
    pub fn merge(&mut self, other: ProcessResult) {
        self.files_processed += other.files_processed;
        self.fixes_applied += other.fixes_applied;
        self.fixes_failed += other.fixes_failed;
        self.files_removed += other.files_removed;
        self.errors.extend(other.errors);
        self.applied_fixes.extend(other.applied_fixes);
        self.report.merge(other.report);
        // Merge check info (keep first non-None)
        if self.check_info.is_none() {
            self.check_info = other.check_info;
        }
    }
}

/// Record of a single fix that was successfully applied.
#[derive(Debug, Clone)]
pub struct AppliedFix {
    /// Fix rule ID from config (e.g. "healthbar_fix").
    pub fix_id: String,
    /// Human-readable name (e.g. "Missing HP Bar").
    pub fix_name: String,
    /// Number of individual changes made.
    pub changes_count: u32,
    /// File path where the fix was applied.
    pub file_path: String,
}
