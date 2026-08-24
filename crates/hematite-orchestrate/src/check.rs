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
        let enabled_fixes = enabled_fixes_in_order(&config);

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

}

/// Every enabled rule, in the order the config declares them.
///
/// ## The order is load-bearing
/// The pipeline applies fixes in the order of the list it is handed, and some pairs only
/// work one way round. `staticmat_texturepath` moves a path out of `TextureName` into
/// `TexturePath`; `staticmat_samplername` then moves the sampler's name into the
/// `TextureName` it just vacated. Run the other way round, the sampler name lands in
/// `TextureName` first and the next rule promotes THAT into `TexturePath`, so the material
/// ends up pointing at "Diffuse_Texture" instead of at a texture. The real path is gone.
///
/// This used to be derived from `config.fixes.keys()`, which is a `HashMap`: arbitrary
/// order, randomised per process, so the pair ran the wrong way round some of the time and
/// silently destroyed materials when it did. `enabled_fixes` is a list precisely so the
/// author can say what runs when.
///
/// Rules enabled individually but absent from that list are appended, sorted, so a config
/// that never grew an `enabled_fixes` entry still runs them and still runs them the same
/// way twice.
fn enabled_fixes_in_order(config: &FixConfig) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    if let Some(declared) = &config.enabled_fixes {
        for id in declared {
            if seen.insert(id.as_str()) {
                out.push(id.clone());
            }
        }
    }

    let mut extra: Vec<String> = config
        .fixes
        .keys()
        .chain(config.wad_fixes.keys())
        .filter(|id| !seen.contains(id.as_str()) && config.is_fix_enabled(id))
        .cloned()
        .collect();
    extra.sort();
    out.extend(extra);
    out
}

impl ModChecker {
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
    /// `path` is a `.wad.client` archive, packed or unpacked, or a directory holding
    /// several. A mod that replaces both a champion and an interface file ships two
    /// archives, and both have to be checked or the second one's defects are invisible.
    ///
    /// Packed archives are unpacked to a scratch directory first, so both forms go through
    /// exactly one code path. Two paths would be two behaviours, and a launcher storing
    /// mods packed would slowly stop matching what the CLI reports for the same file.
    pub fn check(&self, path: &Path) -> Result<CheckReport> {
        let archives = mod_archives(path)?;
        if archives.is_empty() {
            anyhow::bail!("{} holds no .wad.client archive to check", path.display());
        }

        // Lives until the end of the check: dropping it removes the unpacked copies.
        let scratch = tempfile::Builder::new()
            .prefix("hematite-check-")
            .tempdir()
            .context("Failed to create a scratch directory")?;

        let mut report = CheckReport::default();
        let mut failures = Vec::new();
        for archive in &archives {
            match self.check_archive(archive, scratch.path()) {
                Ok(one) => report.merge(one),
                // One unreadable archive must not hide what the others say.
                Err(e) => {
                    tracing::warn!("check failed for {}: {e:#}", archive.display());
                    failures.push(format!("{}: {e:#}", archive.display()));
                }
            }
        }

        if failures.len() == archives.len() {
            anyhow::bail!("every archive failed to check: {}", failures.join("; "));
        }
        for failure in failures {
            report.mark_skipped("archive", SkipReason::Failed(failure));
        }

        report.dedupe();
        report.attach_catalog(&self.config.reasons);
        Ok(report)
    }

    /// Check one archive, unpacking it first when it is a packed file.
    fn check_archive(&self, archive: &Path, scratch: &Path) -> Result<CheckReport> {
        if archive.is_dir() {
            return self.check_one(archive);
        }
        let unpacked = self.unpack(archive, scratch)?;
        self.check_one(&unpacked)
    }

