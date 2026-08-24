//! Check many mods in one run.
//!
//! Checking a folder of mods one process at a time pays the same setup for each: over a
//! second to load the hash dictionary, plus a game index and a shader set, all identical
//! across mods and all thrown away at exit. On a folder of seven that is most of the
//! total runtime spent re-reading the same data.
//!
//! Here the dictionary, the game index and the shader set are built once and shared, and
//! the mods are checked concurrently. Both matter and the sharing matters more: it turns
//! a fixed per-mod cost into a fixed per-run one.
//!
//! Every mod is reported, including one that fails. A batch that aborts on the first bad
//! archive is useless for the case it exists to serve.

use anyhow::Result;
use colored::Colorize;
use hematite_core::traits::HashProvider;
use hematite_types::champion::CharacterRelations;
use hematite_types::config::FixConfig;
use hematite_types::diagnostic::{CheckReport, Severity};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One mod's outcome.
pub struct BatchEntry {
    pub path: PathBuf,
    pub report: Option<CheckReport>,
    pub error: Option<String>,
}

impl BatchEntry {
    fn worst(&self) -> Option<Severity> {
        self.report.as_ref().and_then(CheckReport::worst)
    }
}

/// Every mod archive directly inside `dir`.
///
/// Not recursive: a mod folder contains its own `.wad.client` directories, and descending
/// would treat each as a separate mod.
pub fn discover(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_lowercase();
        let is_mod = name.ends_with(".fantome")
            || name.ends_with(".zip")
            || name.ends_with(".modpkg")
            || (path.is_dir() && name.ends_with(".wad.client"));
        if is_mod {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// Check every mod, sharing the loaded data and running them concurrently.
///
/// `worker_count` of zero picks a thread per available core, capped at the mod count.
#[allow(clippy::too_many_arguments)]
pub fn run(
    mods: &[PathBuf],
    config: &FixConfig,
    selected_fixes: &[String],
    champions: &CharacterRelations,
    hashes: &Arc<dyn HashProvider>,
    live: Option<&hematite_orchestrate::LiveGameProvider>,
    worker_count: usize,
) -> Vec<BatchEntry> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<BatchEntry>>> =
        (0..mods.len()).map(|_| std::sync::Mutex::new(None)).collect();

    let workers = if worker_count == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(mods.len().max(1))
    } else {
        worker_count.min(mods.len().max(1))
    };

    tracing::info!("checking {} mod(s) across {} worker(s)", mods.len(), workers);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(path) = mods.get(i) else {
                    break;
                };

                // Silent reporter: concurrent progress bars would interleave into
                // nonsense, and the per-mod summary is printed afterwards anyway.
                let ui = crate::ui::UiReporter::new(crate::ui::Mode::Silent);
                let outcome = crate::process::process_input(
                    path,
                    config,
                    selected_fixes,
                    champions,
                    true,  // never write during a batch check
                    true,  // detect only
                    None,  // no repath
                    ui,
                    live,
                    false,
                    None,
                    false,
                    Some(hashes),
                );

                let entry = match outcome {
                    Ok(result) => {
                        let mut report = result.report;
                        report.dedupe();
                        BatchEntry {
                            path: path.clone(),
                            report: Some(report),
                            error: None,
                        }
                    }
                    // One unreadable archive must not take the batch down with it.
                    Err(e) => BatchEntry {
                        path: path.clone(),
                        report: None,
                        error: Some(format!("{e:#}")),
                    },
                };
                *results[i].lock().expect("poisoned") = Some(entry);
            });
        }
    });

    results
        .into_iter()
        .filter_map(|slot| slot.into_inner().ok().flatten())
        .collect()
}

/// One line per mod, worst first, then a tally.
pub fn print_summary(entries: &[BatchEntry]) {
    let rank = |e: &BatchEntry| match e.worst() {
        _ if e.error.is_some() => 0,
        Some(Severity::Crash) => 1,
        Some(Severity::Unplayable) => 2,
        Some(Severity::Warning) => 3,
        _ => 4,
    };
    let mut sorted: Vec<&BatchEntry> = entries.iter().collect();
    sorted.sort_by_key(|e| (rank(e), e.path.clone()));

    println!();
    for entry in &sorted {
        let name = entry
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");

        if let Some(err) = &entry.error {
            println!("  {}  {name}", "FAILED    ".red().bold());
            println!("              {}", err.dimmed());
            continue;
        }

        let (label, detail) = match entry.worst() {
            Some(Severity::Crash) => ("CRASH     ".red().bold(), summarise(entry)),
            Some(Severity::Unplayable) => ("UNPLAYABLE".red(), summarise(entry)),
            Some(Severity::Warning) => ("WARNING   ".yellow(), summarise(entry)),
            _ => ("OK        ".green(), String::new()),
        };
        println!("  {label}  {name}");
        if !detail.is_empty() {
            println!("              {}", detail.dimmed());
        }
    }

    let crashes = sorted
        .iter()
        .filter(|e| e.worst() == Some(Severity::Crash))
        .count();
    let unplayable = sorted
        .iter()
        .filter(|e| e.worst() == Some(Severity::Unplayable))
        .count();
    let failed = sorted.iter().filter(|e| e.error.is_some()).count();
    let clean = sorted
        .iter()
        .filter(|e| e.error.is_none() && e.worst().is_none())
        .count();

    println!();
    println!(
        "  {} mod(s): {crashes} crashing, {unplayable} unplayable, {clean} clean, {failed} unreadable",
        sorted.len()
    );
    println!();
}

/// Distinct reasons at the worst severity, so one line says what is actually wrong.
fn summarise(entry: &BatchEntry) -> String {
    let Some(report) = &entry.report else {
        return String::new();
    };
    let Some(worst) = report.worst() else {
        return String::new();
    };
    let mut titles: Vec<&str> = report
        .at(worst)
        .map(|d| {
            report
                .reasons
                .get(&d.reason)
                .map(|r| r.title.as_str())
                .unwrap_or(d.reason.as_str())
        })
        .collect();
    titles.sort_unstable();
    titles.dedup();
    titles.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, worst: Option<Severity>, error: Option<&str>) -> BatchEntry {
        let report = worst.map(|sev| {
            let mut r = CheckReport::default();
            r.push(hematite_types::diagnostic::Diagnostic::with_resolved_severity(
                "x", sev, "rule",
            ));
            r
        });
        BatchEntry {
            path: PathBuf::from(path),
            report,
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn worst_reads_through_to_the_report() {
        assert_eq!(
            entry("a", Some(Severity::Crash), None).worst(),
            Some(Severity::Crash)
        );
        assert_eq!(entry("b", None, None).worst(), None);
    }

    /// A mod that could not be read is not a clean mod, and must not be counted as one.
    #[test]
    fn an_unreadable_mod_has_no_verdict() {
        let e = entry("c", None, Some("bad zip"));
        assert!(e.worst().is_none());
        assert!(e.error.is_some());
    }

    #[test]
    fn discovery_finds_archives_and_ignores_other_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.fantome"), b"x").unwrap();
        std::fs::write(dir.path().join("b.zip"), b"x").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("Jhin.wad.client")).unwrap();
        std::fs::create_dir(dir.path().join("random_folder")).unwrap();

        let found = discover(dir.path()).unwrap();
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["Jhin.wad.client", "a.fantome", "b.zip"]);
    }
}
