//! Library-neutral options for [`crate::fix_folder`].
//!
//! Replaces the CLI's loose `ProcessContext` fields with a plain struct an
//! embedder can populate directly.

use std::path::Path;

/// Session-level options for a folder fix run.
///
/// ## `live` boundary
///
/// `live` is the concrete [`crate::live_provider::LiveGameProvider`] rather
/// than `&dyn GameFileAccess`, because the recovery passes (deep-repair,
/// restore-anm, combo-bin relocation) call its inherent `with_index(...)`
/// method — a bulk hash-snapshot / raw-pull surface that `GameProvider`
/// (`GameFileAccess`) does not expose. `LiveGameProvider` itself implements
/// `GameProvider`, so `fix_folder` still hands it to the BIN engine as
/// `&dyn GameProvider` via `FixContext.game`.
pub struct FixOptions<'a> {
    /// Detect + report, but write nothing (mirrors the CLI's `--dry-run`).
    pub dry_run: bool,
    /// Populate `CheckInfo` and record every fired fix as an `AppliedFix`
    /// with its detection count, but touch NOTHING on disk (no WAD rebuild,
    /// no file writes/removes). Strictly stronger than `dry_run`.
    pub detect_only: bool,
    /// Repath options, when repathing is active.
    pub repath: Option<&'a hematite_types::repath::RepathOptions>,
    /// Whether `--restore-anm` is active.
    pub restore_anm: bool,
    /// Whether combo-bin relocation is active.
    pub relocate_combo_bins: bool,
    /// Explicit `--game-wad`, threaded independently of `repath` so
    /// restore-anm / combo relocation can use it as a game source even
    /// when repathing is off.
    pub game_wad: Option<&'a Path>,
    /// Auto-detected (or `--game-path`) live game access. `None` when no
    /// install was found or live access was disabled — every live-game
    /// feature fails open in that case.
    pub live: Option<&'a crate::live_provider::LiveGameProvider>,
    /// Pull missing referenced assets out of the installed game.
    ///
    /// Makes the mod self-contained: every `.anm`, `.skn`, `.skl`, texture and linked BIN
    /// the mod names but does not ship is fetched from the install, following the
    /// dependency closure until nothing new appears.
    ///
    /// Independent of `repath`. The pull used to be reachable only from inside the repath
    /// pipeline, which meant a caller that did not want every path rewritten could detect a
    /// missing animation and not fix it. Repathing is a separate decision about what the
    /// mod's paths look like; this is about whether its assets are there at all.
    ///
    /// When repathing IS on, the repath pipeline runs the pull itself and this is ignored,
    /// so the assets arrive before the paths are rewritten rather than after.
    pub pull_missing: bool,
    /// Write the fixed files back INTO the source folder instead of a sibling
    /// `<folder>.fixed.wad.client` copy. Overwrites changed files and deletes
    /// originals that were renamed away or removed. Used by embedders (Flint)
    /// that fix a project in place; the CLI leaves this `false` so it keeps
    /// producing the non-destructive `.fixed` copy.
    pub in_place: bool,
}
