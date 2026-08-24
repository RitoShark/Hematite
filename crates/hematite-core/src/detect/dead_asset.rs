//! Dead asset references: a path the BIN names that exists nowhere.
//!
//! An asset reference resolves against two sources at load time: the files the mod
//! ships, and the files the base game ships. A path in neither resolves to nothing, and
//! what happens next depends on the asset. A missing animation clip or mesh is fatal,
//! because the engine dereferences the result without checking.
//!
//! ## Why this is not the existing extension rule
//! `RecursiveStringExtensionNotInWad` asks only whether the MOD ships the file. That is
//! the right question for a conversion fix (`.dds` to `.tex` rewrites what the mod
//! carries), but the wrong one for a crash check: most of a mod's references point at
//! base-game assets it deliberately does not duplicate. On one fixture, 103 animation
//! references resolved to 39 shipped by the game and 64 dead; asking the mod alone would
//! have reported all 103 and buried the real 64.
//!
//! ## Independent of the type migration
//! Retyping a path string to a hashed reference changes how the engine looks the asset
//! up, not whether it is there. A mod can be fully migrated and still crash on a missing
//! clip, so this check and the migration check both have to run.
//!
//! ## Fail-open
//! Without a `GameProvider` there is no way to tell "the game ships it" from "nothing
//! ships it", so nothing is reported.

use crate::context::FixContext;
use crate::walk::string_refs;

/// Re-anchor a repathed asset path on its `characters/` segment.
///
/// A repathed mod rewrites `ASSETS/Characters/...` to `ASSETS/<prefix>/Characters/...`
/// and ships only the assets it actually replaces. Every untouched reference still
/// carries the prefix while the file it names lives at the stock path, so comparing the
/// literal string finds nothing. On one fixture this accounted for 62 of 64 apparent
/// dead references: without this, the check reports a broken mod that is fine.
///
/// Returns `None` when the path has no `characters/` segment to anchor on.
fn canonical(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let idx = normalized.to_lowercase().find("characters/")?;
    Some(format!("ASSETS/{}", &normalized[idx..]))
}

/// Asset paths referenced by the BIN that exist in neither the mod nor the game.
///
/// `extensions` are matched case-insensitively and should include the dot.
/// `path_prefixes`, when non-empty, restricts matching to paths starting with one of
/// them, so a rule can target character assets without touching map or interface ones.
pub fn dead_refs(
    ctx: &FixContext,
    extensions: &[String],
    path_prefixes: &[String],
    suppress_never_loaded: bool,
) -> Vec<String> {
    let Some(game) = ctx.game else {
        return Vec::new();
    };

    let exts: Vec<String> = extensions.iter().map(|e| e.to_lowercase()).collect();
    let prefixes: Vec<String> = path_prefixes.iter().map(|p| p.to_lowercase()).collect();

    // Clips that are dead on paper but that the engine never asks for. Only animations
    // can be in this set, so it is built only for a rule that looks at them: the walk is
    // not free and every other rule would get an empty set for the money.
    let never_loaded = if suppress_never_loaded && exts.iter().any(|e| e == ".anm") {
        crate::detect::anm_scope::never_loaded(&ctx.tree)
    } else {
        std::collections::HashSet::new()
    };

    let mut dead = Vec::new();
    for s in string_refs(&ctx.tree) {
        let lower = s.to_lowercase();
        if !exts.iter().any(|e| lower.ends_with(e.as_str())) {
            continue;
        }
        if !prefixes.is_empty() && !prefixes.iter().any(|p| lower.starts_with(p.as_str())) {
            continue;
        }
        if ctx.wad.has_path(s) || game.has_path(s) {
            continue;
        }
        // A repathed reference names a file that exists at its stock path. Checking only
        // the literal string reports the whole mod as broken.
        if let Some(canon) = canonical(s) {
            if ctx.wad.has_path(&canon) || game.has_path(&canon) {
                continue;
            }
        }
        // The clip is genuinely absent, but nothing ever asks the engine for it.
        if !never_loaded.is_empty()
            && never_loaded.contains(&crate::detect::anm_scope::normalize(s))
        {
            tracing::debug!("dead but never loaded, not reported: {}", s);
            continue;
        }
        dead.push(s.to_string());
    }

    dead.sort();
    dead.dedup();
    dead
}

/// Boolean verdict for the detection dispatch.
pub fn detect(
    ctx: &FixContext,
    extensions: &[String],
    path_prefixes: &[String],
    suppress_never_loaded: bool,
) -> bool {
    !dead_refs(ctx, extensions, path_prefixes, suppress_never_loaded).is_empty()
}

/// Whether a dead reference in THIS BIN is reached as the mod is used.
///
/// A skin BIN the mod ships is loaded whenever that skin is selected, so a dead
/// reference in it is certain. An animation BIN that no shipped skin links is not loaded
/// at all as the mod is used: the clip is dead only for someone selecting the original
/// skin it belongs to, with the mod active. That is a real defect but a conditional one,
/// and calling it a crash overstates what the player will actually hit.
///
/// Unknown load information means treat it as reached, so a defect is never quietly
/// downgraded on the strength of something we could not determine.
pub fn is_reached_in_use(ctx: &FixContext) -> bool {
    if ctx.scope.reachable.is_none() {
        return true;
    }
    // A skin root is loaded with its skin.
    if ctx.scope.skin_slot().is_some() {
        return true;
    }
    // An animation BIN only counts when something loaded links it.
    if ctx.scope.is_animation_bin() {
        return ctx
            .scope
            .reachable
            .is_some_and(|r| r.loads(ctx.scope.chunk_hash));
    }
    // Anything else that survived the reachability gate is loaded.
    true
}

