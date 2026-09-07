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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepathLayout {
    /// `assets/X/y/z` → `ASSETS/{prefix}X/y/z`. Topaz-compatible.
    #[default]
    InFolder,
    /// `assets/X/y/z` → `assets/{prefix}/X/y/z`. LtMAO-compatible.
    Nested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepathStatus {
    Repathed,
    HalflyRepathed,
    NotRepathed,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RepathReport {
    pub source: String,
    pub status: RepathStatus,
    pub percent: f64,
    pub canonical: u32,
    pub prefixed: u32,
    pub total: u32,
    pub bins_scanned: u32,
    pub bins_failed: u32,
    pub unresolved_hashes: u32,
    pub needs_repath: bool,
    pub skip_reason: Option<String>,
}

impl RepathReport {
    pub fn finish(&mut self) {
        self.total = self.canonical + self.prefixed;
        self.percent = if self.total == 0 {
            0.0
        } else {
            self.prefixed as f64 / self.total as f64 * 100.0
        };
        self.status = if self.total == 0 || self.bins_failed > 0 || self.unresolved_hashes > 0 {
            RepathStatus::Unknown
        } else if self.percent >= 99.0 {
            RepathStatus::Repathed
        } else if self.percent >= 60.0 {
            RepathStatus::HalflyRepathed
        } else {
            RepathStatus::NotRepathed
        };
        self.needs_repath = self.canonical > 0;
        self.skip_reason = if self.bins_failed > 0 {
            Some("Repath skipped: one or more input BINs could not be parsed".into())
        } else if self.bins_scanned == 0 {
            Some("Repath skipped: input contains no BIN files".into())
        } else {
            None
        };
    }

    pub fn summary(&self) -> String {
        if let Some(reason) = &self.skip_reason {
            return reason.clone();
        }
        format!(
            "Repath check: {:?}, {:.1}% prefixed; {} shipped canonical reference(s), {} unresolved hash(es)",
            self.status, self.percent, self.canonical, self.unresolved_hashes
        )
    }
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

    /// List of rules mapping regex patterns to asset placeholders.
    pub placeholder_rules: Vec<crate::config::PlaceholderRule>,
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
            placeholder_rules: Vec::new(),
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
