//! How much of a champion's ability VFX actually exists.
//!
//! A skin whose abilities render nothing is not a crash, but it cannot be played: you
//! cannot aim what you cannot see. That is a share question rather than a single missing
//! file, and the share only means something once the effects are classified, because most
//! of what a champion references is not gameplay.
//!
//! ## Only gameplay effects count
//! A champion's particle set is mostly recall animations, idle ambience, level-up flashes
//! and ground decals. Counting all of it would put a fully broken skin around a third dead
//! and a healthy one nowhere near zero, so the ratio would say nothing. Only the effects
//! you aim or react to count toward the denominator.
//!
//! Three exclusions matter and are not interchangeable:
//! - **Legacy game modes** whose effects no live mode renders. Always absent, never a
//!   defect.
//! - **Audio cues**, which are not visual at all.
//! - **Another champion's particles**, left over from whatever the mod was built from.
//!   Never the mod's own effect.
//!
//! ## The missile override
//! One dead missile warns on its own, whatever the percentage. The projectile is the thing
//! the enemy reacts to, so losing it changes how the game plays even when everything else
//! renders. This is narrower than it first looks: a dead cast, target zone or aim indicator
//! does NOT trip the override, because a single one of those over-flagged skins that play
//! perfectly well.

use std::collections::HashSet;

/// What a referenced effect is, for the purpose of the ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A gameplay effect: counted.
    Core,
    /// Real but not gameplay: not counted.
    Cosmetic,
    /// Not the mod's effect, or not visual at all: not counted.
    Ignore,
}

/// Marker lists that decide how an effect is classified.
///
/// Held as data so the lists can be corrected without a rebuild; Riot adds game modes and
/// naming conventions faster than releases happen.
#[derive(Debug, Clone)]
pub struct VfxMarkers<'a> {
    /// Game modes no live queue renders.
    pub legacy: &'a [String],
    /// Audio cues, which are not visual.
    pub audio: &'a [String],
    /// Non-gameplay effects: recalls, idles, deaths, emotes.
    pub cosmetic: &'a [String],
    /// Helper overlays: markers, timers, range rings.
    pub subhelper: &'a [String],
}

/// Ability slots a gameplay effect can belong to.
const ABILITY_SLOTS: &[&str] = &["q", "w", "e", "r", "p", "ba", "passive", "basicattack"];

