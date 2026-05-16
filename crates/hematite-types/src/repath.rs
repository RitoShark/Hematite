//! Repath configuration types.
//!
//! Hematite repath = Topaz `FixPath` semantics: insert a short prefix after
//! the first segment of every asset/data path so the mod's files no longer
//! collide with base-game hashes (or with other installed mods).
//!
//! Two layout modes are supported:
//!
//! * **`InFolder`** (Topaz default) — the prefix is **concatenated** to the
//!   second segment with no slash:
//!   `assets/characters/yone/...` → `ASSETS/.yone1_characters/yone/...`.
//!   Old-school launchers expect this exact layout.
//!
//! * **`Nested`** — the prefix is its own folder segment:
//!   `assets/characters/yone/...` → `assets/yone1/characters/yone/...`.
//!   Slightly cleaner for human inspection; matches LtMAO's `bumpath`.

use std::path::PathBuf;

/// Where the prefix is placed inside the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepathLayout {
    /// `assets/X/y/z` → `ASSETS/{prefix}X/y/z`. Topaz-compatible.
    #[default]
    InFolder,
    /// `assets/X/y/z` → `assets/{prefix}/X/y/z`. LtMAO-compatible.
    Nested,
}

/// Options controlling the asset-repath pipeline.
#[derive(Debug, Clone)]
pub struct RepathOptions {
    /// Prefix inserted into every asset path. Topaz convention is
    /// `.{shortChar}{skinNo}_` (e.g. `.yone1_`); the trailing underscore is
    /// part of the prefix, not added by us.
    pub prefix: String,

    /// Where to insert the prefix.
    pub layout: RepathLayout,

    /// Inject invisible 1×1 `.tex` placeholders for repathed texture paths
    /// that don't have a corresponding file in the WAD.
    pub invis_texture: bool,

    /// Skip voice-over audio paths (`.../wwise2016/vo/...`). VO files live
    /// in separate language WADs and **must** keep their original paths.
    pub skip_vo: bool,

    /// Path to the base-game champion `.wad.client`. When set, files
    /// referenced from BIN strings but missing in the mod are pulled from
    /// here so the repathed mod is fully self-contained.
    pub game_wad: Option<PathBuf>,
}

impl RepathOptions {
    /// Create options with sensible Topaz-faithful defaults.
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            layout: RepathLayout::default(),
            invis_texture: false,
            skip_vo: true,
            game_wad: None,
        }
    }

    /// Derive a Topaz-style prefix from a champion name + skin number.
    ///
    /// Truncates the champion to 4 chars and appends the skin number and a
    /// trailing `_`, matching Topaz's `$".{shortChar}{skinNo}_"`. Returns
    /// `"bum"` if the name is empty (last-resort fallback so we never emit
    /// an empty prefix).
    pub fn derive_prefix(champion: &str, skin_no: u32) -> String {
        let clean: String = champion
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        if clean.is_empty() {
            return "bum".to_string();
        }
        let short: String = clean.chars().take(4).collect::<String>().to_lowercase();
        format!(".{}{}_", short, skin_no)
    }
}
