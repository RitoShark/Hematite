//! How much of a mod's referenced art actually resolves.
//!
//! Some defects are not about one missing file, they are about proportion. A skin missing
//! one particle texture looks fine; a skin missing most of them renders as a grey mess
//! that is technically playable and obviously broken. A single dead reference cannot
//! distinguish those, so this counts what a mod points at and what share of it is absent.
//!
//! ## Two thresholds, and the operators are not interchangeable
//! Each check has a warn level and a fail level, and the comparison differs between them
//! by design: warn is exceeded (`>`), fail is reached (`>=`). Those came from tuning
//! against real mods, so a mod sitting exactly on a boundary lands on a deliberate side.
//! Swapping either operator silently moves that line.
//!
//! ## Counted by basename, not by path
//! Two references to `foo.dds` from different folders are the same art as far as the
//! player is concerned, and a repathed mod writes the same file under several paths.
//! Counting paths would inflate both halves of the ratio and let a repath change the
//! verdict.

use std::collections::HashSet;

/// One measurement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RatioVerdict {
    /// References that resolve nowhere.
    pub missing: usize,
    /// References counted.
    pub total: usize,
}

impl RatioVerdict {
    /// Share missing, as a fraction.
    pub fn share(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        self.missing as f32 / self.total as f32
    }

    /// Whole-percent share, truncated to match how the figure is reported elsewhere.
    pub fn percent(&self) -> u32 {
        (self.share() * 100.0) as u32
    }

    pub fn describe(&self) -> String {
        format!("{}/{} ({}%)", self.missing, self.total, self.percent())
    }
}

/// Which side of the two thresholds a measurement fell on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RatioTier {
    /// At or above the fail level.
    Fail,
    /// Above the warn level but below fail.
    Warn,
}

/// Classify a measurement.
///
/// Returns `None` below the warn level, or when the sample is too small to mean anything:
/// with a handful of references, one missing file is a large percentage and says nothing.
///
/// `warn_at` is exclusive and `fail_at` inclusive. See the module docs.
pub fn classify(
    verdict: &RatioVerdict,
    min_total: usize,
    warn_at: f32,
    fail_at: f32,
) -> Option<RatioTier> {
    if verdict.total < min_total {
        return None;
    }
    let share = verdict.share();
    if share >= fail_at {
        return Some(RatioTier::Fail);
    }
    if share > warn_at {
        return Some(RatioTier::Warn);
    }
    None
}

/// Basename of an asset path, lowercased.
///
/// The segment after the last separator, with no directory context. See the module docs
/// for why the comparison is by basename.
pub fn asset_basename(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    normalized
        .rsplit('/')
        .next()
        .unwrap_or(&normalized)
        .to_lowercase()
}

/// Re-anchor a repathed asset path on its `characters/` segment.
///
/// A repathed mod rewrites `ASSETS/Characters/...` to `ASSETS/<prefix>/Characters/...` and
/// ships only what it replaces, so an untouched reference names a file that exists at the
/// stock path. Without this the untouched references all count as missing.
pub fn canonical(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let idx = normalized.to_lowercase().find("characters/")?;
    Some(format!("ASSETS/{}", &normalized[idx..]))
}

/// Count references and how many resolve nowhere.
///
/// `resolves` answers whether a path exists in the mod or the game. Four things satisfy a
/// reference: the literal path, its canonical form, and for a `.dds` either of those with
/// a `.tex` extension, because the client loads the converted form.
pub fn measure<'a>(
    references: impl Iterator<Item = &'a str>,
    resolves: impl Fn(&str) -> bool,
) -> RatioVerdict {
    let mut total: HashSet<String> = HashSet::new();
    let mut resolved: HashSet<String> = HashSet::new();

    for reference in references {
        let name = asset_basename(reference);
        // Skip the lookup once this art is known to resolve; the cost is a WAD query.
        let known = resolved.contains(&name);
        total.insert(name.clone());
        if !known && satisfied(reference, &resolves) {
            resolved.insert(name);
        }
    }

    // Subtract at the end rather than accumulating misses as they appear. The same art is
    // referenced from several paths in a repathed mod, and only some of them resolve, so a
    // basename is missing only when NO reference to it resolved.
    RatioVerdict {
        missing: total.difference(&resolved).count(),
        total: total.len(),
    }
}

fn satisfied(reference: &str, resolves: &impl Fn(&str) -> bool) -> bool {
    if resolves(reference) {
        return true;
    }
    if let Some(canon) = canonical(reference) {
        if resolves(&canon) {
            return true;
        }
    }
    // The client loads the converted form, so a `.dds` reference is alive when its `.tex`
    // twin exists.
    if reference.to_lowercase().ends_with(".dds") {
        let twin = format!("{}.tex", &reference[..reference.len() - 4]);
        if resolves(&twin) {
            return true;
        }
        if let Some(canon) = canonical(&twin) {
            if resolves(&canon) {
                return true;
            }
        }
    }
    false
}


