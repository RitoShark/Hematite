//! CLI argument definitions using clap derive.
//!
//! ## Available flags
//! | Flag | Fix |
//! |------|-----|
//! | `--healthbar` | Missing HP bar fix |
//! | `--white-model` | TextureName → TexturePath rename |
//! | `--black-icons` | .dds → .tex icon conversion |
//! | `--particles` | Broken particle texture fix |
//! | `--remove-champion-bins` | Remove outdated champion data |
//! | `--remove-bnk` | Remove incompatible audio files |
//! | `--vfx-shape` | VFX shape migration (14.1+) |
//! | `--pull-gear` | Pull missing GearSkinUpgrade entries from the live game |
//! | `--pull-cac` | Pull missing ContextualActionData entries from the live game |
//! | `--fix-refs` | Rewrite dead asset references using the live game |
//! | `--relocate-bins` | Relocate legacy combo-bin WAD entries |
//! | `--file-refs` | Convert migrated asset-path strings to xxh64 file references |
//! | `--all` / `-a` | Enable all fixes |
//!
//! ## Output control
//! | Flag | Effect |
//! |------|--------|
//! | `--json` | JSON output for automation |
//! | `--dry-run` | Show what would be fixed, don't modify |
//! | `-v <level>` | Verbosity: quiet, normal, verbose, trace |
//! | `-o <path>` | Output path (default: overwrite input) |

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "hematite-cli")]
#[command(about = "League of Legends custom skin fixer")]
#[command(version)]
pub struct Cli {
    /// Input file or directory to process. Not required for `--check-version`.
    #[arg(required_unless_present = "check_version")]
    pub input: Option<PathBuf>,

    /// Output path (default: overwrite input)
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    // Fix flags
    #[arg(long, help = "Fix missing health bars")]
    pub healthbar: bool,

    #[arg(long, help = "Fix white models (TextureName → TexturePath)")]
    pub white_model: bool,

    #[arg(long, help = "Fix black/missing icons (.dds → .tex)")]
    pub black_icons: bool,

    #[arg(long, help = "Fix broken particle textures")]
    pub particles: bool,

    #[arg(long, help = "Remove outdated champion data BINs")]
    pub remove_champion_bins: bool,

    #[arg(long, help = "Remove incompatible BNK audio files")]
    pub remove_bnk: bool,

    #[arg(long, help = "Fix VFX shape format (14.1+ migration)")]
    pub vfx_shape: bool,

    #[arg(long, help = "Remove .anm animation files from mod")]
    pub remove_anm: bool,

    #[arg(long, help = "Fix invalid shader references with closest match")]
    pub fix_shaders: bool,

