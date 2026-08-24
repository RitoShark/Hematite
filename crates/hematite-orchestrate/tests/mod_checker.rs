//! The embedding entry point, exercised against real content.
//!
//! `ModChecker` is what Celestial calls instead of carrying its own copy of the check. If
//! it disagrees with the CLI, one of the two is lying to its users, so these run over the
//! real fixtures when they are present and are skipped when they are not.
//!
//! Set `HEMATITE_FIXTURES` to a folder of extracted mods (each a directory of `.wad.client`
//! folders) to enable the fixture-backed cases. They need the hash dictionary and a League
//! install, so they stay opt-in rather than failing on a machine without them.

use hematite_orchestrate::ModChecker;
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

/// The catalog has to arrive with the report, or the caller has findings it cannot render.
#[test]
fn a_checker_carries_the_reason_catalog() {
    let Some(checker) = checker() else { return };
    assert!(
        !checker.reasons().reasons.is_empty(),
        "no reasons loaded; the config did not resolve"
    );
}

/// A folder holding no archive is a caller mistake, not a clean mod. Reporting it clean
/// would tell someone their broken import is fine.
#[test]
fn a_folder_with_no_archive_is_an_error_not_a_pass() {
    let Some(checker) = checker() else { return };
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("META")).unwrap();
    assert!(checker.check(dir.path()).is_err());
}

/// Every finding must resolve to a reason with a title, or the launcher renders blanks.
#[test]
fn every_finding_resolves_against_the_catalog() {
    let (Some(checker), Some(dir)) = (checker(), fixtures()) else {
        return;
    };
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(report) = checker.check(&path) else {
            continue;
        };
        checked += 1;
        for finding in &report.diagnostics {
            let reason = report
                .reasons
                .get(&finding.reason)
                .unwrap_or_else(|| panic!("{}: no catalog entry for {}", path.display(), finding.reason));
            assert!(
                !reason.title.trim().is_empty(),
                "{}: {} has no title",
                path.display(),
                finding.reason
            );
        }
    }
    assert!(checked > 0, "no fixture produced a report");
}

/// Checking the same mod twice must say the same thing. The checker memoises game BINs and
/// shader definitions across calls, and a cache that leaks state between mods would show up
/// here first.
#[test]
fn checking_twice_gives_the_same_answer() {
    let (Some(checker), Some(dir)) = (checker(), fixtures()) else {
        return;
    };
    let Some(mod_dir) = first_mod(&dir) else { return };

    let first = titles(&checker.check(&mod_dir).unwrap());
    let second = titles(&checker.check(&mod_dir).unwrap());
    assert_eq!(first, second);
}

/// One checker over several mods must give each the same answer a fresh checker would.
/// This is the property the shared game index exists to preserve, and the one a stale cache
/// would break.
#[test]
fn a_shared_checker_matches_a_fresh_one_per_mod() {
    let (Some(shared), Some(dir)) = (checker(), fixtures()) else {
        return;
    };
    let mods: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .take(3)
        .collect();
    if mods.len() < 2 {
        return;
    }

    for path in &mods {
        let Ok(from_shared) = shared.check(path) else {
            continue;
        };
        let fresh = checker().expect("a checker built once builds again");
        let from_fresh = fresh.check(path).expect("the same mod checks again");
        assert_eq!(
            titles(&from_shared),
            titles(&from_fresh),
            "{} differs between a shared and a fresh checker",
            path.display()
        );
    }
}

fn first_mod(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir())
}

fn titles(report: &hematite_types::diagnostic::CheckReport) -> Vec<String> {
    let mut out: Vec<String> = report
        .diagnostics
        .iter()
        .map(|d| format!("{:?}:{}", d.severity, d.reason))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Print what the checker finds, for comparing against the CLI by hand.
///
/// Ignored: it is a reporting aid, not an assertion. Run with
/// `cargo test -p hematite-orchestrate --test mod_checker -- --ignored --nocapture`.
#[test]
#[ignore]
fn report_every_fixture() {
    let (Some(checker), Some(dir)) = (checker(), fixtures()) else {
        println!("no fixtures");
        return;
    };
    let mut mods: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    mods.sort();
    for path in mods {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        match checker.check(&path) {
            Ok(report) => {
                let mut titles: Vec<String> = report
                    .diagnostics
                    .iter()
                    .filter_map(|d| report.reasons.get(&d.reason).map(|r| r.title.clone()))
                    .collect();
                titles.sort();
                titles.dedup();
                println!("{:<46} {}", &name[..name.len().min(45)], titles.join(" | "));
            }
            Err(e) => println!("{name:<46} ERROR {e:#}"),
        }
    }
}
