//! Guards on the shipped reason catalog.
//!
//! Every finding the engine reports goes through `[reasons.*]` in `config/fix_config.toml`.
//! A reason with a missing field does not fail loudly, it renders a blank line in the
//! launcher, so the shape is checked here instead of being discovered in the UI.

use hematite_types::config::FixConfig;
use hematite_types::diagnostic::Severity;

fn shipped() -> FixConfig {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/fix_config.toml"
    ))
    .expect("the repo config must be readable");
    toml::from_str(&raw).expect("the repo config must parse")
}

/// Every reason needs a title, an explanation, and the author-facing sentence.
///
/// The last one is what the Creator Hub renders. A player wants to know what the launcher
/// will do; an author wants to know what is wrong in their files. Both ship, and a reason
/// added without the author half silently shows nothing to the person who can fix it.
#[test]
fn every_reason_carries_both_audiences() {
    let config = shipped();
    assert!(!config.reasons.reasons.is_empty(), "catalog is empty");

    let mut broken: Vec<String> = Vec::new();
    for (id, reason) in &config.reasons.reasons {
        if reason.title.trim().is_empty() {
            broken.push(format!("{id}: no title"));
        }
        if reason.explain.trim().is_empty() {
            broken.push(format!("{id}: no explain"));
        }
        match &reason.author {
            None => broken.push(format!("{id}: no author copy")),
            Some(text) if text.trim().is_empty() => {
                broken.push(format!("{id}: empty author copy"))
            }
            Some(_) => {}
        }
    }
    assert!(broken.is_empty(), "incomplete reasons: {broken:#?}");
}

/// House style: no em-dashes in anything a person reads.
#[test]
fn no_em_dashes_in_user_facing_copy() {
    let config = shipped();
    let mut offenders: Vec<String> = Vec::new();
    for (id, reason) in &config.reasons.reasons {
        for (field, text) in [
            ("title", Some(&reason.title)),
            ("explain", Some(&reason.explain)),
            ("remedy", reason.remedy.as_ref()),
            ("author", reason.author.as_ref()),
        ] {
            if text.is_some_and(|t| t.contains('\u{2014}')) {
                offenders.push(format!("{id}.{field}"));
            }
        }
    }
    for (id, fix) in &config.fixes {
        if fix.description.contains('\u{2014}') {
            offenders.push(format!("fixes.{id}.description"));
        }
    }
    assert!(offenders.is_empty(), "em-dashes present: {offenders:#?}");
}

/// A crash-severity reason with no remedy is fine, but it must say so by omitting the
/// field rather than carrying an empty string, because the UI keys the repair button off
/// whether a remedy is there at all.
#[test]
fn a_remedy_is_absent_rather_than_blank() {
    for (id, reason) in &shipped().reasons.reasons {
        if let Some(remedy) = &reason.remedy {
            assert!(
                !remedy.trim().is_empty(),
                "{id} carries an empty remedy; omit the field instead"
            );
        }
    }
}

/// Every reason a rule names must exist in the catalog.
///
/// A typo here produces a finding with no title and no explanation, which reads as a bug in
/// the check rather than in the config.
#[test]
fn every_reason_a_rule_names_exists() {
    let config = shipped();
    let unknown = hematite_core::check::unknown_reason_ids(&config);
    assert!(unknown.is_empty(), "rules name missing reasons: {unknown:#?}");
}

/// Anything above a warning must tell the reader what it costs them.
#[test]
fn serious_reasons_explain_the_consequence() {
    for (id, reason) in &shipped().reasons.reasons {
        if matches!(reason.severity, Severity::Crash | Severity::Unplayable) {
            assert!(
                reason.explain.len() > 20,
                "{id} is {:?} but its explanation is too thin to be useful",
                reason.severity
            );
        }
    }
}
