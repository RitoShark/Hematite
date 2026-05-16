//! Hematite CLI — League of Legends custom skin fixer.
//!
//! ## Entry modes
//!
//! The binary picks one of three flows based on the raw process args
//! (see [`detect_entry_mode`]):
//!
//! - **Interactive** — no args (typically a double-click). Shows the
//!   big banner and a numbered menu via [`interactive::run`].
//! - **Drag-drop** — a single existing path, no flags. Applies every
//!   fix and repaths under the `hematite` namespace. Same effective
//!   Cli as "Fix a mod" in the interactive menu.
//! - **Flag-driven** — anything else. Standard clap-parsed Cli used
//!   by users and scripts that want explicit control.
//!
//! All three converge on [`run_with_cli`], the single source of truth
//! for what a fix session does.

mod args;
mod banner;
mod hash_downloader;
mod interactive;
mod logging;
mod process;
mod remote;
mod version_check;

use anyhow::Result;
use args::{Cli, RepathLayoutArg};
use clap::Parser;
use hematite_types::champion::CharacterRelations;
use hematite_types::repath::RepathOptions;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    // Enable Windows ANSI virtual-terminal processing before ANY output
    // hits stderr. The banner / interactive menu / drag-drop banner all
    // print BEFORE `logging::init` runs, so they'd otherwise dump raw
    // ESC[...m sequences on cmd.exe. Idempotent + no-op elsewhere.
    #[cfg(windows)]
    let _ = colored::control::set_virtual_terminal(true);

    let raw: Vec<String> = std::env::args().collect();
    let mode = detect_entry_mode(&raw);

    // Interactive + drag-drop modes are user-facing by definition —
    // the human is sitting at a terminal looking at the output, so
    // force colours on regardless of TTY auto-detection. (Auto-detect
    // can wrongly disable colours under certain terminal-emulator /
    // pipe configurations.) Flag-driven mode keeps auto-detect so
    // `--json` and piped scripts stay clean.
    if matches!(mode, EntryMode::Interactive | EntryMode::DragDrop(_)) {
        colored::control::set_override(true);
    }

    let result = match mode {
        EntryMode::Interactive => {
            // Menu loop handles its own banner + pacing. Skip the
            // "press enter" pause — the user is already in a prompt.
            let r = interactive::run();
            return early_exit(r, true);
        }
        EntryMode::DragDrop(path) => {
            // Drag-drop = "do the thing" — apply every fix and repath
            // under the `hematite` namespace. Same exact CLI a user
            // would assemble manually, just preset.
            banner::print();
            let cli = interactive::build_fix_all_cli(path);
            run_with_cli(cli)
        }
        EntryMode::Flagged => {
            // Normal clap parse — let the user drive.
            run_with_cli(Cli::parse())
        }
    };

    if let Err(ref e) = result {
        eprintln!("Error: {e:#}");
    }

    // Pause before exit so the console doesn't close instantly when
    // double-clicked / drag-dropped. `--json` and `--no-pause` callers
    // bypass.
    if !raw.iter().any(|a| a == "--json" || a == "--no-pause") {
        eprintln!();
        eprintln!("Press Enter to exit...");
        let _ = std::io::Read::read(&mut std::io::stdin(), &mut [0u8]);
    }

    if result.is_err() {
        std::process::exit(1);
    }
}