    /// Write a packed archive out as a WAD folder under `scratch`.
    ///
    /// Uses the same extractor and the same folder convention the CLI writes, including the
    /// hex fallback for chunks the dictionary cannot name, so the unpacked form is the one
    /// the checks were built against.
    fn unpack(&self, archive: &Path, scratch: &Path) -> Result<PathBuf> {
        use hematite_file::wad_adapter::WadFile;

        let name = archive
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("mod.wad.client");
        // Distinct per archive: two mods can both ship a `Global.wad.client`.
        let target = scratch.join(format!("{:016x}", hash_of(archive))).join(name);

        let mut wad = WadFile::open(archive)
            .with_context(|| format!("Failed to open {}", archive.display()))?;
        let files = wad
            .extract_all_files(self.hashes.as_ref())
            .with_context(|| format!("Failed to read {}", archive.display()))?;
        hematite_file::wad_folder::write_wad_folder(&target, &files, &[])
            .with_context(|| format!("Failed to unpack {}", archive.display()))?;
        Ok(target)
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
            pull_missing: false,
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

/// Repair a mod, writing the result back where it came from.
///
/// The counterpart to [`ModChecker::check`], sharing its setup. Everything the check knows
/// about a mod's defects, the fix pipeline can act on, so running them from one loaded
/// engine costs nothing extra.
///
/// ## Both forms, one code path
/// A packed archive is unpacked to a scratch folder, fixed there, and repacked over the
/// original. That is slower than editing in place would be, and it is the only way the two
/// forms cannot diverge: the fix pipeline has exactly one implementation, the folder one,
/// and a packed mod is a folder it has not been unpacked into yet.
///
/// ## Writes only on success
/// The repack goes to a temporary file beside the target and is renamed over it, so an
/// archive is never left half-written. A mod that fails mid-repair is the mod you started
/// with.
impl ModChecker {
    /// Apply every enabled fix to a mod, in place.
    ///
    /// Returns the report of what was found, the same as [`ModChecker::check`], plus how
    /// many fixes were applied. Nothing is written when nothing fired.
    pub fn repair(&self, path: &Path) -> Result<RepairOutcome> {
        let archives = mod_archives(path)?;
        if archives.is_empty() {
            anyhow::bail!("{} holds no .wad.client archive to repair", path.display());
        }

        let scratch = tempfile::Builder::new()
            .prefix("hematite-repair-")
            .tempdir()
            .context("Failed to create a scratch directory")?;

        let mut outcome = RepairOutcome::default();
        let mut failures = Vec::new();
        for archive in &archives {
            match self.repair_archive(archive, scratch.path()) {
                Ok((report, applied)) => {
                    outcome.report.merge(report);
                    outcome.fixes_applied += applied;
                    if applied > 0 {
                        outcome.archives_changed += 1;
                    }
                }
                // One archive failing must not abandon the others, and must not be
                // reported as a clean repair either.
                Err(e) => {
                    tracing::warn!("repair failed for {}: {e:#}", archive.display());
                    failures.push(format!("{}: {e:#}", archive.display()));
                }
            }
        }

        if failures.len() == archives.len() {
            anyhow::bail!("every archive failed to repair: {}", failures.join("; "));
        }
        for failure in failures {
            outcome.report.mark_skipped("archive", SkipReason::Failed(failure));
        }

        outcome.report.dedupe();
        outcome.report.attach_catalog(&self.config.reasons);
        Ok(outcome)
    }

    fn repair_archive(&self, archive: &Path, scratch: &Path) -> Result<(CheckReport, u32)> {
        let packed = archive.is_file();
        let folder = if packed {
            self.unpack(archive, scratch)?
        } else {
            archive.to_path_buf()
        };

        let opts = FixOptions {
            dry_run: false,
            detect_only: false,
            repath: None,
            restore_anm: false,
            relocate_combo_bins: false,
            game_wad: None,
            live: self.live.as_ref(),
            // The point of a repair: a missing animation or mesh is fixed by fetching it,
            // and the engine is looking at the installed game anyway.
            pull_missing: true,
            in_place: true,
        };
        let result = crate::fix_folder(
            &folder,
            &self.config,
            &self.enabled_fixes,
            &self.champions,
            &self.hashes,
            &opts,
            &NoopSink,
        )?;

        if packed && result.fixes_applied > 0 {
            repack(&folder, archive)?;
        }
        Ok((result.report, result.fixes_applied))
    }
}

/// What a repair did.
#[derive(Debug, Default)]
pub struct RepairOutcome {
    /// What was found, whether or not it could be fixed.
    pub report: CheckReport,
    /// How many fixes fired across every archive.
    pub fixes_applied: u32,
    /// How many archives were rewritten.
    pub archives_changed: usize,
}

/// Repack a fixed folder over the archive it came from.
///
/// Written to a sibling temporary file and renamed, so a failure part way through leaves
/// the original intact rather than a truncated archive the game cannot read.
fn repack(folder: &Path, archive: &Path) -> Result<()> {
    let mut files: Vec<(u64, String, Vec<u8>)> = Vec::new();
    for entry in walkdir::WalkDir::new(folder).into_iter().flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(folder)
            .context("Failed to relativise a path inside the fixed folder")?
            .to_string_lossy()
            .replace('\\', "/");
        // A root-level 16-hex-digit name IS the chunk's path hash: the extractor's form for
        // a chunk the dictionary could not name. Hashing the hex string would re-key it.
        let hash = hematite_file::wad_folder::hex_chunk_hash(&relative)
            .unwrap_or_else(|| hematite_file::wad_adapter::wad_path_hash(&relative));
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        files.push((hash, relative, bytes));
    }

    // Never write an empty archive over a real one. A fixed folder always has content, so
    // finding none means the walk failed rather than that the mod is empty, and writing the
    // result anyway destroys the mod to report success.
    if files.is_empty() {
        anyhow::bail!(
            "refusing to repack {}: the fixed folder {} yielded no files",
            archive.display(),
            folder.display()
        );
    }

    let temp = archive.with_extension("client.hematite-new");
    {
        let mut out = std::fs::File::create(&temp)
            .with_context(|| format!("Failed to create {}", temp.display()))?;
        hematite_file::wad_builder::build_wad(&files, &[], &mut out)
            .context("Failed to build the repaired archive")?;
        out.sync_all().ok();
    }
    std::fs::rename(&temp, archive).with_context(|| {
        format!("Failed to replace {} with the repaired archive", archive.display())
    })?;
    Ok(())
}

/// Every `.wad.client` archive under `path`, packed or unpacked.
///
/// Three layouts, because all three turn up. `path` can be one archive; a folder of
/// archives; or an unpacked fantome, which keeps `META/info.json` beside a `WAD/` holding
/// them. Looking only at the top level finds nothing in the third, and a mod that reports
/// no archives reads as a mod with nothing wrong with it.
///
/// Packed and unpacked are both accepted because both are real: the CLI is handed downloads,
/// a launcher may store either. Never recursive past those two levels. A WAD folder contains
/// its own directory tree, and descending into it would treat `assets/` as another archive.
fn mod_archives(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return if is_wad_name(path) {
            Ok(vec![path.to_path_buf()])
        } else {
            anyhow::bail!("{} is not a .wad.client archive", path.display())
        };
    }
    if !path.is_dir() {
        anyhow::bail!("{} does not exist", path.display());
    }
    if is_wad_folder(path) {
        return Ok(vec![path.to_path_buf()]);
    }

