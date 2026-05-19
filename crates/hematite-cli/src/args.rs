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
                When set, files referenced by BIN strings but missing from the mod \
                are extracted from this WAD and included in the output, so the \
                repathed mod is fully self-contained. Requires --repath."
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
const ALL_FIX_IDS: &[&str] = &[
    "healthbar_fix",
    "staticmat_texturepath",
    "staticmat_samplername",
    "black_icons",
    "dds_to_tex",
    "champion_bin_remover",
    "bnk_remover",
    "anm_remover",
    "dds_texture_converter",
    "sco_mesh_converter",
    "fix_tex_dimensions",
    "vfx_shape_fix",
    "shader_fallback",
    "entry_validator",
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

    // If --all or no specific flags: apply all fixes
    if cli.all || fixes.is_empty() {
        return ALL_FIX_IDS.iter().map(|s| (*s).into()).collect();
    }

    fixes
}
