//! Interactive menu surface — shown when the CLI is invoked with no
//! arguments (typically a double-click on the .exe).
//!
//! Goals:
//! * Greet with the [`crate::banner`] splash.
//! * Offer a small numbered menu the user can drive without flags.
//! * Hand off to the same processing path the flag-driven CLI uses
//!   (via a synthesized [`crate::args::Cli`]) so behaviour stays
//!   consistent across entry points.

use crate::args::{Cli, RepathLayoutArg, Verbosity};
use crate::banner;
use anyhow::Result;
use colored::Colorize;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

/// Run the menu loop until the user picks "Quit". Returns the cumulative
/// `Result` so the outer entry point can set the right exit code.
pub fn run() -> Result<()> {
    banner::print();

    loop {
        let action = prompt_action()?;
        match action {
            Action::FixAll => {
                if let Some(path) = prompt_path("Drop the mod here (or paste a path)")? {
                    let cli = build_fix_all_cli(path);
                    if let Err(e) = crate::run_with_cli(cli) {
                        eprintln!("{} {e:#}", "error:".bright_red().bold());
                    }
                }
            }
            Action::Check => {
                if let Some(path) = prompt_path("Drop the mod here (or paste a path)")? {
                    let cli = build_check_cli(path);
                    if let Err(e) = crate::run_with_cli(cli) {
                        eprintln!("{} {e:#}", "error:".bright_red().bold());
                    }
                }
            }
            Action::FixOnlyNoRepath => {
                if let Some(path) = prompt_path("Drop the mod here (or paste a path)")? {
                    let cli = build_fix_no_repath_cli(path);
                    if let Err(e) = crate::run_with_cli(cli) {
                        eprintln!("{} {e:#}", "error:".bright_red().bold());
                    }
                }
            }
            Action::UpdateCheck => {
                let outcome = crate::version_check::check_version();
                let blocked = crate::version_check::report(&outcome, true);
                if !blocked
                    && matches!(
                        outcome.status,
                        crate::version_check::VersionStatus::UpToDate
                            | crate::version_check::VersionStatus::Unknown
                    )
                {
                    eprintln!(
                        "{} Hematite-CLI {} — up to date.",
                        "✓".bright_green(),
                        env!("CARGO_PKG_VERSION")
                    );
                }
            }
            Action::Quit => break,
        }

        banner::divider();
        if !prompt_yes_no("Do another?", true)? {
            break;
        }
    }

    eprintln!("\n  {} {}\n", "bye!".bright_red(), "👋".dimmed());
    Ok(())
}

// ---------------------------------------------------------------------------
// Menu definition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Action {
    FixAll,
    Check,
    FixOnlyNoRepath,
    UpdateCheck,
    Quit,
}

const MENU: &[(Action, &str, &str)] = &[
    (
        Action::FixAll,
        "Fix a mod",
        "apply all fixes + repath (drag-drop default)",
    ),
    (
        Action::Check,
        "Check a mod",
        "detect issues only, don't modify",
    ),
    (
        Action::FixOnlyNoRepath,
        "Fix without repathing",
        "apply all fixes, keep original paths",
    ),
    (
        Action::UpdateCheck,
        "Check for updates",
        "compare against the remote version manifest",
    ),
    (Action::Quit, "Quit", ""),
];

fn prompt_action() -> Result<Action> {
    eprintln!("  {}", "What do you want to do?".bright_white().bold());
    eprintln!();
    for (i, (_, label, hint)) in MENU.iter().enumerate() {
        if hint.is_empty() {
            eprintln!(
                "    {}  {}",
                format!("[{}]", i + 1).bright_red().bold(),
                label.bright_white()
            );
        } else {
            eprintln!(
                "    {}  {} {}",
                format!("[{}]", i + 1).bright_red().bold(),
                label.bright_white(),
                format!("— {hint}").bright_black()
            );
        }
    }
    eprintln!();

    loop {
        let raw = read_line(&format!(
            "  {} (1-{}): ",
            "choice".bright_red().bold(),
            MENU.len()
        ))?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Accept the leading initial too — "f" for fix, "c" for check, "q" for quit.
        let lower = trimmed.to_lowercase();
        if matches!(lower.as_str(), "q" | "quit" | "exit") {
            return Ok(Action::Quit);
        }
        if let Ok(n) = trimmed.parse::<usize>() {
            if n >= 1 && n <= MENU.len() {
                return Ok(MENU[n - 1].0);
            }
        }
        eprintln!(
            "  {} not a valid choice — try 1-{} (or 'q' to quit).",
            "✗".bright_red(),
            MENU.len()
        );
    }
}

