//! Effects a specific champion cannot be played without.
//!
//! The overall ability-VFX share answers "does this skin render its kit", and for most
//! champions that is the whole question. For a few it is not. Talon's E vaults walls, and
//! the `Edgemesh` particles are what draw the indicator on every vaultable edge. Lose them
//! and the rest of his kit still renders beautifully, so the overall share stays low while
//! his defining mechanic has become invisible.
//!
//! Folding that into the general ratio would not work in either direction: weighting those
//! effects heavily enough to move the overall share would distort every other champion, and
//! leaving them unweighted is what lets the case through today. So they are measured on
//! their own, against their own threshold.
//!
//! ## Held as config, not code
//! Each entry is a champion, a name fragment, and a share. Talon's Edgemesh is the first,
//! and it will not be the last: every champion with an indicator VFX carrying a mechanic has
//! the same failure. Adding the next one should be a line of TOML, not a release.
//!
//! Fail-open throughout. A champion this does not name, a mod that references none of the
//! effects, or a missing dictionary all mean no finding rather than a guess.

use hematite_types::config::SignatureVfxConfig;
use hematite_types::diagnostic::{Diagnostic, ReasonCatalog};
use std::collections::HashSet;

/// What one signature-effect measurement found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureVerdict {
    /// Matching effects the mod references.
    pub total: usize,
    /// Of those, how many are defined nowhere.
    pub dead: usize,
}

impl SignatureVerdict {
    pub fn share(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        self.dead as f32 / self.total as f32
    }
}

/// Count the matching effects and how many are missing.
///
/// `fragment` must be lowercase. Counted by full name rather than leaf: two effects with
/// the same leaf under different skins are two effects.
pub fn measure<'a>(
    referenced: impl Iterator<Item = &'a str>,
    fragment: &str,
    is_defined: impl Fn(&str) -> bool,
) -> SignatureVerdict {
    let mut total: HashSet<String> = HashSet::new();
    let mut dead: HashSet<String> = HashSet::new();

    for name in referenced {
        let lower = name.to_ascii_lowercase();
        if !lower.contains(fragment) {
            continue;
        }
        total.insert(lower.clone());
        if !is_defined(name) {
            dead.insert(lower);
        }
    }

    SignatureVerdict {
        total: total.len(),
        dead: dead.len(),
    }
}

/// Run every configured signature-effect measurement for this champion.
///
/// `champion` is lowercased before matching. Returns one diagnostic per entry that crossed
/// its threshold; entries for other champions cost a string compare.
pub fn run_all(
    entries: &[SignatureVfxConfig],
    catalog: &ReasonCatalog,
    references: &[String],
    champion: &str,
    is_defined: impl Fn(&str) -> bool,
) -> Vec<Diagnostic> {
    if champion.is_empty() {
        return Vec::new();
    }
    let champion = champion.to_ascii_lowercase();

    let mut out = Vec::new();
    for entry in entries.iter().filter(|e| e.enabled) {
        if !entry.champion.eq_ignore_ascii_case(&champion) {
            continue;
        }
        let Some(reason) = entry.reason.as_deref() else {
            continue;
        };
        let fragment = entry.contains.to_ascii_lowercase();
        if fragment.is_empty() {
            continue;
        }

        let verdict = measure(references.iter().map(String::as_str), &fragment, &is_defined);
        // Nothing referenced is not the same as everything missing.
        if verdict.total == 0 || verdict.share() < entry.dead_at {
            continue;
        }

        let detail = format!(
            "{}/{} {} effects missing",
            verdict.dead, verdict.total, entry.contains
        );
        tracing::info!("signature VFX ({}): {} {}", entry.champion, detail, reason);
        out.push(
            Diagnostic::new(catalog, reason, format!("signature_vfx:{}", entry.champion))
                .with_detail(detail),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn talon() -> Vec<SignatureVfxConfig> {
        vec![SignatureVfxConfig {
            enabled: true,
            champion: "talon".into(),
            contains: "edgemesh".into(),
            dead_at: 0.80,
            reason: Some("talon_edgemesh_missing".into()),
        }]
    }

    fn catalog() -> ReasonCatalog {
        let mut reasons = std::collections::HashMap::new();
        reasons.insert(
            "talon_edgemesh_missing".to_string(),
            hematite_types::diagnostic::ReasonDef {
                severity: hematite_types::diagnostic::Severity::Unplayable,
                title: "Wall-vault indicator missing".into(),
                explain: String::new(),
                remedy: None,
                author: None,
            },
        );
        ReasonCatalog { reasons }
    }

    fn refs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_mostly_dead_signature_effect_fires() {
        let r = refs(&[
            "assets/characters/talon/particles/talon_base_e_edgemesh_01",
            "assets/characters/talon/particles/talon_base_e_edgemesh_02",
            "assets/characters/talon/particles/talon_base_q_mis",
        ]);
        let found = run_all(&talon(), &catalog(), &r, "talon", |_| false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].detail.as_deref(), Some("2/2 edgemesh effects missing"));
    }

    /// The threshold is inclusive, matching the ability-VFX tiers.
    #[test]
    fn exactly_the_threshold_fires() {
        let r = refs(&[
            "x/talon_edgemesh_1",
            "x/talon_edgemesh_2",
            "x/talon_edgemesh_3",
            "x/talon_edgemesh_4",
            "x/talon_edgemesh_5",
        ]);
        let found = run_all(&talon(), &catalog(), &r, "talon", |p| p.ends_with('5'));
        assert_eq!(found.len(), 1, "4 of 5 dead is exactly 80%");
    }

    #[test]
    fn below_the_threshold_is_quiet() {
        let r = refs(&["x/talon_edgemesh_1", "x/talon_edgemesh_2"]);
        let found = run_all(&talon(), &catalog(), &r, "talon", |p| p.ends_with('2'));
        assert!(found.is_empty(), "half missing is not the whole indicator");
    }

    /// Referencing none of the effects is not the same as all of them being gone.
    #[test]
    fn a_mod_that_names_none_of_them_is_quiet() {
        let r = refs(&["assets/characters/talon/particles/talon_base_q_mis"]);
        assert!(run_all(&talon(), &catalog(), &r, "talon", |_| false).is_empty());
    }

    #[test]
    fn another_champion_is_never_measured() {
        let r = refs(&["x/edgemesh_1", "x/edgemesh_2"]);
        assert!(run_all(&talon(), &catalog(), &r, "jhin", |_| false).is_empty());
        assert!(run_all(&talon(), &catalog(), &r, "", |_| false).is_empty());
    }

    #[test]
    fn the_same_effect_named_twice_counts_once() {
        let r = refs(&["X/Talon_Edgemesh_1", "x/talon_edgemesh_1"]);
        let v = measure(r.iter().map(String::as_str), "edgemesh", |_| false);
        assert_eq!((v.dead, v.total), (1, 1));
    }
}