/// Same as falling through to the bottom of `main`, but used by entry
/// modes that own their own pacing (the interactive loop ends with
/// "bye!" — a "press enter" afterwards would be obnoxious).
fn early_exit(result: Result<()>, skip_pause: bool) {
    if let Err(ref e) = result {
        eprintln!("Error: {e:#}");
    }
    if !skip_pause {
        eprintln!();
        eprintln!("Press Enter to exit...");
        let _ = std::io::Read::read(&mut std::io::stdin(), &mut [0u8]);
    }
    if result.is_err() {
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Entry mode detection
// ---------------------------------------------------------------------------

enum EntryMode {
    Interactive,
    DragDrop(PathBuf),
    Flagged,
}

/// Pick an entry mode from the raw argv. Runs BEFORE clap so we can
/// support "no args" cleanly (clap's `required_unless_present` would
/// otherwise error out before we got a chance).
fn detect_entry_mode(raw: &[String]) -> EntryMode {
    // First element is the exe path.
    let user_args: Vec<&str> = raw.iter().skip(1).map(|s| s.as_str()).collect();

    // Pure double-click: nothing at all → interactive menu.
    if user_args.is_empty() {
        return EntryMode::Interactive;
    }

    // Single argument, no flags, points at something that exists →
    // drag-drop. Heuristic intentionally tight: any flag at all
    // (including `-`) routes through clap so users with explicit
    // intent get the existing behaviour.
    if user_args.len() == 1 {
        let only = user_args[0];
        if !only.starts_with('-') && Path::new(only).exists() {
            return EntryMode::DragDrop(PathBuf::from(only));
        }
    }

    EntryMode::Flagged
}

// ---------------------------------------------------------------------------
// Shared processing entry point
// ---------------------------------------------------------------------------

/// Run a fix session against an already-built [`Cli`]. The flag-driven
/// flow, the interactive menu, and the drag-drop fast path all
/// converge here so behaviour stays consistent.
pub fn run_with_cli(cli: Cli) -> Result<()> {
    // Initialize logging
    logging::init(&cli.verbosity, cli.json);

    // -- Version gate -------------------------------------------------------
    // Runs before input validation so `--check-version` works without
    // requiring an input path. JSON-mode callers shouldn't see human-
    // readable banners; the hard-block still fires but silently.
    let version_outcome = version_check::check_version();
    if cli.check_version {
        let blocked = version_check::report(&version_outcome, true);
        if !blocked
            && matches!(
                version_outcome.status,
                version_check::VersionStatus::UpToDate
                    | version_check::VersionStatus::Unknown
            )
        {
            eprintln!(
                "Hematite-CLI {} — up to date.",
                env!("CARGO_PKG_VERSION")
            );
        }
        if blocked {
            std::process::exit(2);
        }
        return Ok(());
    }
    if !cli.json {
        let blocked = version_check::report(&version_outcome, cli.skip_version_check);
        if blocked {
            anyhow::bail!(
                "Refusing to run: CLI is older than the published minimum. \
                 Pass --skip-version-check to override."
            );
        }
    } else if matches!(version_outcome.status, version_check::VersionStatus::Outdated { .. })
        && !cli.skip_version_check
    {
        anyhow::bail!(
            "Refusing to run: CLI is older than the published minimum. \
             Pass --skip-version-check to override (see --check-version)."
        );
    }

    // After `--check-version` short-circuit, `input` is guaranteed present
    // by clap's `required_unless_present` (or by the entry-mode dispatcher
    // for the interactive / drag-drop paths).
    let input = cli
        .input
        .as_ref()
        .expect("input must be present at this point — entry modes guarantee it");

    if !input.exists() {
        anyhow::bail!("Input path does not exist: {}", input.display());
    }

    let start_time = Instant::now();

    let selected_fixes = args::collect_selected_fixes(&cli);

    let config = remote::load_fix_config();
    let champion_list = remote::load_champion_list();
    let champions = CharacterRelations::from_champion_list(&champion_list);

    if !cli.json {
        logging::log_session_start(&input.to_string_lossy(), &selected_fixes);
    }

    let dry_run = cli.dry_run || cli.check;

    // Build repath options.
    // Priority: CLI flags > fix_config.json repath section.
    // --repath flag or config.repath.enabled activates repathing.
    let repath_opts: Option<RepathOptions> = {
        let cfg = &config.repath;
        let active = cli.repath || cfg.enabled;
        if active {
            // Prefix priority: explicit CLI > config (when non-placeholder)
            // > Topaz-derived from filename.
            let prefix = cli
                .repath_prefix
                .clone()
                .or_else(|| {
                    let p = &cfg.prefix;
                    if p.is_empty() || p == "bum" || p == "hematite" {
                        None
                    } else {
                        Some(p.clone())
                    }
                })
                .unwrap_or_else(|| derive_prefix_from_input(input));
            let mut opts = RepathOptions::new(prefix);
            opts.layout = cli.repath_layout.into();
            opts.invis_texture = cli.invis_texture || cfg.invis_texture;
            opts.skip_vo = cfg.skip_vo;
            opts.game_wad = cli.game_wad.clone();
            Some(opts)
        } else {
            None
        }
    };

    let result = process::process_input(
        input,
        &config,
        &selected_fixes,
        &champions,
        dry_run,
        cli.check,
        repath_opts.as_ref(),
    )?;

    let duration = start_time.elapsed().as_secs_f64();

    if cli.check {
        if cli.json {
            output_check_json(&result)?;
        } else {
            logging::log_check_summary(&result);
        }
    } else if cli.json {
        output_json(&result, duration)?;
    } else {
        logging::log_session_summary(&result, duration);
    }

    if result.errors.is_empty() {
        // Silence: the `RepathLayoutArg` field is consumed here implicitly
        // through `cli.repath_layout` above; suppress a defensive warning
        // some toolchains emit when a value-enum is only used to be
        // converted away.
        let _: RepathLayoutArg = cli.repath_layout;
        Ok(())
    } else {
        anyhow::bail!("Processing completed with {} error(s)", result.errors.len());
    }
}

// ---------------------------------------------------------------------------
// Helpers (unchanged)
// ---------------------------------------------------------------------------

/// Best-effort Topaz-style prefix from an input filename like
/// "Sasuke by Noxli (V1.0).fantome" or "ahri_skin5.zip".
///
/// Picks the first alphabetic run as the "champion" and the first digit run
/// after it as the skin number.  Falls back to "bum" if neither is found —
/// `RepathOptions::derive_prefix` already does the rest.
fn derive_prefix_from_input(input: &std::path::Path) -> String {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("mod");
    let mut champion = String::new();
    for c in stem.chars() {
        if c.is_ascii_alphabetic() {
            champion.push(c);
        } else if !champion.is_empty() {
            break;
        }
    }
    let after_champ: String = stem
        .chars()
        .skip(champion.len())
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let skin_no: u32 = after_champ.parse().unwrap_or(0);
    RepathOptions::derive_prefix(&champion, skin_no)
}

fn output_check_json(result: &hematite_types::result::ProcessResult) -> Result<()> {
    if let Some(check_info) = &result.check_info {
        let json = serde_json::to_string_pretty(check_info)?;
        println!("{}", json);
    } else {
        println!("{{}}");
    }
    Ok(())
}

fn output_json(result: &hematite_types::result::ProcessResult, duration: f64) -> Result<()> {
    #[derive(serde::Serialize)]
    struct JsonOutput {
        success: bool,
        files_processed: u32,
        fixes_applied: u32,
        fixes_failed: u32,
        errors: Vec<String>,
        duration_seconds: f64,
    }

    let output = JsonOutput {
        success: result.errors.is_empty(),
        files_processed: result.files_processed,
        fixes_applied: result.fixes_applied,
        fixes_failed: result.fixes_failed,
        errors: result.errors.clone(),
        duration_seconds: duration,
    };

    let json = serde_json::to_string_pretty(&output)?;
    println!("{}", json);

    Ok(())
}