    #[arg(
        long,
        help = "Remove unreferenced entries (CAD, AnimGraph, GearSkinUpgrade)"
    )]
    pub validate_entries: bool,

    #[arg(long, help = "Convert DDS textures to TEX format")]
    pub fix_textures: bool,

    #[arg(long, help = "Convert ASCII SCO meshes to binary SCB")]
    pub fix_meshes: bool,

    #[arg(long, help = "Fix non-block-aligned TEX texture dimensions")]
    pub fix_tex_dimensions: bool,

    #[arg(
        long,
        help = "Pull missing GearSkinUpgrade entries from the live game (fixes dead gear links)"
    )]
    pub pull_gear: bool,

    #[arg(
        long,
        help = "Pull missing ContextualActionData entries from the live game (restores voiceovers)"
    )]
    pub pull_cac: bool,

    #[arg(
        long,
        help = "Rewrite dead asset references to a live form using the installed game"
    )]
    pub fix_refs: bool,

    #[arg(
        long,
        help = "Relocate legacy combo-bin WAD entries to their multi-skin path"
    )]
    pub relocate_bins: bool,

    #[arg(
        long,
        help = "Convert migrated asset-path strings to xxh64 'file' references \
                (Riot's BIN type migration — old string-typed fields no longer load)"
    )]
    pub file_refs: bool,

    #[arg(short, long, help = "Enable all fixes")]
    pub all: bool,

    // Output control
    #[arg(long, help = "JSON output for automation")]
    pub json: bool,

    #[arg(long, help = "Show what would be fixed without modifying files")]
    pub dry_run: bool,

    #[arg(
        long,
        help = "Check mode: detect issues and report skin info without fixing"
    )]
    pub check: bool,

    // Repath flags
    #[arg(
        long,
        help = "Repath mod assets with a prefix to prevent hash collisions with base-game files"
    )]
    pub repath: bool,

    #[arg(
        long,
        value_name = "PREFIX",
        help = "Custom repath prefix. If omitted, derived Topaz-style from the input \
                filename + skin number (e.g. .yone1_). With the default in-folder layout \
                the prefix is concatenated to the next path segment, so \
                \".yone1_\" turns assets/characters/yone/... into \
                ASSETS/.yone1_characters/yone/..."
    )]
    pub repath_prefix: Option<String>,

    #[arg(
        long,
        value_enum,
        default_value = "in-folder",
        help = "Repath layout. 'in-folder' = Topaz-style (concat to next segment, ROOT \
                upper-cased). 'nested' = LtMAO-style (prefix as its own folder)."
    )]
    pub repath_layout: RepathLayoutArg,

    #[arg(
        long,
        help = "Inject invisible 1×1 placeholder textures for repathed paths missing from the WAD \
                (prevents black/missing-texture crashes). Requires --repath."
    )]
    pub invis_texture: bool,

    #[arg(
        long,
        value_name = "PATH",
        help = "Path to the base-game champion .wad.client (e.g. \
                \"C:/Riot Games/.../Champions/ahri.wad.client\"). \
                When set with --repath, files referenced by BIN strings but missing \
                from the mod are extracted from this WAD and included in the output, \
                so the repathed mod is fully self-contained. Also used by \
                --restore-anm as its game source when no live install is available, \
                independent of --repath."
    )]
    pub game_wad: Option<std::path::PathBuf>,

    #[arg(
        long,
        help = "Small mod optimization: only validate paths, don't add fallback assets"
    )]
    pub small_mod: bool,

    #[arg(long, help = "Process all skins found in mod (not just primary skin)")]
    pub all_skins: bool,

    #[arg(short = 'v', long, default_value = "normal", help = "Verbosity level")]
    pub verbosity: Verbosity,

    // -- Version-gate controls (see version_check.rs) -------------------
    #[arg(
        long,
        help = "Bypass the remote version-gate check. The advisory banner is still printed, \
                but a hard-block 'CLI too old' verdict no longer prevents execution. Use \
                this for CI runs or when you know the new minimum is wrong."
    )]
    pub skip_version_check: bool,

    #[arg(
        long,
        help = "Print version check status and exit without processing any input."
    )]
    pub check_version: bool,

    // -- Live-game features ----------------------------------------------
    #[arg(
        long,
        value_name = "DIR",
        help = "Path to the League of Legends install (root or Game dir). \
                  If omitted, hematite auto-detects the install. Live-game \
                  features (deep repair, gear/CAC pull, ref ladder, --restore-anm) \
                  use this."
    )]
    pub game_path: Option<std::path::PathBuf>,

    #[arg(
        long,
        help = "Disable all live-game features (no install detection, no game pulls)"
    )]
    pub no_live: bool,

    #[arg(
        long,
        help = "Restore missing .anm animation files by pulling them from the game \
                        (disables anm_remover for this run)"
    )]
    pub restore_anm: bool,
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum Verbosity {
    Quiet,
    Normal,
    Verbose,
    Trace,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum RepathLayoutArg {
    /// Topaz-style: ROOT/{prefix}{seg1}/seg2/... (default)
    InFolder,
    /// LtMAO-style: root/{prefix}/seg1/seg2/...
    Nested,
}

impl From<RepathLayoutArg> for hematite_types::repath::RepathLayout {
    fn from(v: RepathLayoutArg) -> Self {
        match v {
            RepathLayoutArg::InFolder => Self::InFolder,
            RepathLayoutArg::Nested => Self::Nested,
        }
    }
}

/// All known fix IDs in application order.
///
/// Every ID here MUST have a rule in `config/fix_config.json` (`fixes` or
/// `wad_fixes`) — `apply_fixes` records an error for any selected BIN-level
/// ID absent from both maps, and main.rs bails on any error. Guarded by
/// `all_fix_ids_exist_in_repo_config` below.
///
/// `apply_fixes` (see `hematite-core/src/pipeline.rs`) walks
/// `selected_fix_ids` in the order given here, not config declaration order
/// — so relative position in this list is load-bearing, not cosmetic.
/// `gear_pull` and `cac_pull` pull missing entries out of the live game's
/// BIN closure; they must run BEFORE `entry_validator`, which deletes
/// unreferenced entries of the same types. Running entry_validator first
/// would strip the very links gear_pull/cac_pull need to resolve, so they
/// sit immediately before it despite `entry_validator` being declared
/// earlier in `fix_config.json`.
const ALL_FIX_IDS: &[&str] = &[
    "healthbar_fix",
    "staticmat_texturepath",
    "staticmat_samplername",
    "black_icons",
    "dds_to_tex",
    "resolve_dead_refs",
    "champion_bin_remover",
    "combo_bin_relocate",
    "bnk_remover",
    "anm_remover",
    "dds_texture_converter",
    "sco_mesh_converter",
    "fix_tex_dimensions",
    "vfx_shape_fix",
    "shader_fallback",
    "gear_pull",
    "cac_pull",
    "entry_validator",
    "file_ref_migration",
];

/// Collect selected fix IDs based on CLI flags.
///
/// If `--all` is set or no flags are passed, returns all fix IDs.
/// Otherwise, returns only the specifically selected fixes.
pub fn collect_selected_fixes(cli: &Cli) -> Vec<String> {
    let mut fixes = Vec::new();
    if cli.healthbar {
        fixes.push("healthbar_fix".into());
    }
    if cli.white_model {
        fixes.push("staticmat_texturepath".into());
        fixes.push("staticmat_samplername".into());
    }
    if cli.black_icons {
        fixes.push("black_icons".into());
    }
    if cli.particles {
        fixes.push("dds_to_tex".into());
    }
    if cli.remove_champion_bins {
        fixes.push("champion_bin_remover".into());
    }
    if cli.remove_bnk {
        fixes.push("bnk_remover".into());
    }
    if cli.vfx_shape {
        fixes.push("vfx_shape_fix".into());
    }
    if cli.remove_anm {
        fixes.push("anm_remover".into());
    }
    if cli.fix_shaders {
        fixes.push("shader_fallback".into());
    }
    if cli.validate_entries {
        fixes.push("entry_validator".into());
    }
    if cli.fix_textures {
        fixes.push("dds_texture_converter".into());
    }
    if cli.fix_meshes {
        fixes.push("sco_mesh_converter".into());
    }
    if cli.fix_tex_dimensions {
        fixes.push("fix_tex_dimensions".into());
    }
    if cli.pull_gear {
        fixes.push("gear_pull".into());
    }
    if cli.pull_cac {
        fixes.push("cac_pull".into());
    }
    if cli.fix_refs {
        fixes.push("resolve_dead_refs".into());
    }
    if cli.relocate_bins {
        fixes.push("combo_bin_relocate".into());
    }
    if cli.file_refs {
        fixes.push("file_ref_migration".into());
    }

    // If --all or no specific flags: apply all fixes
    if cli.all || fixes.is_empty() {
        return ALL_FIX_IDS.iter().map(|s| (*s).into()).collect();
    }

    fixes
}

#[cfg(test)]
mod tests {
    use super::ALL_FIX_IDS;

    /// Regression guard: every ID in ALL_FIX_IDS must have a rule in the
    /// repo's fix config (`fixes` ∪ `wad_fixes`). An ID selected by default
    /// (`--all` / no flags) but missing from the config makes `apply_fixes`
    /// push "Fix rule not found" into result.errors, and main.rs bails on
    /// any error — i.e. every default invocation hard-fails. Never let
    /// ALL_FIX_IDS drift ahead of the config again.
    fn load_repo_config() -> hematite_types::config::FixConfig {
        let config_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/fix_config.toml");
        let raw = std::fs::read_to_string(&config_path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", config_path.display()));
        toml::from_str(&raw)
            .unwrap_or_else(|e| panic!("cannot parse {}: {e}", config_path.display()))
    }

    #[test]
    fn all_fix_ids_exist_in_repo_config() {
        let config = load_repo_config();

        let missing: Vec<&&str> = ALL_FIX_IDS
            .iter()
            .filter(|id| !config.fixes.contains_key(**id) && !config.wad_fixes.contains_key(**id))
            .collect();

        assert!(
            missing.is_empty(),
            "ALL_FIX_IDS entries missing from config/fix_config.json \
             (fixes ∪ wad_fixes): {missing:?} — add the config rule first, \
             then list the ID here"
        );
    }

    /// Round-trip the repo's fix config and assert the new v2.2.0 rules
    /// (`gear_pull`, `cac_pull`, `resolve_dead_refs`, `combo_bin_relocate`)
    /// deserialize into the expected `DetectionRule`/`TransformAction`
    /// variants with the fields the pipeline actually reads. Catches silent
    /// schema drift (e.g. a typo'd `"type"` tag falling back to a serde
    /// error, or a field name mismatch) that `all_fix_ids_exist_in_repo_config`
    /// wouldn't — that test only checks key presence, not shape.
    #[test]
    fn new_v2_2_0_rules_parse_into_expected_variants() {
        use hematite_types::config::{
            DetectionRule, TransformAction, WadDetectionRule, WadTransformAction,
        };

        let config = load_repo_config();

        assert_eq!(config.version, "2.3.0");

        // The central enable list is the single authority — spot-check both
        // directions plus a WAD-level entry.
        assert!(config.enabled_fixes.is_some());
        assert!(config.is_fix_enabled("gear_pull"));
        assert!(config.is_fix_enabled("bnk_remover"));
        assert!(!config.is_fix_enabled("vfx_entry_split"));
        assert!(!config.is_fix_enabled("no_such_fix"));

        // file_ref_migration: class_field_is_string detect +
        // retype_string_to_file apply, post_repath phase (it destroys the
        // strings repath needs, so it must never run in the standard pass).
        let file_refs = config
            .fixes
            .get("file_ref_migration")
            .expect("file_ref_migration rule missing");
        assert!(config.is_fix_enabled("file_ref_migration"));
        assert_eq!(file_refs.severity, "critical");
        assert_eq!(
            file_refs.phase,
            hematite_types::config::FixPhase::PostRepath,
            "file_ref_migration must run post-repath"
        );
        match &file_refs.detect {
            DetectionRule::ClassFieldIsString { targets } => {
                assert!(
                    targets
                        .iter()
                        .any(|t| t.class == "AnimationResourceData"
                            && t.field == "mAnimationFilePath")
                );
            }
            other => {
                panic!("file_ref_migration.detect: expected ClassFieldIsString, got {other:?}")
            }
        }
        match &file_refs.apply {
            TransformAction::RetypeStringToFile { targets } => {
                assert!(targets.iter().any(
                    |t| t.class == "StaticMaterialShaderSamplerDef" && t.field == "texturePath"
                ));
            }
            other => {
                panic!("file_ref_migration.apply: expected RetypeStringToFile, got {other:?}")
            }
        }

        // gear_pull: dead_entry_link detect + pull_entries_from_game apply
        // with nuke_fallback_field set (last-resort fallback).
        let gear_pull = config
            .fixes
            .get("gear_pull")
            .expect("gear_pull rule missing");
        assert!(config.is_fix_enabled("gear_pull"));
        assert_eq!(gear_pull.severity, "critical");
        match &gear_pull.detect {
            DetectionRule::DeadEntryLink {
                main_entry_type,
                targets,
            } => {
                assert_eq!(main_entry_type, "SkinCharacterDataProperties");
                assert_eq!(targets.len(), 1);
                assert_eq!(targets[0].entry_type, "GearSkinUpgrade");
                assert_eq!(targets[0].reference_field, "skinUpgradeData");
            }
            other => panic!("gear_pull.detect: expected DeadEntryLink, got {other:?}"),
        }
        match &gear_pull.apply {
            TransformAction::PullEntriesFromGame {
                targets,
                nuke_fallback_field,
                ..
            } => {
                assert_eq!(targets.len(), 1);
                assert_eq!(
                    nuke_fallback_field.as_deref(),
                    Some("skinUpgradeData"),
                    "gear_pull must nuke skinUpgradeData when the link can't be pulled"
                );
            }
            other => panic!("gear_pull.apply: expected PullEntriesFromGame, got {other:?}"),
        }

        // cac_pull: dead_entry_link detect + pull_entries_from_game apply
        // with NO nuke_fallback_field (drop-only behavior).
        let cac_pull = config.fixes.get("cac_pull").expect("cac_pull rule missing");
        assert!(config.is_fix_enabled("cac_pull"));
        assert_eq!(cac_pull.severity, "medium");
        match &cac_pull.detect {
            DetectionRule::DeadEntryLink { targets, .. } => {
                assert_eq!(targets.len(), 1);
                assert_eq!(targets[0].entry_type, "ContextualActionData");
            }
            other => panic!("cac_pull.detect: expected DeadEntryLink, got {other:?}"),
        }
        match &cac_pull.apply {
            TransformAction::PullEntriesFromGame {
                nuke_fallback_field,
                ..
            } => {
                assert!(
                    nuke_fallback_field.is_none(),
                    "cac_pull must drop dead links only, not nuke a fallback field"
                );
            }
            other => panic!("cac_pull.apply: expected PullEntriesFromGame, got {other:?}"),
        }

        // resolve_dead_refs: recursive_string_extension_not_in_wad detect
        // (cheap trigger) + resolve_dead_refs apply covering all extensions.
        let resolve_dead_refs = config
            .fixes
            .get("resolve_dead_refs")
            .expect("resolve_dead_refs rule missing");
        assert!(config.is_fix_enabled("resolve_dead_refs"));
        assert_eq!(resolve_dead_refs.severity, "high");
        match &resolve_dead_refs.detect {
            DetectionRule::RecursiveStringExtensionNotInWad { extension, .. } => {
                assert_eq!(extension, ".dds");
            }
            other => panic!(
                "resolve_dead_refs.detect: expected RecursiveStringExtensionNotInWad, got {other:?}"
            ),
        }
        match &resolve_dead_refs.apply {
            TransformAction::ResolveDeadRefs { extensions } => {
                for ext in ["dds", "tex", "anm", "skn", "skl", "scb", "sco"] {
                    assert!(
                        extensions.iter().any(|e| e == ext),
                        "resolve_dead_refs.apply.extensions missing {ext:?}: {extensions:?}"
                    );
                }
            }
            other => panic!("resolve_dead_refs.apply: expected ResolveDeadRefs, got {other:?}"),
        }

        // combo_bin_relocate: descriptor-only wad_fixes entry. The pipeline
        // step that does the real work is keyed off the fix id directly
        // (see main.rs), not off this rule's detect/apply — just assert it
        // parses and is present so the guard test + reporting resolve it.
        let combo_bin_relocate = config
            .wad_fixes
            .get("combo_bin_relocate")
            .expect("combo_bin_relocate rule missing");
        assert!(config.is_fix_enabled("combo_bin_relocate"));
        matches!(
            combo_bin_relocate.detect,
            WadDetectionRule::FilePattern { .. }
        )
        .then_some(())
        .unwrap_or_else(|| panic!("combo_bin_relocate.detect: expected FilePattern"));
        matches!(
            combo_bin_relocate.apply,
            WadTransformAction::RenameFile { .. }
        )
        .then_some(())
        .unwrap_or_else(|| panic!("combo_bin_relocate.apply: expected RenameFile"));
    }
}
