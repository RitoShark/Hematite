//! Checking a mod, for an application embedding Hematite.
//!
//! The CLI reaches the check through its own argument handling, its own progress bars and
//! its own config plumbing. An application wants none of that: it wants to hand over a
//! folder and get back what is wrong with it. That is this module.
//!
//! ## Built once, used many times
//! Everything expensive here is shared setup: the hash dictionary, the game index, and the
//! shader definition set read out of the install. A launcher checks a whole library, and
//! rebuilding that per mod is most of the runtime. [`ModChecker`] holds it, so the second
//! mod costs only the mod.
//!
//! ## Fails open, deliberately
//! No game install, no hash dictionary, no network for the config: each of those loses some
//! checks and keeps the rest. A launcher that refuses to import a mod because it could not
//! reach GitHub is worse than one that imports it having checked less, and the report says
//! which checks did not run rather than implying everything passed.

use crate::live_provider::LiveGameProvider;
use crate::options::FixOptions;
use crate::progress::NoopSink;
use anyhow::{Context, Result};
use hematite_core::traits::HashProvider;
use hematite_types::champion::CharacterRelations;
use hematite_types::config::FixConfig;
use hematite_types::diagnostic::{CheckReport, SkipReason};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A mod checker with its shared data already loaded.
///
/// Construct once and keep it. Safe to share across threads: the game index is behind a
/// lock and everything else is immutable.
pub struct ModChecker {
    config: FixConfig,
    champions: CharacterRelations,
    hashes: Arc<dyn HashProvider>,
    live: Option<LiveGameProvider>,
    enabled_fixes: Vec<String>,
}

impl ModChecker {
    /// Load the config, the hash dictionary and the installed game.
    ///
    /// `game_path` overrides install detection; pass `None` to detect. A missing install is
    /// not an error, it is a smaller check.
    pub fn new(game_path: Option<&Path>) -> Result<Self> {
        let config = crate::remote::load_fix_config();
        let champions =
            CharacterRelations::from_champion_list(&crate::remote::load_champion_list());
        // Every rule the config turns on, which `is_fix_enabled` documents as the single
        // authority. Deriving it beats carrying a second hand-written list: the CLI's copy
        // had drifted two entries behind, so `stale_character_record` was enabled in config
        // and never actually ran.
        let enabled_fixes: Vec<String> = config
            .fixes
            .keys()
            .chain(config.wad_fixes.keys())
            .filter(|id| config.is_fix_enabled(id))
            .cloned()
            .collect();

        let hashes: Arc<dyn HashProvider> =
            Arc::new(hematite_file::lmdb_hash_adapter::LmdbHashProvider::load_from_appdata().context(
                "The hash dictionary is not installed. Download it before checking mods.",
            )?);

        let install = match game_path {
            Some(p) => hematite_live::LeagueInstall::from_path(p).ok(),
            None => hematite_live::detect_league(),
        };
        if install.is_none() {
            tracing::warn!("no League install found; game-dependent checks will be skipped");
        }
        let live = install.map(|i| {
            LiveGameProvider::new(
                hematite_live::GameIndex::new(&i),
                Box::new(hematite_file::bin_adapter::FileBinProvider::new()),
            )
        });

        Ok(Self {
            config,
            champions,
            hashes,
            live,
            enabled_fixes,
        })
    }

    /// The reason catalog behind the findings, for rendering them.
    pub fn reasons(&self) -> &hematite_types::diagnostic::ReasonCatalog {
        &self.config.reasons
    }

    /// Whether a game install was found. Without one, several checks cannot run.
    pub fn has_game(&self) -> bool {
        self.live.is_some()
    }

    /// Check one mod, writing nothing.
    ///
    /// `path` is either a single `.wad.client` folder or a directory holding several. A mod
    /// that replaces both a champion and an interface file ships two archives, and both have
    /// to be checked or the second one's defects are invisible.
    pub fn check(&self, path: &Path) -> Result<CheckReport> {
        let folders = wad_folders(path)?;
        if folders.is_empty() {
            anyhow::bail!(
                "{} holds no .wad.client folder to check",
                path.display()
            );
        }

        let mut report = CheckReport::default();
        let mut failures = Vec::new();
        for folder in &folders {
            match self.check_one(folder) {
                Ok(one) => report.merge(one),
                // One unreadable archive must not hide what the others say.
                Err(e) => {
                    tracing::warn!("check failed for {}: {e:#}", folder.display());
                    failures.push(format!("{}: {e:#}", folder.display()));
                }
            }
        }

        if failures.len() == folders.len() {
            anyhow::bail!("every archive failed to check: {}", failures.join("; "));
        }
        for failure in failures {
            report.mark_skipped("archive", SkipReason::Failed(failure));
        }

        report.dedupe();
        report.attach_catalog(&self.config.reasons);
        Ok(report)
    }

