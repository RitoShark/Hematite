//! `--crashcheck` output: does this mod crash, and if not, what is wrong with it.
//!
//! Renders the [`CheckReport`] the engine produced. Two audiences, one report:
//! humans get a grouped summary, `--json` callers (Celestial, Quartz, Flint) get the
//! report verbatim with its reason catalog embedded so they need no copy of the config.
//!
//! ## Skipped checks are reported, not hidden
//! A check that could not run is printed as loudly as a failing one. Without that, a
//! missing hash dictionary produces a clean-looking report for a mod nobody actually
//! verified, which is worse than no report at all.

use anyhow::Result;
use colored::Colorize;
use hematite_types::diagnostic::{CheckReport, Severity};
use hematite_types::result::ProcessResult;

/// Render the report and return nothing. Exit status is the caller's business.
pub fn report(result: &ProcessResult, json: bool) -> Result<()> {
    // Rules run per BIN and a mod ships many copies of the same BIN, so the raw list
    // repeats one defect once per clone. Collapse before anyone reads it.
    let mut report = result.report.clone();
    report.dedupe();

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    print_human(&report);
    Ok(())
}

fn print_human(report: &CheckReport) {
    if report.diagnostics.is_empty() {
        // "Nothing found" and "nothing checked" are different claims. Only make the
        // first one when the second is not also true.
        if report.incomplete() {
            println!("{}", "No problems found, but some checks could not run.".yellow());
        } else {
            println!("{}", "No problems found.".green());
        }
        print_skipped(report);
        return;
    }

    let verdict = match report.worst() {
        Some(Severity::Crash) => "This mod crashes the game.".red().bold(),
        Some(Severity::Unplayable) => "This mod loads but cannot be played normally.".red(),
        Some(Severity::Warning) => "This mod works, with problems.".yellow(),
        _ => "Notes.".normal(),
    };
    println!("\n{verdict}\n");

    for severity in [
        Severity::Crash,
        Severity::Unplayable,
        Severity::Warning,
        Severity::Info,
    ] {
        let group: Vec<_> = report.at(severity).collect();
        if group.is_empty() {
            continue;
        }

        let heading = match severity {
            Severity::Crash => "CRASH".red().bold(),
            Severity::Unplayable => "UNPLAYABLE".red(),
            Severity::Warning => "WARNING".yellow(),
            Severity::Info => "INFO".normal(),
        };
        println!("{heading}");

        // One line per distinct reason, with a count, rather than one line per hit: a
        // mod with 200 unmigrated textures should read as one problem, not 200.
        let mut seen: Vec<(&str, usize, Option<&str>)> = Vec::new();
        for d in &group {
            let sample = d.detail.as_deref();
            match seen.iter_mut().find(|(r, _, _)| *r == d.reason.as_str()) {
                Some((_, count, _)) => *count += 1,
                None => seen.push((d.reason.as_str(), 1, sample)),
            }
        }

        for (reason_id, count, sample) in seen {
            let def = report.reasons.get(reason_id);
            let title = def.map(|d| d.title.as_str()).unwrap_or(reason_id);
            let times = if count > 1 {
                format!(" ({count} places)")
            } else {
                String::new()
            };
            println!("  {}{}", title.bold(), times.dimmed());

            if let Some(def) = def {
                println!("    {}", def.explain);
                match &def.remedy {
                    Some(remedy) => println!("    {}", remedy.green()),
                    None => println!("    {}", "Cannot be repaired automatically.".dimmed()),
                }
            }
            if let Some(sample) = sample {
                println!("    {}", sample.dimmed());
            }
        }
        println!();
    }

    print_skipped(report);
}

fn print_skipped(report: &CheckReport) {
    if report.skipped.is_empty() {
        return;
    }
    println!("{}", "Could not check".yellow().bold());
    for (id, why) in &report.skipped {
        println!("  {id}: {why:?}");
    }
    println!();
}
