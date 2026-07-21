//! Library-neutral progress reporting.
//!
//! The folder pipeline drives a [`ProgressSink`] instead of the CLI's
//! concrete `UiReporter`. The CLI supplies an adapter wrapping its progress
//! bar; Flint supplies one emitting Tauri events; tests use [`NoopSink`].
//!
//! The three core methods ([`ProgressSink::stage`],
//! [`ProgressSink::fix_applied`], [`ProgressSink::note`]) mirror the
//! `UiReporter` calls the lifted pipeline makes. The two determinate-bar
//! methods ([`ProgressSink::set_length`], [`ProgressSink::tick`]) carry
//! default no-op bodies so an embedder that doesn't render a bar needn't
//! implement them.

/// Sink for user-visible progress emitted by the fix pipeline.
pub trait ProgressSink: Send + Sync {
    /// Set the label for the next phase (e.g. "Extracting…", "Rebuilding WAD…").
    fn stage(&self, label: &str);

    /// Report a fix having been applied, with an optional change count.
    fn fix_applied(&self, name: &str, count: Option<u32>);

    /// Emit a non-fatal note the user should still see.
    fn note(&self, message: &str);

    /// Switch a determinate progress bar to the given expected step count.
    /// Defaulted no-op — embedders without a bar can ignore it.
    fn set_length(&self, _total: u64) {}

    /// Advance a determinate progress bar by one step. Defaulted no-op.
    fn tick(&self) {}
}

/// A [`ProgressSink`] that discards everything — handy for tests and
/// non-interactive embedders.
pub struct NoopSink;

impl ProgressSink for NoopSink {
    fn stage(&self, _label: &str) {}
    fn fix_applied(&self, _name: &str, _count: Option<u32>) {}
    fn note(&self, _message: &str) {}
}