/// Run every configured proportion check over one mod.
///
/// `references` is every asset path the mod's BINs name; `resolves` answers whether a path
/// exists in the mod or the game. Returns one diagnostic per check that crossed a
/// threshold.
pub fn run_all(
    checks: &[hematite_types::config::RatioCheckConfig],
    catalog: &hematite_types::diagnostic::ReasonCatalog,
    references: &[String],
    resolves: impl Fn(&str) -> bool,
) -> Vec<hematite_types::diagnostic::Diagnostic> {
    let mut out = Vec::new();
    for check in checks.iter().filter(|c| c.enabled) {
        let exts: Vec<String> = check.extensions.iter().map(|e| e.to_lowercase()).collect();
        let matching: Vec<&str> = references
            .iter()
            .filter(|r| {
                let lower = r.to_lowercase();
                exts.iter().any(|e| lower.ends_with(e.as_str()))
            })
            .map(String::as_str)
            .collect();
        if matching.is_empty() {
            continue;
        }

        let verdict = measure(matching.into_iter(), &resolves);
        let Some(tier) = classify(&verdict, check.min_total, check.warn_at, check.fail_at)
        else {
            continue;
        };
        let reason = match tier {
            RatioTier::Fail => check.fail_reason.as_deref(),
            RatioTier::Warn => check.warn_reason.as_deref(),
        };
        let Some(reason) = reason else {
            continue;
        };
        tracing::info!("{}: {} {}", check.id, verdict.describe(), reason);
        out.push(
            hematite_types::diagnostic::Diagnostic::new(catalog, reason, check.id.clone())
                .with_detail(format!("{} of referenced assets are missing", verdict.describe())),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_and_percent_truncate() {
        let v = RatioVerdict {
            missing: 799,
            total: 1000,
        };
        assert_eq!(v.percent(), 79);
        assert_eq!(v.describe(), "799/1000 (79%)");
    }

    /// The operators are asymmetric on purpose: warn is exceeded, fail is reached.
    #[test]
    fn warn_is_exclusive_and_fail_is_inclusive() {
        let at_warn = RatioVerdict {
            missing: 25,
            total: 100,
        };
        assert_eq!(classify(&at_warn, 20, 0.25, 0.80), None, "exactly warn");

        let over_warn = RatioVerdict {
            missing: 26,
            total: 100,
        };
        assert_eq!(
            classify(&over_warn, 20, 0.25, 0.80),
            Some(RatioTier::Warn)
        );

        let at_fail = RatioVerdict {
            missing: 80,
            total: 100,
        };
        assert_eq!(
            classify(&at_fail, 20, 0.25, 0.80),
            Some(RatioTier::Fail),
            "exactly fail"
        );
    }

    /// A handful of references is not a measurement.
    #[test]
    fn small_samples_do_not_fire() {
        let v = RatioVerdict {
            missing: 10,
            total: 10,
        };
        assert_eq!(classify(&v, 20, 0.25, 0.80), None);
    }

    #[test]
    fn basename_ignores_directory_and_case() {
        assert_eq!(asset_basename("ASSETS/Characters/X/Foo.DDS"), "foo.dds");
        assert_eq!(asset_basename(r"assets\x\bar.tex"), "bar.tex");
    }

    #[test]
    fn canonical_strips_a_repath_prefix() {
        assert_eq!(
            canonical("ASSETS/bum/Characters/Jhin/x.dds").as_deref(),
            Some("ASSETS/Characters/Jhin/x.dds")
        );
        assert!(canonical("ASSETS/Maps/x.dds").is_none());
    }

    #[test]
    fn a_resolving_reference_is_not_missing() {
        let refs = ["assets/characters/x/a.dds", "assets/characters/x/b.dds"];
        let v = measure(refs.into_iter(), |p| p.ends_with("a.dds"));
        assert_eq!((v.missing, v.total), (1, 2));
    }

    /// The client loads the converted form, so shipping the `.tex` keeps the `.dds`
    /// reference alive.
    #[test]
    fn a_dds_reference_is_satisfied_by_its_tex_twin() {
        let refs = ["assets/characters/x/a.dds"];
        let v = measure(refs.into_iter(), |p| p.ends_with("a.tex"));
        assert_eq!(v.missing, 0);
    }

    /// A repathed reference names a file that exists at the stock path.
    #[test]
    fn a_repathed_reference_resolves_canonically() {
        let refs = ["ASSETS/bum/Characters/Jhin/a.dds"];
        let v = measure(refs.into_iter(), |p| {
            p == "ASSETS/Characters/Jhin/a.dds"
        });
        assert_eq!(v.missing, 0);
    }

    /// Repathing writes the same art under several paths; counting paths would inflate
    /// both halves and let a repath change the verdict.
    #[test]
    fn the_same_basename_counts_once() {
        let refs = [
            "assets/characters/x/a.dds",
            "assets/other/a.dds",
            "assets/third/a.dds",
        ];
        let v = measure(refs.into_iter(), |_| false);
        assert_eq!((v.missing, v.total), (1, 1));
    }

    /// One resolving path is enough, whichever order the references arrive in.
    #[test]
    fn a_basename_that_resolves_anywhere_is_not_missing() {
        let refs = ["assets/dead/a.dds", "assets/characters/x/a.dds"];
        let v = measure(refs.into_iter(), |p| p.starts_with("assets/characters/"));
        assert_eq!(v.missing, 0, "the second reference resolves");
    }
}