/// Whether a leaf names an ability slot.
///
/// Read from the leaf's third underscore-separated segment onwards, because the first two
/// are the champion and skin. Without that, a champion whose name contains a slot letter
/// would match everything.
pub fn is_ability_effect(leaf: &str) -> bool {
    let leaf = leaf.to_ascii_lowercase();
    let tail = if leaf.matches('_').count() >= 2 {
        let mut it = leaf.splitn(3, '_');
        it.next();
        it.next();
        format!("_{}", it.next().unwrap_or(""))
    } else {
        leaf.clone()
    };

    let bytes = tail.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b != b'_' {
            continue;
        }
        let rest = &tail[i + 1..];
        for slot in ABILITY_SLOTS {
            if let Some(after) = rest.strip_prefix(slot) {
                // The slot has to end here, so `_q` and `_q_cas` match but `_quinn` does not.
                if after.is_empty()
                    || after.starts_with('_')
                    || after.starts_with(|c: char| c.is_ascii_digit())
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Whether a leaf is the projectile.
///
/// Deliberately just the missile. See the module docs: widening this to casts, zones and
/// indicators over-flagged skins that play fine.
pub fn is_projectile(leaf: &str) -> bool {
    leaf.to_ascii_lowercase().contains("_mis")
}

/// Classify one referenced effect.
pub fn classify(name: &str, champion: &str, markers: &VfxMarkers<'_>) -> Kind {
    let lower = name.to_ascii_lowercase();

    if markers.legacy.iter().any(|m| lower.contains(m.as_str()))
        || markers.audio.iter().any(|m| lower.contains(m.as_str()))
    {
        return Kind::Ignore;
    }

    // Another champion's particle is a leftover from whatever the mod was built from.
    if let Some(i) = lower.find("characters/") {
        let owner = lower[i + "characters/".len()..]
            .split('/')
            .next()
            .unwrap_or("");
        if !owner.is_empty() && owner != champion {
            return Kind::Ignore;
        }
    }

    let leaf = lower.rsplit('/').next().unwrap_or(&lower);
    if markers.cosmetic.iter().any(|m| leaf.contains(m.as_str()))
        || markers.subhelper.iter().any(|m| leaf.contains(m.as_str()))
    {
        return Kind::Cosmetic;
    }

    if is_ability_effect(leaf) && is_projectile(leaf) {
        Kind::Core
    } else {
        Kind::Cosmetic
    }
}

/// What the measurement found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VfxVerdict {
    /// Gameplay effects referenced.
    pub core_total: usize,
    /// Of those, how many are defined nowhere.
    pub core_dead: usize,
    /// Whether any dead effect is a projectile.
    pub projectile_dead: bool,
}

impl VfxVerdict {
    pub fn share(&self) -> f32 {
        if self.core_total == 0 {
            return 0.0;
        }
        self.core_dead as f32 / self.core_total as f32
    }

    pub fn percent(&self) -> u32 {
        (self.share() * 100.0) as u32
    }

    pub fn describe(&self) -> String {
        format!(
            "{}/{} ({}%) of ability effects",
            self.core_dead,
            self.core_total,
            self.percent()
        )
    }
}

/// Count referenced gameplay effects and how many are missing.
///
/// `referenced` is every effect name a BIN points at; `is_defined` answers whether the
/// effect exists. Only names naming a particle are considered at all.
pub fn measure<'a>(
    referenced: impl Iterator<Item = &'a str>,
    champion: &str,
    markers: &VfxMarkers<'_>,
    is_defined: impl Fn(&str) -> bool,
) -> VfxVerdict {
    let mut core: HashSet<String> = HashSet::new();
    let mut dead: HashSet<String> = HashSet::new();
    let mut projectile_dead = false;

    for name in referenced {
        let lower = name.to_ascii_lowercase();
        if !lower.contains("/particles/") {
            continue;
        }
        if classify(name, champion, markers) != Kind::Core {
            continue;
        }
        core.insert(lower.clone());
        if !is_defined(name) {
            let leaf = lower.rsplit('/').next().unwrap_or(&lower).to_string();
            if is_projectile(&leaf) {
                projectile_dead = true;
            }
            dead.insert(lower);
        }
    }

    VfxVerdict {
        core_total: core.len(),
        core_dead: dead.len(),
        projectile_dead,
    }
}

/// Which tier a measurement lands in, if any.
///
/// `warn_at` and `fail_at` are both INCLUSIVE here, unlike the asset ratio where warn is
/// exclusive. Carried over deliberately from the tuning these came from.
pub fn tier(verdict: &VfxVerdict, warn_at: f32, fail_at: f32) -> Option<super::asset_ratio::RatioTier> {
    use super::asset_ratio::RatioTier;
    if verdict.core_total == 0 {
        return None;
    }
    let share = verdict.share();
    if share >= fail_at {
        return Some(RatioTier::Fail);
    }
    // A dead projectile warns on its own: it changes how the game plays even when
    // everything else renders.
    if share >= warn_at || verdict.projectile_dead {
        return Some(RatioTier::Warn);
    }
    None
}


/// Run the ability-VFX measurement over one mod.
///
/// `champion` is whose effects these are; another champion's particles are leftovers and
/// do not count. Returns at most one diagnostic.
pub fn run(
    cfg: &hematite_types::config::VfxRatioConfig,
    catalog: &hematite_types::diagnostic::ReasonCatalog,
    references: &[String],
    champion: &str,
    is_defined: impl Fn(&str) -> bool,
) -> Option<hematite_types::diagnostic::Diagnostic> {
    if !cfg.enabled || champion.is_empty() {
        return None;
    }
    let markers = VfxMarkers {
        legacy: &cfg.legacy_markers,
        audio: &cfg.audio_markers,
        cosmetic: &cfg.cosmetic_markers,
        subhelper: &cfg.subhelper_markers,
    };

    let verdict = measure(
        references.iter().map(String::as_str),
        champion,
        &markers,
        is_defined,
    );
    let t = tier(&verdict, cfg.warn_at, cfg.fail_at)?;
    let reason = match t {
        super::asset_ratio::RatioTier::Fail => cfg.fail_reason.as_deref(),
        super::asset_ratio::RatioTier::Warn => cfg.warn_reason.as_deref(),
    }?;

    let detail = if verdict.projectile_dead && t == super::asset_ratio::RatioTier::Warn {
        format!("{}, including a missile", verdict.describe())
    } else {
        verdict.describe()
    };
    tracing::info!("ability VFX: {} ({})", detail, reason);
    Some(
        hematite_types::diagnostic::Diagnostic::new(catalog, reason, "vfx_ratio")
            .with_detail(detail),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn markers() -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        (
            s(&["doombot", "ascension", "_urf_"]),
            s(&["audio", "sound", "_sfx"]),
            s(&["recall", "idle", "death", "emote"]),
            s(&["_marker", "_timer", "_ring"]),
        )
    }

    fn m<'a>(v: &'a (Vec<String>, Vec<String>, Vec<String>, Vec<String>)) -> VfxMarkers<'a> {
        VfxMarkers {
            legacy: &v.0,
            audio: &v.1,
            cosmetic: &v.2,
            subhelper: &v.3,
        }
    }

    #[test]
    fn ability_slots_are_read_past_champion_and_skin() {
        assert!(is_ability_effect("jhin_skin55_q_mis"));
        assert!(is_ability_effect("jhin_skin55_r_cas"));
        assert!(is_ability_effect("ahri_base_passive_tar"));
    }

    /// The slot has to end at a boundary, or a champion whose name merely contains a slot
    /// letter would match everything.
    #[test]
    fn a_slot_letter_inside_a_word_is_not_a_slot() {
        assert!(!is_ability_effect("some_thing_quinn_glow"));
        assert!(!is_ability_effect("some_thing_ratchet"));
    }

    #[test]
    fn only_the_missile_is_a_projectile() {
        assert!(is_projectile("jhin_skin55_q_mis"));
        assert!(!is_projectile("jhin_skin55_q_cas"));
        assert!(!is_projectile("jhin_skin55_q_indicator"));
    }

    #[test]
    fn legacy_modes_and_audio_are_ignored() {
        let v = markers();
        assert_eq!(classify("x/doombot_q_mis", "jhin", &m(&v)), Kind::Ignore);
        assert_eq!(classify("x/jhin_base_q_mis_audio", "jhin", &m(&v)), Kind::Ignore);
    }

    /// Another champion's particle is a leftover from whatever the mod was cloned from.
    #[test]
    fn another_champions_particle_is_ignored() {
        let v = markers();
        assert_eq!(
            classify("assets/characters/lux/particles/lux_base_q_mis", "jhin", &m(&v)),
            Kind::Ignore
        );
        assert_eq!(
            classify("assets/characters/jhin/particles/jhin_base_q_mis", "jhin", &m(&v)),
            Kind::Core
        );
    }

    #[test]
    fn cosmetic_effects_do_not_count() {
        let v = markers();
        assert_eq!(classify("x/jhin_base_recall_mis", "jhin", &m(&v)), Kind::Cosmetic);
        assert_eq!(classify("x/jhin_base_q_marker", "jhin", &m(&v)), Kind::Cosmetic);
    }

    /// A ground decal on an ability is still not something you aim at.
    #[test]
    fn a_non_projectile_ability_effect_is_cosmetic() {
        let v = markers();
        assert_eq!(
            classify("x/particles/jhin_base_q_dagger_land_dirt", "jhin", &m(&v)),
            Kind::Cosmetic
        );
    }

    #[test]
    fn measure_counts_only_gameplay_particles() {
        let v = markers();
        let refs = [
            "assets/characters/jhin/particles/jhin_base_q_mis",
            "assets/characters/jhin/particles/jhin_base_w_mis",
            "assets/characters/jhin/particles/jhin_base_recall_mis",
            "assets/characters/jhin/skins/base/jhin.dds",
        ];
        let verdict = measure(refs.into_iter(), "jhin", &m(&v), |_| false);
        assert_eq!(verdict.core_total, 2, "recall and the texture do not count");
        assert_eq!(verdict.core_dead, 2);
    }

    /// One dead projectile warns even when the share is far below the threshold.
    #[test]
    fn a_dead_projectile_warns_on_its_own() {
        use super::super::asset_ratio::RatioTier;
        let verdict = VfxVerdict {
            core_total: 100,
            core_dead: 1,
            projectile_dead: true,
        };
        assert_eq!(tier(&verdict, 0.30, 0.80), Some(RatioTier::Warn));
    }

    /// Both bounds are inclusive here, unlike the asset ratio.
    #[test]
    fn both_thresholds_are_inclusive() {
        use super::super::asset_ratio::RatioTier;
        let at_warn = VfxVerdict {
            core_total: 100,
            core_dead: 30,
            projectile_dead: false,
        };
        assert_eq!(tier(&at_warn, 0.30, 0.80), Some(RatioTier::Warn));

        let at_fail = VfxVerdict {
            core_total: 100,
            core_dead: 80,
            projectile_dead: false,
        };
        assert_eq!(tier(&at_fail, 0.30, 0.80), Some(RatioTier::Fail));
    }

    #[test]
    fn nothing_referenced_means_no_verdict() {
        let verdict = VfxVerdict {
            core_total: 0,
            core_dead: 0,
            projectile_dead: false,
        };
        assert_eq!(tier(&verdict, 0.30, 0.80), None);
    }
}