    fn check_one(&self, folder: &Path) -> Result<CheckReport> {
        let opts = FixOptions {
            dry_run: true,
            detect_only: true,
            repath: None,
            restore_anm: false,
            relocate_combo_bins: false,
            game_wad: None,
            live: self.live.as_ref(),
            in_place: false,
        };
        let result = crate::fix_folder(
            folder,
            &self.config,
            &self.enabled_fixes,
            &self.champions,
            &self.hashes,
            &opts,
            &NoopSink,
        )?;
        Ok(result.report)
    }
}

/// The `.wad.client` folders under `path`, or `path` itself when it is one.
///
/// Two layouts, because both turn up. A mod extracted on its own is a folder of
/// `.wad.client` directories; a fantome unpacked keeps its own shape, `META/info.json`
/// beside a `WAD/` holding the archives. Looking only at the top level finds nothing in the
/// second, and a mod that reports no archives reads as a mod with nothing wrong with it.
///
/// Never recursive past those two levels. A WAD folder contains its own directory tree, and
/// descending into it would treat `assets/` as another archive.
fn wad_folders(path: &Path) -> Result<Vec<PathBuf>> {
    if !path.is_dir() {
        anyhow::bail!("{} is not a folder", path.display());
    }
    if is_wad_folder(path) {
        return Ok(vec![path.to_path_buf()]);
    }

    let mut out = Vec::new();
    collect_wad_folders(path, &mut out)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    // The fantome convention. Case-insensitive because the directory is written by whatever
    // packed the mod.
    if let Some(wad_dir) = child_named(path, "wad") {
        let _ = collect_wad_folders(&wad_dir, &mut out);
    }

    out.sort();
    out.dedup();
    Ok(out)
}

fn collect_wad_folders(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)?.flatten() {
        let child = entry.path();
        if child.is_dir() && is_wad_folder(&child) {
            out.push(child);
        }
    }
    Ok(())
}

/// A subdirectory with this name, matched without regard to case.
fn child_named(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
}

fn is_wad_folder(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.to_lowercase().ends_with(".wad.client"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(root: &Path, names: &[&str]) {
        for name in names {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
    }

    #[test]
    fn a_folder_of_archives_yields_each_one() {
        let dir = tempfile::tempdir().unwrap();
        make(
            dir.path(),
            &["Jhin.wad.client", "Global.wad.client", "META", "notes"],
        );
        let found = wad_folders(dir.path()).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|p| is_wad_folder(p)));
    }

    /// Handed one archive directly, that is the whole answer.
    #[test]
    fn a_single_archive_is_itself() {
        let dir = tempfile::tempdir().unwrap();
        let wad = dir.path().join("Jhin.wad.client");
        std::fs::create_dir_all(wad.join("data/characters")).unwrap();
        assert_eq!(wad_folders(&wad).unwrap(), vec![wad]);
    }

    /// A WAD folder's own subdirectories are its content, not more archives.
    #[test]
    fn the_search_does_not_descend_into_an_archive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("A.wad.client/nested.wad.client")).unwrap();
        let found = wad_folders(dir.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("A.wad.client"));
    }

    /// An unpacked fantome keeps `META/` beside a `WAD/` holding the archives. Missing this
    /// reported such a mod as having nothing to check, which reads as a clean mod.
    #[test]
    fn an_unpacked_fantome_is_found_through_its_wad_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("META")).unwrap();
        std::fs::create_dir_all(dir.path().join("WAD/UI.wad.client/ASSETS")).unwrap();
        std::fs::create_dir_all(dir.path().join("WAD/Global.wad.client")).unwrap();

        let found = wad_folders(dir.path()).unwrap();
        assert_eq!(found.len(), 2, "both archives under WAD/");
    }

    /// Some packers write `wad`, some `WAD`.
    #[test]
    fn the_wad_folder_name_is_matched_without_case() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("wad/UI.wad.client")).unwrap();
        assert_eq!(wad_folders(dir.path()).unwrap().len(), 1);
    }

    /// A mod with archives in both places yields each exactly once.
    #[test]
    fn both_layouts_at_once_do_not_double_count() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Jhin.wad.client")).unwrap();
        std::fs::create_dir_all(dir.path().join("WAD/UI.wad.client")).unwrap();

        let found = wad_folders(dir.path()).unwrap();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn a_folder_with_no_archive_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        make(dir.path(), &["META", "assets"]);
        assert!(wad_folders(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn a_file_is_not_a_mod_folder() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mod.fantome");
        std::fs::write(&file, b"").unwrap();
        assert!(wad_folders(&file).is_err());
    }

    #[test]
    fn the_extension_match_ignores_case() {
        let dir = tempfile::tempdir().unwrap();
        make(dir.path(), &["Jhin.WAD.CLIENT"]);
        assert_eq!(wad_folders(dir.path()).unwrap().len(), 1);
    }
}