// ---------------------------------------------------------------------------
// Prompts
// ---------------------------------------------------------------------------

/// Ask for a path. Returns `Ok(None)` when the user cancels with an
/// empty line — that bubbles back to the action loop and re-shows the
/// "do another?" prompt without trying to process a bogus path.
fn prompt_path(label: &str) -> Result<Option<PathBuf>> {
    eprintln!();
    eprintln!("  {} {}", "→".bright_red().bold(), label.bright_white());
    eprintln!(
        "    {}",
        "(empty to cancel; quotes auto-stripped)".bright_black()
    );

    loop {
        let raw = read_line(&format!("  {}: ", "path".bright_red().bold()))?;
        let cleaned = strip_path_quotes(raw.trim());
        if cleaned.is_empty() {
            return Ok(None);
        }
        let path = PathBuf::from(&cleaned);
        if path.exists() {
            return Ok(Some(path));
        }
        eprintln!(
            "  {} not found: {} {}",
            "✗".bright_red(),
            cleaned.bright_yellow(),
            "(try again or empty to cancel)".bright_black()
        );
    }
}

fn prompt_yes_no(label: &str, default_yes: bool) -> Result<bool> {
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    loop {
        let raw = read_line(&format!(
            "  {} {} ",
            label.bright_white(),
            hint.bright_black()
        ))?;
        let trimmed = raw.trim().to_lowercase();
        if trimmed.is_empty() {
            return Ok(default_yes);
        }
        match trimmed.as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => eprintln!("  {} answer 'y' or 'n'.", "✗".bright_red()),
        }
    }
}

fn read_line(prompt: &str) -> Result<String> {
    let mut stdout = io::stderr();
    write!(stdout, "{prompt}")?;
    stdout.flush()?;
    let mut buf = String::new();
    io::stdin().lock().read_line(&mut buf)?;
    // Strip trailing newline / carriage-return.
    while buf.ends_with('\n') || buf.ends_with('\r') {
        buf.pop();
    }
    Ok(buf)
}

/// File paths dragged into a terminal often come pre-quoted (Windows
/// Explorer wraps spaces in double-quotes; macOS Terminal in single).
/// Strip a matching outer pair so the path resolves cleanly.
fn strip_path_quotes(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

// ---------------------------------------------------------------------------
// Cli synthesis
// ---------------------------------------------------------------------------

/// Default-fix-all-plus-repath — same as the drag-drop heuristic in `main`.
pub fn build_fix_all_cli(input: PathBuf) -> Cli {
    let mut cli = baseline_cli(input);
    cli.all = true;
    cli.repath = true;
    cli.repath_prefix = Some("hematite".into());
    cli.repath_layout = RepathLayoutArg::Nested;
    cli
}

fn build_check_cli(input: PathBuf) -> Cli {
    let mut cli = baseline_cli(input);
    cli.check = true;
    cli
}

fn build_fix_no_repath_cli(input: PathBuf) -> Cli {
    let mut cli = baseline_cli(input);
    cli.all = true;
    cli
}

fn baseline_cli(input: PathBuf) -> Cli {
    Cli {
        input: Some(input),
        output: None,
        healthbar: false,
        white_model: false,
        black_icons: false,
        particles: false,
        remove_champion_bins: false,
        remove_bnk: false,
        vfx_shape: false,
        remove_anm: false,
        fix_shaders: false,
        validate_entries: false,
        fix_textures: false,
        fix_meshes: false,
        fix_tex_dimensions: false,
        all: false,
        json: false,
        dry_run: false,
        check: false,
        repath: false,
        repath_prefix: None,
        repath_layout: RepathLayoutArg::InFolder,
        invis_texture: false,
        game_wad: None,
        small_mod: false,
        all_skins: false,
        verbosity: Verbosity::Normal,
        skip_version_check: false,
        check_version: false,
    }
}