    let mut out = Vec::new();
    collect_archives(path, &mut out)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    // The fantome convention. Case-insensitive because the directory is written by whatever
    // packed the mod.
    if let Some(wad_dir) = child_named(path, "wad") {
        let _ = collect_archives(&wad_dir, &mut out);
    }

    out.sort();
    out.dedup();
    Ok(out)
}

fn collect_archives(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)?.flatten() {
        let child = entry.path();
        if is_wad_name(&child) {
            out.push(child);
        }
    }
    Ok(())
}

/// A stable per-path key, so two archives with the same file name unpack side by side.
fn hash_of(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

/// Whether this name is a `.wad.client`, whatever it is on disk.
fn is_wad_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.to_lowercase().ends_with(".wad.client"))
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
    path.is_dir() && is_wad_name(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(root: &Path, names: &[&str]) {
        for name in names {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
    }

    /// The pair that made this matter, and the reason it is a regression test rather than a
    /// comment: run the wrong way round, a material ends up with its sampler's NAME sitting
    /// in `texturePath` and the real texture path gone.
    #[test]
    fn the_material_rules_keep_their_declared_order() {
        let raw = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/fix_config.toml"
        ))
        .expect("the repo config must be readable");
        let config: FixConfig = toml::from_str(&raw).expect("the repo config must parse");

        let order = enabled_fixes_in_order(&config);
        let at = |id: &str| order.iter().position(|x| x == id);
        let (path_rule, sampler_rule) = (
            at("staticmat_texturepath").expect("staticmat_texturepath is enabled"),
            at("staticmat_samplername").expect("staticmat_samplername is enabled"),
        );
        assert!(
            path_rule < sampler_rule,
            "texturePath must be vacated before the sampler name moves in; got {order:?}"
        );

        let mut unique = order.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), order.len(), "a rule must not run twice: {order:?}");
    }

    /// The list is what the config declares, in that order, every time.
    #[test]
    fn a_folder_of_archives_yields_each_one() {
        let dir = tempfile::tempdir().unwrap();
        make(
            dir.path(),
            &["Jhin.wad.client", "Global.wad.client", "META", "notes"],
        );
        let found = mod_archives(dir.path()).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|p| is_wad_folder(p)));
    }

    /// Handed one archive directly, that is the whole answer.
    #[test]
    fn a_single_archive_is_itself() {
        let dir = tempfile::tempdir().unwrap();
        let wad = dir.path().join("Jhin.wad.client");
        std::fs::create_dir_all(wad.join("data/characters")).unwrap();
        assert_eq!(mod_archives(&wad).unwrap(), vec![wad]);
    }

    /// A WAD folder's own subdirectories are its content, not more archives.
    #[test]
    fn the_search_does_not_descend_into_an_archive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("A.wad.client/nested.wad.client")).unwrap();
        let found = mod_archives(dir.path()).unwrap();
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

        let found = mod_archives(dir.path()).unwrap();
        assert_eq!(found.len(), 2, "both archives under WAD/");
    }

    /// Some packers write `wad`, some `WAD`.
    #[test]
    fn the_wad_folder_name_is_matched_without_case() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("wad/UI.wad.client")).unwrap();
        assert_eq!(mod_archives(dir.path()).unwrap().len(), 1);
    }

    /// A packed archive is as valid an input as an unpacked one.
    /// The unpack/repack pair has to be lossless, or a repair that fixed one file would
    /// quietly corrupt every other file in the archive.
    #[test]
    fn a_folder_repacks_into_a_readable_archive() {
        use hematite_file::wad_adapter::{wad_path_hash, WadFile};

        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Jhin.wad.client");
        let entries = [
            ("data/characters/jhin/skins/skin0.bin", &b"PROPfirst"[..]),
            ("assets/characters/jhin/x.tex", &b"TEXbytes"[..]),
        ];
        for (path, bytes) in entries {
            let file = folder.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, bytes).unwrap();
        }
        // A chunk the dictionary could not name keeps its hex name at the root, and must
        // come back under its ORIGINAL hash rather than the hash of the hex string.
        let unnamed = 0x0123_4567_89ab_cdefu64;
        std::fs::write(folder.join(format!("{unnamed:016x}")), b"unnamed").unwrap();

        let archive = dir.path().join("Jhin.wad.client.packed");
        repack(&folder, &archive).unwrap();

        let mut wad = WadFile::open(&archive).unwrap();
        let hashes = wad.chunk_hash_set();
        for (path, bytes) in entries {
            let hash = wad_path_hash(path);
            assert!(hashes.contains(&hash), "{path} missing from the archive");
            assert_eq!(
                wad.extract_chunk_by_hash(hash).unwrap().as_deref(),
                Some(bytes),
                "{path} came back different"
            );
        }
        assert!(
            hashes.contains(&unnamed),
            "an unnamed chunk must keep its original hash"
        );
    }

    /// The original must survive a repack that cannot finish.
    #[test]
    fn a_failed_repack_leaves_the_original_alone() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("Jhin.wad.client");
        std::fs::write(&archive, b"the original bytes").unwrap();

        // A folder that does not exist: nothing to walk, so nothing to write.
        let missing = dir.path().join("not-there");
        let _ = repack(&missing, &archive);

        assert_eq!(
            std::fs::read(&archive).unwrap(),
            b"the original bytes",
            "a repack that found nothing must not have touched the archive"
        );
    }

    #[test]
    fn a_packed_archive_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let packed = dir.path().join("WAD/Jhin.wad.client");
        std::fs::create_dir_all(packed.parent().unwrap()).unwrap();
        std::fs::write(&packed, b"RW").unwrap();
        assert_eq!(mod_archives(dir.path()).unwrap(), vec![packed.clone()]);
        assert_eq!(mod_archives(&packed).unwrap(), vec![packed]);
    }

    /// Two mods can both ship a `Global.wad.client`, so the scratch key is per path.
    #[test]
    fn two_archives_with_one_name_get_different_scratch_keys() {
        let a = Path::new("/mods/one/Global.wad.client");
        let b = Path::new("/mods/two/Global.wad.client");
        assert_ne!(hash_of(a), hash_of(b));
        assert_eq!(hash_of(a), hash_of(a));
    }

    /// A mod with archives in both places yields each exactly once.
    #[test]
    fn both_layouts_at_once_do_not_double_count() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Jhin.wad.client")).unwrap();
        std::fs::create_dir_all(dir.path().join("WAD/UI.wad.client")).unwrap();

        let found = mod_archives(dir.path()).unwrap();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn a_folder_with_no_archive_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        make(dir.path(), &["META", "assets"]);
        assert!(mod_archives(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn a_file_that_is_not_an_archive_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mod.fantome");
        std::fs::write(&file, b"").unwrap();
        assert!(mod_archives(&file).is_err());
    }

    #[test]
    fn the_extension_match_ignores_case() {
        let dir = tempfile::tempdir().unwrap();
        make(dir.path(), &["Jhin.WAD.CLIENT"]);
        assert_eq!(mod_archives(dir.path()).unwrap().len(), 1);
    }
}
