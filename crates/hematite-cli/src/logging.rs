//! Structured logging with tracing and colored console output.

use crate::args::Verbosity;
use colored::Colorize;
use tracing::Level;
use tracing_subscriber::EnvFilter;

/// Initialize the tracing subscriber based on verbosity level.
pub fn init(verbosity: &Verbosity, json_mode: bool) {
    // Enable ANSI colors on Windows (no-op on other platforms)
    #[cfg(windows)]
    let _ = colored::control::set_virtual_terminal(true);

    // Normal mode is now user-facing — the progress bar (see `ui.rs`)
    // surfaces what the user actually wants to see, and tracing is held
    // to warnings + errors so the per-step "Processing chunk X" stream
    // doesn't drown out the bar. Verbose / Trace are unchanged for
    // developers and bug reports.
    let level = match verbosity {
        Verbosity::Quiet => Level::ERROR,
        Verbosity::Normal => Level::WARN,
        Verbosity::Verbose => Level::DEBUG,
        Verbosity::Trace => Level::TRACE,
    };

    // Per-crate directives mirror the global level. The previous
    // hard-coded `hematite_*=debug` directives short-circuited the
    // user's chosen verbosity, leaving Normal mode just as noisy as
    // Verbose — which is the bug the progress-bar work fixes.
    let crate_level = match verbosity {
        Verbosity::Quiet => "warn",
        Verbosity::Normal => "warn",
        Verbosity::Verbose => "debug",
        Verbosity::Trace => "trace",
    };
    let filter = EnvFilter::from_default_env()
        .add_directive(level.into())
        .add_directive(
            format!("hematite_cli={crate_level}")
                .parse()
                .expect("BUG: hardcoded directive is invalid"),
        )
        .add_directive(
            format!("hematite_core={crate_level}")
                .parse()
                .expect("BUG: hardcoded directive is invalid"),
        )
        .add_directive(
            format!("hematite_ltk={crate_level}")
                .parse()
                .expect("BUG: hardcoded directive is invalid"),
        );

    // `try_init` instead of `init` — the interactive menu re-enters
    // `run_with_cli` for every "do another?" loop iteration, and
    // `init` panics on the second call because the global subscriber
    // is already set. The first call installs the subscriber, every
    // subsequent call no-ops with an Err we intentionally drop.
    let result = if json_mode {
        // JSON output for automation
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .try_init()
    } else {
        // Human-readable colored output
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .with_level(true)
            .try_init()
    };
    if let Err(e) = result {
        // Already-set is expected on re-entry; log at debug so it
        // doesn't muddy a clean run. Anything else is a real config
        // bug worth surfacing.
        tracing::debug!("tracing subscriber already initialised: {}", e);
    }
}

/// Log a session start banner (human-readable mode only).
pub fn log_session_start(input: &str, selected_fixes: &[String]) {
    println!("{}", "═".repeat(60).bright_cyan());
    println!(
        "{}",
        "  Hematite — League of Legends Skin Fixer"
            .bright_cyan()
            .bold()
    );
    println!("{}", "═".repeat(60).bright_cyan());
    println!("{}: {}", "Input".bright_white().bold(), input);

    if selected_fixes.is_empty() {
        println!(
            "{}: {}",
            "Mode".bright_white().bold(),
            "Auto-detect (all fixes)".yellow()
        );
    } else {
        println!(
            "{}: {} selected",
            "Fixes".bright_white().bold(),
            selected_fixes.len()
        );
        for fix_id in selected_fixes {
            println!("  {} {}", "•".bright_cyan(), fix_id.bright_white());
        }
    }
    println!();
}

/// Log session summary.
pub fn log_session_summary(result: &hematite_types::result::ProcessResult, duration: f64) {
    println!();
    println!("{}", "═".repeat(60).bright_cyan());
    println!("{}", "  Summary".bright_cyan().bold());
    println!("{}", "═".repeat(60).bright_cyan());
    println!(
        "{}: {}",
        "Files processed".bright_white().bold(),
        result.files_processed
    );
    println!(
        "{}: {}",
        "Fixes applied".bright_white().bold(),
        result.fixes_applied.to_string().green()
    );
    println!(
        "{}: {}",
        "Fixes failed".bright_white().bold(),
        result.fixes_failed.to_string().red()
    );

    if !result.errors.is_empty() {
        println!("\n{}:", "Errors".red().bold());
        for error in &result.errors {
            println!("  {} {}", "•".red(), error);
        }
    }

    println!("\n{}: {:.2}s", "Duration".bright_white().bold(), duration);
    println!("{}", "═".repeat(60).bright_cyan());
}

/// Log check-mode summary (human-readable).
pub fn log_check_summary(result: &hematite_types::result::ProcessResult) {
    println!();
    println!("{}", "═".repeat(60).bright_cyan());
    println!("{}", "  Check Mode Results".bright_cyan().bold());
    println!("{}", "═".repeat(60).bright_cyan());

    if let Some(info) = &result.check_info {
        println!(
            "{}: {}",
            "Champion".bright_white().bold(),
            info.champion.as_deref().unwrap_or("unknown").yellow()
        );
        println!(
            "{}: {}",
            "Skin Number".bright_white().bold(),
            info.skin_number
                .map(|n| n.to_string())
                .unwrap_or_else(|| "none".to_string())
                .yellow()
        );
        println!(
            "{}: {}",
            "Binless Mod".bright_white().bold(),
            if info.is_binless {
                "yes".red().to_string()
            } else {
                "no".green().to_string()
            }
        );

        if info.detected_issues.is_empty() {
            println!(
                "\n{}",
                "No issues detected — mod looks clean!".green().bold()
            );
        } else {
            println!(
                "\n{} ({}):",
                "Detected Issues".red().bold(),
                info.detected_issues.len()
            );
            for issue in &info.detected_issues {
                println!("  {} {}", "•".red(), issue.bright_white());
            }
        }
    } else {
        println!("{}", "No check info available".yellow());
    }

    println!("{}", "═".repeat(60).bright_cyan());
}
