//! Live progress UI for user-facing fix runs.
//!
//! When verbosity is `Normal`, the CLI hides all the per-step tracing
//! noise (parse this BIN, extract that chunk, look up this hash, ...)
//! and instead shows a live progress bar plus a clean stream of "✓ Fix
//! Name" ticks above it. Verbose / Trace verbosity skip the bar
//! entirely so the developer view stays untouched; quiet / json modes
//! skip it too so machine consumers see nothing extra.
//!
//! ## API shape
//! [`UiReporter`] is an explicit, cloneable handle that the processing
//! layer drives by calling [`UiReporter::stage`] (sets the bar's label
//! for the next phase), [`UiReporter::tick`] (advances one step within
//! the current phase), [`UiReporter::fix_applied`] (prints a green
//! check + fix name above the bar) and [`UiReporter::finish`] (clears
//! the bar at the end of a session).
//!
//! Calls are cheap no-ops when the reporter is in [`Mode::Silent`], so
//! callers don't have to branch on verbosity.

use crate::args::Verbosity;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

/// Whether to render the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Render the bar + per-fix ticks. Used for human-facing Normal runs.
    Live,
    /// Don't render anything. Used for quiet / json / verbose / trace
    /// — those flows surface info through other channels.
    Silent,
}

impl Mode {
    /// Pick a mode from `--verbosity` + `--json`. The bar is live only
    /// when the user is sitting at a terminal looking at "normal"
    /// human output.
    pub fn from_args(verbosity: &Verbosity, json: bool) -> Self {
        if json {
            return Mode::Silent;
        }
        match verbosity {
            Verbosity::Normal => Mode::Live,
            Verbosity::Quiet | Verbosity::Verbose | Verbosity::Trace => Mode::Silent,
        }
    }
}

/// Lightweight handle wrapped around an optional [`ProgressBar`].
/// Cloneable so it can be threaded through nested helpers without
/// lifetime gymnastics.
#[derive(Clone)]
pub struct UiReporter {
    bar: Option<ProgressBar>,
}

impl UiReporter {
    /// Build a reporter according to `mode`. In `Live` mode the bar
    /// starts as a spinner with no fixed total — it ticks on a 100ms
    /// timer so the user sees movement even when stages don't actively
    /// step. Switching to a determinate bar happens lazily the first
    /// time [`Self::set_length`] is called.
    pub fn new(mode: Mode) -> Self {
        let bar = match mode {
            Mode::Silent => None,
            Mode::Live => {
                let pb = ProgressBar::new_spinner();
                pb.set_style(
                    ProgressStyle::with_template("  {spinner:.bright_red} {msg}")
                        .expect("BUG: hard-coded progress style is invalid")
                        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
                );
                pb.enable_steady_tick(Duration::from_millis(100));
                Some(pb)
            }
        };
        Self { bar }
    }

    /// Silent reporter — handy for tests / non-interactive callers.
    pub fn silent() -> Self {
        Self { bar: None }
    }

    /// Switch the bar from spinner-style to a determinate bar with the
    /// given expected step count. Safe to call multiple times — each
    /// call resets the bar to the new total.
    pub fn set_length(&self, total: u64) {
        let Some(bar) = &self.bar else { return };
        bar.set_style(
            ProgressStyle::with_template(
                "  {spinner:.bright_red} [{bar:30.bright_red/bright_black}] {pos}/{len} {msg}",
            )
            .expect("BUG: hard-coded progress style is invalid")
            .progress_chars("█▉▊▋▌▍▎  "),
        );
        bar.set_length(total);
        bar.set_position(0);
    }

    /// Update the message shown next to the bar (e.g. "Extracting WAD",
    /// "Applying fixes"). Resets position to 0 if a length is set.
    pub fn stage(&self, label: &str) {
        if let Some(bar) = &self.bar {
            bar.set_message(label.to_string());
        }
    }

    /// Advance the determinate bar by one step. No-op for spinner-style
    /// bars and for silent reporters.
    pub fn tick(&self) {
        if let Some(bar) = &self.bar {
            bar.inc(1);
        }
    }

    /// Print a green "✓ Fix Name" line above the bar without disrupting
    /// the bar itself. The optional `count` is rendered as a dimmed
    /// suffix (e.g. "✓ Missing HP Bar (1 change)").
    pub fn fix_applied(&self, name: &str, count: Option<u32>) {
        let line = match count {
            Some(n) if n > 0 => format!(
                "  {} {} {}",
                "✓".bright_green().bold(),
                name.bright_white(),
                format!("({} change{})", n, if n == 1 { "" } else { "s" }).bright_black()
            ),
            _ => format!("  {} {}", "✓".bright_green().bold(), name.bright_white()),
        };
        match &self.bar {
            Some(bar) => bar.println(line),
            None => eprintln!("{line}"),
        }
    }

    /// Print a yellow-tinted "! note" above the bar — used for
    /// non-fatal warnings the user should still see.
    pub fn note(&self, message: &str) {
        let line = format!(
            "  {} {}",
            "!".bright_yellow().bold(),
            message.bright_white()
        );
        match &self.bar {
            Some(bar) => bar.println(line),
            None => eprintln!("{line}"),
        }
    }

    /// Wipe the bar from the screen. Call when the session is done so
    /// the final summary renders without a stale spinner above it.
    pub fn finish(&self) {
        if let Some(bar) = &self.bar {
            bar.finish_and_clear();
        }
    }
}

impl Default for UiReporter {
    fn default() -> Self {
        Self::silent()
    }
}
