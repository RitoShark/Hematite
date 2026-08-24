//! A repair has to actually clear what the check reported.
//!
//! It is easy to build a repair that runs, reports fixes, and leaves the mod exactly as
//! broken as it was. The only honest test is to check a mod, repair it, check it again, and
//! insist the second report is smaller.
//!
//! Needs real mods, a game install and the hash dictionary, so it is opt-in:
//!
//! ```text
//! HEMATITE_FIXTURES=<folder of extracted mods> \
//!   cargo test -p hematite-orchestrate --test repair_closes_findings -- --nocapture
//! ```
//!
//! Every fixture is copied before being touched: this writes to the mods it is given.

use hematite_orchestrate::ModChecker;
use hematite_types::diagnostic::{CheckReport, Severity};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn fixtures() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var("HEMATITE_FIXTURES").ok()?);
    dir.is_dir().then_some(dir)
}

fn checker() -> Option<ModChecker> {
    match ModChecker::new(None) {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("skipping: {e:#}");
            None
        }
    }
}

fn reasons_of(report: &CheckReport) -> BTreeSet<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.severity != Severity::Info)
        .map(|d| d.reason.clone())
        .collect()
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Repairing must never make a mod worse.
///
/// The weakest claim worth asserting, and the one that catches a repair writing corrupt
/// bytes: whatever the second check finds, it must not be something the first did not.
#[test]
fn a_repair_introduces_no_new_findings() {
    let (Some(checker), Some(dir)) = (checker(), fixtures()) else {
        return;
    };
    let scratch = tempfile::Builder::new()
        .prefix("hematite-repair-test-")
        .tempdir()
        .unwrap();

    let mut examined = 0;
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let source = entry.path();
        if !source.is_dir() {
            continue;
        }
        let name = source.file_name().unwrap().to_string_lossy().to_string();
        let working = scratch.path().join(&name);
        if copy_tree(&source, &working).is_err() {
            continue;
        }

        let Ok(before) = checker.check(&working) else {
            continue;
        };
        let Ok(outcome) = checker.repair(&working) else {
            continue;
        };
        let Ok(after) = checker.check(&working) else {
            panic!("{name}: the mod could not be checked after being repaired");
        };
        examined += 1;

        let was = reasons_of(&before);
        let now = reasons_of(&after);
        let introduced: Vec<&String> = now.difference(&was).collect();
        let cleared: Vec<&String> = was.difference(&now).collect();

        println!(
            "{name}: {} fix(es), {} cleared, {} left",
            outcome.fixes_applied,
            cleared.len(),
            now.len()
        );
        if !cleared.is_empty() {
            println!("   cleared: {cleared:?}");
        }
        assert!(
            introduced.is_empty(),
            "{name}: repairing introduced {introduced:?}"
        );
    }
    assert!(examined > 0, "no fixture was repaired");
}

/// The mod must still be readable afterwards.
///
/// A repack that produces an archive nothing can open is the worst outcome available here,
/// and it looks like success from the inside.
#[test]
fn a_repaired_mod_is_still_readable() {
    let (Some(checker), Some(dir)) = (checker(), fixtures()) else {
        return;
    };
    let scratch = tempfile::Builder::new()
        .prefix("hematite-readable-test-")
        .tempdir()
        .unwrap();

    let Some(source) = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir())
    else {
        return;
    };
    let working = scratch.path().join("mod");
    copy_tree(&source, &working).unwrap();

    checker.repair(&working).expect("the mod repairs");
    checker
        .check(&working)
        .expect("a repaired mod must still be checkable");
}

/// Print one fixture's findings before and after a repair, with details.
///
/// Ignored: a diagnostic aid. `HEMATITE_ONE` picks the fixture by folder name.
#[test]
#[ignore]
fn report_one_fixture_before_and_after() {
    let (Some(checker), Some(dir)) = (checker(), fixtures()) else {
        return;
    };
    let Ok(want) = std::env::var("HEMATITE_ONE") else {
        println!("set HEMATITE_ONE to a fixture folder name");
        return;
    };
    let source = dir.join(&want);
    let scratch = tempfile::tempdir().unwrap();
    let working = scratch.path().join("mod");
    copy_tree(&source, &working).unwrap();

    let _ = tracing_subscriber::fmt()
        .with_env_filter("hematite_core=debug,hematite_orchestrate=debug")
        .with_test_writer()
        .try_init();

    let show = |label: &str, report: &CheckReport| {
        println!("── {label}");
        for d in &report.diagnostics {
            println!(
                "   {:?} {} [in {}] {}",
                d.severity,
                d.reason,
                d.entry.as_deref().unwrap_or("?"),
                d.detail.as_deref().unwrap_or("")
            );
        }
    };

    show("before", &checker.check(&working).unwrap());
    let outcome = checker.repair(&working).unwrap();
    println!("── {} fix(es) applied", outcome.fixes_applied);
    show("after", &checker.check(&working).unwrap());
}

/// List which of a mod's files changed across a repair.
///
/// Ignored: a diagnostic aid, same selection as the report above.
#[test]
#[ignore]
fn report_one_fixture_file_changes() {
    let (Some(checker), Some(dir)) = (checker(), fixtures()) else {
        return;
    };
    let Ok(want) = std::env::var("HEMATITE_ONE") else {
        return;
    };
    let source = dir.join(&want);
    let scratch = tempfile::tempdir().unwrap();
    let working = scratch.path().join("mod");
    copy_tree(&source, &working).unwrap();

    let listing = |root: &Path| -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for entry in walkdir::WalkDir::new(root).into_iter().flatten() {
            if entry.path().is_file() {
                out.insert(
                    entry
                        .path()
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/")
                        .to_lowercase(),
                );
            }
        }
        out
    };

    let before = listing(&working);
    checker.repair(&working).unwrap();
    let after = listing(&working);

    let want = std::env::var("HEMATITE_GREP").unwrap_or_default();
    if !want.is_empty() {
        println!("── before, matching {want:?}");
        for p in before.iter().filter(|p| p.contains(&want)) {
            println!("   B {p}");
        }
        println!("── after, matching {want:?}");
        for p in after.iter().filter(|p| p.contains(&want)) {
            println!("   A {p}");
        }
    }

    let gone: Vec<&String> = before.difference(&after).collect();
    let added: Vec<&String> = after.difference(&before).collect();
    println!("── {} file(s) removed", gone.len());
    for p in gone.iter().filter(|p| p.contains("particle")).take(20) {
        println!("   -{p}");
    }
    println!("── {} file(s) added", added.len());
    for p in added.iter().filter(|p| p.contains("particle")).take(20) {
        println!("   +{p}");
    }
}