/// The character an asset path belongs to, e.g. `sru_chaosminionmelee`.
pub fn character_of_asset(path: &str) -> Option<&str> {
    let normalized = path.as_bytes();
    let lower = path.to_lowercase();
    let idx = lower.find("characters/")? + "characters/".len();
    let rest = &path[idx..];
    let _ = normalized;
    let name = rest.split(['/', '\\']).next()?;
    (!name.is_empty()).then_some(name)
}

/// Whether an asset belongs to a map character rather than a playable one.
///
/// Minions, turrets, structures and shopkeepers are loaded only by the map variant that
/// uses them, so a missing mesh there is conditional rather than certain: the game may
/// never load that variant at all. Reporting it as a guaranteed crash on a map mod turns
/// one themed rift into dozens of crash findings.
///
/// Markers come from config so the list can be corrected without a rebuild. Note what is
/// deliberately NOT a marker: `inhibitor` and `nexus` are always loaded, so a missing
/// mesh on those stays a crash.
pub fn is_map_character(path: &str, markers: &[String]) -> bool {
    let Some(name) = character_of_asset(path) else {
        // No character segment at all, so this is world or particle geometry rather than
        // anyone's body. A rule whose crash reason is "the champion body cannot build"
        // is simply not describing this asset, so it must not claim it.
        return true;
    };
    let name = name.to_lowercase();
    markers.iter().any(|m| name.contains(&m.to_lowercase()))
}

#[cfg(test)]
mod tests {
    /// Extension and prefix filtering, exercised without a full `FixContext`.
    fn selects(path: &str, exts: &[&str], prefixes: &[&str]) -> bool {
        let lower = path.to_lowercase();
        let ext_ok = exts.iter().any(|e| lower.ends_with(&e.to_lowercase()));
        let pre_ok =
            prefixes.is_empty() || prefixes.iter().any(|p| lower.starts_with(&p.to_lowercase()));
        ext_ok && pre_ok
    }

    #[test]
    fn matches_extension_case_insensitively() {
        assert!(selects("ASSETS/Characters/Jhin/x.ANM", &[".anm"], &[]));
        assert!(selects("assets/characters/jhin/x.anm", &[".anm"], &[]));
        assert!(!selects("assets/characters/jhin/x.tex", &[".anm"], &[]));
    }

    #[test]
    fn matches_any_of_several_extensions() {
        assert!(selects("a/b.skn", &[".skn", ".skl"], &[]));
        assert!(selects("a/b.skl", &[".skn", ".skl"], &[]));
        assert!(!selects("a/b.scb", &[".skn", ".skl"], &[]));
    }

    #[test]
    fn prefix_filter_restricts_scope() {
        assert!(selects(
            "assets/characters/jhin/x.anm",
            &[".anm"],
            &["assets/characters/"]
        ));
        assert!(!selects(
            "assets/maps/shipping/x.anm",
            &[".anm"],
            &["assets/characters/"]
        ));
    }

    /// A repathed mod invents prefixes of its own, so a prefix filter tied to the stock
    /// layout would silently skip exactly the paths most likely to be dead.
    #[test]
    fn empty_prefix_list_matches_repathed_paths() {
        assert!(selects(
            "ASSETS/bum/Characters/Jhin/Skins/Skin05/Animations/x.anm",
            &[".anm"],
            &[]
        ));
    }

    /// The regression that mattered most: on one fixture, 62 of 64 apparent dead
    /// references were repathed copies of files that exist at their stock path.
    #[test]
    fn canonical_strips_a_repath_prefix() {
        assert_eq!(
            super::canonical("ASSETS/bum/Characters/Jhin/Skins/Skin05/Animations/x.anm").as_deref(),
            Some("ASSETS/Characters/Jhin/Skins/Skin05/Animations/x.anm")
        );
    }

    /// An already-stock path must canonicalise to itself, not to something new.
    #[test]
    fn canonical_is_idempotent_on_stock_paths() {
        let stock = "ASSETS/Characters/Jhin/Skins/Skin05/Animations/x.anm";
        assert_eq!(super::canonical(stock).as_deref(), Some(stock));
    }

    #[test]
    fn canonical_normalises_backslashes_and_matches_case_insensitively() {
        assert_eq!(
            super::canonical(r"assets\rep\CHARACTERS\Jhin\x.anm").as_deref(),
            Some("ASSETS/CHARACTERS/Jhin/x.anm")
        );
    }

    /// Anchoring needs a `characters/` segment; a map or interface path has none and
    /// must not be rewritten into something that accidentally resolves.
    #[test]
    fn canonical_declines_paths_with_no_character_segment() {
        assert!(super::canonical("ASSETS/Maps/Shipping/x.anm").is_none());
    }

    /// Inner-suffix renames are NOT rescued: the engine resolves by literal path, so a
    /// clip Riot renamed is genuinely dead and must still be reported.
    #[test]
    fn canonical_does_not_strip_an_inner_suffix() {
        let renamed = "ASSETS/Characters/Jhin/Skins/Skin55/Animations/Recall.SKINS_Jhin_Skin55.anm";
        assert_eq!(super::canonical(renamed).as_deref(), Some(renamed));
    }
}
