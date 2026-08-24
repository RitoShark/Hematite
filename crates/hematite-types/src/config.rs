//! Fix configuration schema — deserialized from `fix_config.json`.
//!
//! This module defines the JSON schema for fix rules. Each rule has:
//! - A **detection rule** that identifies when an issue exists
//! - A **transform action** that fixes the issue
//!
//! The schema is designed to be config-driven: new fixes can be added by
//! editing JSON without changing Rust code (for simple detection/transform patterns).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root config structure loaded from fix_config.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixConfig {
    pub version: String,
    pub last_updated: String,
    /// Central enable list: when present, a fix is enabled iff its ID is
    /// listed here, and the per-rule `enabled` flags are ignored. When
    /// absent, per-rule `enabled` governs (legacy configs). Read through
    /// [`FixConfig::is_fix_enabled`], never the rule flag directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_fixes: Option<Vec<String>>,
    /// BIN-level fixes (operate on parsed BIN trees)
    pub fixes: HashMap<String, FixRule>,
    /// WAD-level fixes (operate on files before BIN parsing)
    #[serde(default)]
    pub wad_fixes: HashMap<String, WadFixRule>,
    /// Default repath settings (can be overridden by CLI flags).
    /// When `enabled` is true, drag-and-drop runs repathing automatically.
    #[serde(default)]
    pub repath: RepathConfig,
    /// Reason catalog: what each defect means to the player, how severe it is, and what
    /// to do about it. Lives in the same config as the rules so a new crash class needs
    /// no rebuild. Rules reference entries here by id through [`FixRule::reason`].
    #[serde(default)]
    pub reasons: crate::diagnostic::ReasonCatalog,
    /// Settings for the whole-archive loose-texture measurement.
    #[serde(default)]
    pub loose_textures: LooseTextureConfig,
    /// Proportion checks: how much of what a mod references actually resolves.
    #[serde(default)]
    pub ratio_checks: Vec<RatioCheckConfig>,
    /// Settings for the ability-VFX measurement.
    #[serde(default)]
    pub vfx_ratio: VfxRatioConfig,
    /// Settings for the archive fan-out measurement.
    #[serde(default)]
    pub wad_fanout: WadFanoutConfig,
    /// Effects a specific champion cannot be played without.
    #[serde(default)]
    pub signature_vfx: Vec<SignatureVfxConfig>,
    /// Textures a render pass binds without a fallback.
    #[serde(default)]
    pub required_textures: Vec<RequiredTextureConfig>,
}

/// A field whose texture the renderer binds without checking it arrived.
///
/// Nearly every missing texture falls back to a default and merely renders wrong. The few
/// listed here do not: the pass dereferences a null handle and the map fails to load. What
/// separates them is the field the path is stored in, not the file, so each entry names a
/// field rather than an extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredTextureConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Field name, as written in the schema. Hashed with FNV-1a over its lowercased form.
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One champion's signature effect, measured on its own.
///
/// The overall ability-VFX share cannot see these: losing them leaves the rest of the kit
/// rendering, so the share stays low while the champion's defining mechanic has gone
/// invisible. Weighting them inside the general ratio would distort every other champion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureVfxConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Whose effect this is, lowercase.
    pub champion: String,
    /// Name fragment identifying the effect, matched case-insensitively.
    pub contains: String,
    /// Fire when at least this share of them is missing. Inclusive.
    pub dead_at: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Settings for the archive fan-out measurement.
///
/// A mod whose files collide with shared game assets gets written into every archive that
/// holds a copy. The game still runs, but the overlay balloons and unrelated mods can be
/// disturbed, so it is worth telling the author.
///
/// Off by default: answering it needs every WAD's table of contents, which no other check
/// requires, and the cost is not worth paying on every run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WadFanoutConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Warn when the mod would be written into MORE than this many archives.
    #[serde(default = "default_fanout_threshold")]
    pub threshold: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn default_fanout_threshold() -> usize {
    50
}

impl Default for WadFanoutConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: default_fanout_threshold(),
            reason: None,
        }
    }
}

/// Settings for the ability-VFX measurement.
///
/// Separate from the generic proportion checks because the denominator is not "everything
/// referenced": most of a champion's particle set is recalls, idles and ground decals, and
/// counting those would make the share meaningless. Only gameplay effects count, which
/// needs the classification these marker lists drive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VfxRatioConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Share at which the abilities are considered unrenderable. INCLUSIVE.
    #[serde(default = "default_vfx_fail")]
    pub fail_at: f32,
    /// Share at which the abilities are visibly degraded. INCLUSIVE, unlike the asset
    /// ratio's exclusive warn bound; both were tuned separately.
    #[serde(default = "default_vfx_warn")]
    pub warn_at: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn_reason: Option<String>,
    /// Game modes no live queue renders. Their effects are always absent and never a defect.
    #[serde(default)]
    pub legacy_markers: Vec<String>,
    /// Audio cues, which are not visual effects at all.
    #[serde(default)]
    pub audio_markers: Vec<String>,
    /// Non-gameplay effects: recalls, idles, deaths, emotes.
    #[serde(default)]
    pub cosmetic_markers: Vec<String>,
    /// Helper overlays: markers, timers, range rings.
    #[serde(default)]
    pub subhelper_markers: Vec<String>,
}

fn default_vfx_fail() -> f32 {
    0.80
}
fn default_vfx_warn() -> f32 {
    0.30
}

impl Default for VfxRatioConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fail_at: default_vfx_fail(),
            warn_at: default_vfx_warn(),
            fail_reason: None,
            warn_reason: None,
            legacy_markers: Vec::new(),
            audio_markers: Vec::new(),
            cosmetic_markers: Vec::new(),
            subhelper_markers: Vec::new(),
        }
    }
}

/// One proportion check.
///
/// Some defects are about share rather than any single file: a skin missing one particle
/// texture looks fine, a skin missing most of them is visibly broken. Not a `FixRule`,
/// because the question spans the whole archive rather than one BIN.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RatioCheckConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Identifier used as the diagnostic's rule id.
    pub id: String,
    /// Extensions counted, including the dot.
    pub extensions: Vec<String>,
    /// Below this many distinct references the sample says nothing: one missing file out
    /// of a handful is a large percentage and no evidence.
    pub min_total: usize,
    /// Share that must be EXCEEDED to warn. Exclusive.
    pub warn_at: f32,
    /// Share that must be REACHED to fail. Inclusive. The asymmetry with `warn_at` is
    /// deliberate; both were tuned against real mods.
    pub fail_at: f32,
    /// Reason reported between `warn_at` and `fail_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn_reason: Option<String>,
    /// Reason reported at or above `fail_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_reason: Option<String>,
}

/// Settings for the loose-texture measurement.
///
/// Not a `FixRule` because it is not answerable one BIN at a time: it needs the archive's
/// file list, every BIN's references and the game's contents at once. The parameters
/// still live in config so the threshold can be corrected without a rebuild.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LooseTextureConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Reason id reported when the share is exceeded.
    #[serde(default = "default_loose_texture_reason")]
    pub reason: String,
    /// Below this many textures the sample is too small to be meaningful.
    #[serde(default = "default_min_textures")]
    pub min_textures: usize,
    /// Share that must be EXCEEDED. Exactly this value does not fire.
    #[serde(default = "default_loose_threshold")]
    pub threshold: f32,
    /// Path fragments that mark interface art rather than skin art.
    #[serde(default)]
    pub excluded_segments: Vec<String>,
}

fn default_loose_texture_reason() -> String {
    "loose_textures_unplayable".to_string()
}
fn default_min_textures() -> usize {
    6
}
fn default_loose_threshold() -> f32 {
    0.80
}

impl Default for LooseTextureConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reason: default_loose_texture_reason(),
            min_textures: default_min_textures(),
            threshold: default_loose_threshold(),
            excluded_segments: Vec::new(),
        }
    }
}

impl FixConfig {
    /// Whether the fix with this ID is enabled — the single authority every
    /// pipeline must consult (see `enabled_fixes`).
    pub fn is_fix_enabled(&self, id: &str) -> bool {
        match &self.enabled_fixes {
            Some(list) => list.iter().any(|e| e == id),
            None => self
                .fixes
                .get(id)
                .map(|r| r.enabled)
                .or_else(|| self.wad_fixes.get(id).map(|r| r.enabled))
                .unwrap_or(false),
        }
    }
}

/// Default repath settings stored in `fix_config.json`.
///
/// CLI flags (`--repath`, `--repath-prefix`, `--invis-texture`) always take
/// precedence over these values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepathConfig {
    /// Run repathing automatically even without `--repath` flag.
    /// Set to `true` to make drag-and-drop repath by default.
    #[serde(default)]
    pub enabled: bool,
    /// Prefix inserted after the first "/" of every asset path.
    #[serde(default = "default_repath_prefix")]
    pub prefix: String,
    /// Inject invisible `.tex` placeholders for missing repathed textures.
    #[serde(default)]
    pub invis_texture: bool,
    /// Skip voice-over audio paths (should almost always stay `true`).
    #[serde(default = "default_true")]
    pub skip_vo: bool,
    /// List of rules mapping regex patterns to asset placeholders.
    #[serde(default)]
    pub placeholder_rules: Vec<PlaceholderRule>,
}

/// A rule that associates a regex pattern with a custom asset name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceholderRule {
    pub pattern: String,
    pub asset: String,
}

fn default_repath_prefix() -> String {
    "bum".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for RepathConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            prefix: default_repath_prefix(),
            invis_texture: false,
            skip_vo: true,
            placeholder_rules: Vec::new(),
        }
    }
}

/// A single fix rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixRule {
    pub name: String,
    pub description: String,
    /// Legacy per-rule flag — ignored when the config has `enabled_fixes`.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// How important the FIX is (`critical`/`high`/`medium`/`low`). This is the repair
    /// priority axis. How badly the defect affects the *player* is a separate question,
    /// answered by the reason catalog via [`FixRule::reason`].
    pub severity: String,
    #[serde(default)]
    pub phase: FixPhase,
    /// Reason id (see the `[reasons.*]` catalog) reported when this rule fires in check
    /// mode. Absent means the rule is fix-only and contributes no diagnostic, which is
    /// correct for cosmetic normalisations that are not defects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Report only for a main character, never for a subcharacter form.
    ///
    /// Some properties are shipped by Riot only on the playable champion, and its
    /// summoned forms (a trap, an egg, a minion) legitimately lack them. A rule that
    /// checks such a property fires on every form and reports a defect on the ones that
    /// are correct: a mod with one subcharacter produces dozens of false findings, which
    /// is how a checker teaches people to ignore it.
    ///
    /// Affects reporting only. The transform still runs, so this changes what the user
    /// is told, not what gets repaired.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub main_character_only: bool,
    /// Report only for BINs the mod actually loads.
    ///
    /// A mod built by cloning a champion WAD carries skin BINs it never wires up. Those
    /// are never loaded, so a dead reference inside one cannot crash anything, and
    /// reporting it describes a defect that can never occur.
    ///
    /// Has no effect when load information is unavailable: the gate may only shrink what
    /// is reported, never grow it. Animation BINs are always inspected regardless, since
    /// a clip in one that nothing links is latent rather than absent.
    ///
    /// Affects reporting only; the transform still runs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub loaded_bins_only: bool,
    pub detect: DetectionRule,
    pub apply: TransformAction,
}

/// When in the pipeline a BIN-level rule runs.
///
/// `PostRepath` rules run in a second pass after the repath stage, because
/// they destroy the string paths repath needs to see (e.g. retyping asset
/// path strings into xxh64 `file` hashes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixPhase {
    #[default]
    Standard,
    PostRepath,
}

/// How to detect an issue in a BIN file.
///
/// Uses serde internally-tagged enum: `"type": "missing_or_wrong_field"` etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DetectionRule {
    /// Field is missing or has the wrong value in a specific embed path.
    #[serde(rename = "missing_or_wrong_field")]
    MissingOrWrongField {
        entry_type: String,
        #[serde(default)]
        embed_path: Option<String>,
        #[serde(default)]
        embed_type: Option<String>,
        field: String,
        #[serde(default)]
        expected_value: Option<serde_json::Value>,
    },

    /// A field hash exists at a dot-separated path (e.g. "SamplerValues.*.TextureName").
    #[serde(rename = "field_hash_exists")]
    FieldHashExists { entry_type: String, path: String },

    /// Strings with a given extension that don't exist in the WAD cache.
    #[serde(rename = "string_extension_not_in_wad")]
    StringExtensionNotInWad {
        entry_type: String,
        fields: Vec<String>,
        extension: String,
    },

    /// Recursive scan for strings with extension not in WAD (with path prefix filtering).
    #[serde(rename = "recursive_string_extension_not_in_wad")]
    RecursiveStringExtensionNotInWad {
        extension: String,
        #[serde(default)]
        path_prefixes: Vec<String>,
    },

    /// Any object in the BIN matches one of the given entry types.
    #[serde(rename = "entry_type_exists_any")]
    EntryTypeExistsAny { entry_types: Vec<String> },

    /// BNK audio file version is not in the allowed list.
    #[serde(rename = "bnk_version_not_in")]
    BnkVersionNotIn { allowed_versions: Vec<u32> },

    /// VFX shape data needs migration (post-patch 14.1 format change).
    #[serde(rename = "vfx_shape_needs_fix")]
    VfxShapeNeedsFix { entry_type: String },

    /// Shader references that don't exist in the valid shader list.
    #[serde(rename = "invalid_shader_reference")]
    InvalidShaderReference {
        shader_def_type: String,
        shader_link_field: String,
    },

    /// Entries of specific types not referenced by the main skin entry.
    #[serde(rename = "unreferenced_entry_of_type")]
    UnreferencedEntryOfType {
        main_entry_type: String,
        targets: Vec<EntryValidationTarget>,
    },

    /// Link fields on the main entry reference target entries that are defined
    /// nowhere: not in this tree, not in mod-shipped linked trees, and not in
    /// any game-resolvable `linked:` BIN. The lethal inverse of
    /// `UnreferencedEntryOfType` (e.g. dead GearSkinUpgrade links crash).
    /// With `require_pullable`, only fires when the game's BIN closure can
    /// actually supply a missing entry — unpullable links are left alone
    /// (Topaz semantics: an unresolved CAC link is harmless at runtime).
    #[serde(rename = "dead_entry_link")]
    DeadEntryLink {
        main_entry_type: String,
        targets: Vec<EntryValidationTarget>,
        #[serde(default)]
        require_pullable: bool,
    },

    /// One of the targeted (class, field) pairs still holds a `string` value —
    /// the field was migrated to the xxh64 `file` type by Riot and the mod's
    /// BIN predates the migration.
    #[serde(rename = "class_field_is_string")]
    ClassFieldIsString { targets: Vec<ClassFieldTarget> },

    /// This BIN REPLACES a stock game BIN wholesale, and the replacement's entry set
    /// differs from the vanilla one in a way the client cannot survive.
    ///
    /// Two failure shapes, both from a mod authored against an older client:
    /// - **Dropped**: the replacement omits by-key entries the live client still looks
    ///   up. Unguarded consumers dereference the missing entry and crash. A few classes
    ///   null-check the lookup, so the same drop is only bugged-but-playable there,
    ///   which is why severity is per target rather than per rule.
    /// - **Added**: the replacement carries stale entries of a class whose layout the
    ///   client has since changed, and the UI builder faults walking the old layout.
    ///
    /// Requires a `GameProvider`: without the vanilla BIN to diff against there is
    /// nothing to compare, so the rule fails open.
    #[serde(rename = "replaced_bin_entry_diff")]
    ReplacedBinEntryDiff { targets: Vec<ReplacedBinTarget> },

    /// A material links a shader entry that exists in neither the installed game nor the
    /// mod itself. The engine's resolver returns null for the missing shader and the
    /// game goes down, with no error code recorded, so this cannot be diagnosed from the
    /// client log afterwards.
    ///
    /// Takes no parameters: the valid set is read from the installed game at runtime
    /// rather than configured, because it changes every patch. A shipped list would
    /// either go stale and invent crashes or go missing and disable the check.
    ///
    /// Requires a `GameProvider` that can supply the shader set; without one the rule
    /// reports as skipped rather than clean.
    #[serde(rename = "dead_shader_link")]
    DeadShaderLink,

    /// The mod replaces a character's root record with one older than the live schema.
    ///
    /// Differential, not absolute: a field counts as missing only when the LIVE record
    /// has it and the mod's does not, so a field Riot removes never becomes a finding.
    /// `critical_field` names the one whose absence changes the verdict, because losing
    /// the ability binding makes a champion unplayable while losing a stat field only
    /// makes it wrong.
    #[serde(rename = "stale_character_record")]
    StaleCharacterRecord {
        /// Record class to compare, name or `0x…` hex.
        entry_type: String,
        /// Fields whose absence counts as drift.
        fields: Vec<String>,
        /// Field whose absence reports `critical_reason` instead of the rule's reason.
        #[serde(default)]
        critical_field: Option<String>,
        /// Reason used when `critical_field` is the missing one.
        #[serde(default)]
        critical_reason: Option<String>,
    },

    /// An asset path the BIN names exists in neither the mod nor the base game.
    ///
    /// Distinct from `recursive_string_extension_not_in_wad`, which asks only whether
    /// the MOD ships the file. Most references in a mod point at base-game assets it
    /// deliberately does not duplicate, so for a crash check the mod-only question
    /// reports mostly healthy references and buries the dead ones.
    ///
    /// Requires a `GameProvider`; without one there is no way to separate "the game
    /// ships it" from "nothing ships it", and the rule reports as skipped.
    #[serde(rename = "dead_asset_reference")]
    DeadAssetReference {
        /// Extensions to match, including the dot. Case-insensitive.
        extensions: Vec<String>,
        /// When non-empty, only paths starting with one of these are considered. Leave
        /// empty to also catch repathed mods, which invent prefixes of their own.
        #[serde(default)]
        path_prefixes: Vec<String>,
        /// Character-name fragments whose assets are reported with
        /// [`DetectionRule::DeadAssetReference::downgrade_reason`] instead of the rule's
        /// own reason.
        ///
        /// Minions, turrets and structures load only with the map variant that uses
        /// them, so a missing asset there is conditional rather than certain. Without
        /// this a single themed map mod produces dozens of crash findings for defects
        /// that may never be reached. Deliberately exclude `inhibitor` and `nexus`:
        /// those always load.
        #[serde(default)]
        downgrade_markers: Vec<String>,
        /// Reason used for assets matching `downgrade_markers`. Absent means no
        /// downgrade, and every dead asset reports at the rule's own severity.
        #[serde(default)]
        downgrade_reason: Option<String>,
        /// Reason used when the BIN holding the reference is not loaded as the mod is
        /// used.
        ///
        /// An animation BIN no shipped skin links is not reached in normal play: the
        /// clip is dead only for someone selecting the original skin with the mod
        /// active. Real, but conditional, and calling it a crash overstates it.
        #[serde(default)]
        latent_reason: Option<String>,
        /// Drop dead clips the engine never asks for.
        ///
        /// Two cases look dead and are not: an animation on a mesh particle, which just
        /// renders un-animated, and a clip reachable only through an animation graph the
        /// skin does not link. Both were found by chasing false positives on real mods.
        ///
        /// Only affects `.anm`; nothing else can be in that set. Off by default so a rule
        /// has to opt in.
        #[serde(default)]
        suppress_never_loaded: bool,
    },
}

/// Which direction of entry-set difference is dangerous for one class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplacedBinMode {
    /// Flag keys the mod BIN adds that the vanilla BIN does not have (stale layout).
    Added,
    /// Flag keys the vanilla BIN has that the mod BIN drops (missing on lookup).
    Dropped,
}

/// One class-keyed rule for [`DetectionRule::ReplacedBinEntryDiff`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplacedBinTarget {
    /// Class to compare. Accepts a name (FNV1a-hashed, lowercased) or a `0x…` hex hash.
    pub class: String,
    /// Human label for the class, used in the diagnostic detail when the class is only
    /// known by hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub mode: ReplacedBinMode,
    /// When non-empty, only these entry keys count. Some classes crash only on a handful
    /// of core keys and are harmless for the rest, so flagging every difference would
    /// bury the real signal in noise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lethal_keys: Vec<String>,
    /// Reason reported for this target, overriding the rule's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// How to fix a detected issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TransformAction {
    /// Add or update a field value (optionally creating parent embeds).
    #[serde(rename = "ensure_field")]
    EnsureField {
        field: String,
        value: serde_json::Value,
        data_type: String,
        #[serde(default)]
        create_parent: Option<ParentEmbed>,
    },

    /// Rename a field hash across the BIN tree.
    #[serde(rename = "rename_hash")]
    RenameHash { from_hash: String, to_hash: String },

    /// Replace file extension in all string values (e.g. .dds → .tex).
    #[serde(rename = "replace_string_extension")]
    ReplaceStringExtension {
        from: String,
        to: String,
        #[serde(default)]
        path_prefixes: Vec<String>,
        /// Optional regex on the **field name** carrying the string. When
        /// set, only fields whose resolved name matches the regex are
        /// rewritten. Used to scope HUD-only conversions like
        /// `(?i)iconcircle|iconsquare` so the rule doesn't touch material
        /// or particle textures.
        ///
        /// Fields whose hash can't be resolved (e.g. missing from the
        /// dictionary) are skipped when a filter is set.
        #[serde(default)]
        field_filter: Option<String>,
    },

    /// Mark file for removal from WAD.
    #[serde(rename = "remove_from_wad")]
    RemoveFromWad,

    /// Change a field's value type (e.g. vec3 → vec4, link → string).
    #[serde(rename = "change_field_type")]
    ChangeFieldType {
        from_type: String,
        to_type: String,
        #[serde(default)]
        conversion_rule: Option<String>,
        #[serde(default)]
        append_values: Vec<serde_json::Value>,
    },

    /// Regex-based string replacement.
    #[serde(rename = "regex_replace")]
    RegexReplace {
        pattern: String,
        replacement: String,
        #[serde(default)]
        field_filter: Option<String>,
    },

    /// Regex-based field rename with capture group support.
    #[serde(rename = "regex_rename_field")]
    RegexRenameField {
        pattern: String,
        replacement: String,
    },

    /// Complex VFX shape structure migration.
    #[serde(rename = "vfx_shape_fix")]
    VfxShapeFix,

    /// Replace invalid shader references with closest valid match.
    #[serde(rename = "shader_fallback")]
    ShaderFallback {
        shader_def_type: String,
        shader_link_field: String,
    },

    /// Remove entries not referenced by the main skin entry.
    #[serde(rename = "remove_unreferenced_entries")]
    RemoveUnreferencedEntries {
        main_entry_type: String,
        targets: Vec<EntryValidationTarget>,
    },

    /// Move every object whose class name is in `entry_types` out of the
    /// source BIN and into a brand-new BIN written at
    /// `output_path_template`. Powers VFX separation (split
    /// `VfxSystemDefinitionData` entries into `{champ}_vfx_skin{N}.bin`)
    /// and similar object-extraction fixes.
    ///
    /// `output_path_template` supports a small set of substitutions
    /// resolved from the source file's path (see
    /// [`split_entries::resolve_template`]):
    ///
    /// * `{source_dir}` — directory of the source path (no trailing `/`)
    /// * `{source_stem}` — source filename without extension
    /// * `{source_ext}` — source extension (no leading dot)
    /// * `{champion}` — champion folder from `data/characters/{X}/...` (lowercased)
    /// * `{skin}` — first integer in the source stem (e.g. `0` for `skin0`)
    ///
    /// When `link_in_source` is true the new BIN's path is appended to
    /// the source's linked-deps list so the engine resolves both files
    /// together.
    #[serde(rename = "split_entries_by_type")]
    SplitEntriesByType {
        /// Class names whose objects get moved into the new BIN.
        entry_types: Vec<String>,
        /// Path template for the new BIN (see above for substitutions).
        output_path_template: String,
        /// Add `output_path_template` to `source.linked` after the split.
        #[serde(default = "default_true")]
        link_in_source: bool,
    },

    /// Merge linked spell/anim BINs into the champion's main BIN.
    #[serde(rename = "merge_linked_bins")]
    MergeLinkedBins,

    /// Pull referenced-but-missing target entries out of the live game's BIN
    /// closure and inject them into this tree. Unpullable links either nuke
    /// a fallback field on the main entry (gear: "skinUpgradeData") or drop
    /// the dead link value (CAC).
    #[serde(rename = "pull_entries_from_game")]
    PullEntriesFromGame {
        main_entry_type: String,
        targets: Vec<EntryValidationTarget>,
        /// Field (by name) on the main entry to REMOVE when a target link
        /// cannot be pulled. `None` = drop only the dead link value from
        /// its container.
        #[serde(default)]
        nuke_fallback_field: Option<String>,
    },

    /// Rewrite dead asset-path strings to a live form, consulting both the mod
    /// WAD and the live game index. Ladder per string: exact-in-mod → skip;
    /// exact-in-game → skip; ext-twin in mod → rewrite; ext-twin in game →
    /// rewrite; inner-suffix-strip in game → rewrite; strip+twin in game →
    /// rewrite. No-op without a game provider.
    #[serde(rename = "resolve_dead_refs")]
    ResolveDeadRefs {
        /// Extensions to consider (no leading dot), e.g. ["dds","tex","anm","skn","skl","scb","sco"].
        extensions: Vec<String>,
    },

    /// Convert `string` values to xxh64 `file` hashes on the targeted
    /// (class, field) pairs — Riot's asset-reference type migration. Handles
    /// plain fields plus `option[string]`/`list[string]` and map values. Each
    /// converted path's `hash → path` pair is recorded in the BIN's trailer
    /// side table so the readable path survives the retype. Rules using this
    /// action must set `"phase": "post_repath"`.
    #[serde(rename = "retype_string_to_file")]
    RetypeStringToFile { targets: Vec<ClassFieldTarget> },

    /// Report the defect and change nothing.
    ///
    /// Some crash classes have no safe automatic repair: an outdated map geometry
    /// format needs a real format migration, and a replaced UI bin that dropped entries
    /// needs the author to put them back. Before this existed every rule had to claim a
    /// fix, which forced unfixable defects to either be left undetected or be given a
    /// sham transform that reports success without repairing anything. Detection is
    /// valuable on its own: warning the player is the whole point of a crash check.
    #[serde(rename = "report_only")]
    ReportOnly,
}

/// A (class, field) pair targeted by type-migration rules. Both accept a
/// name (FNV1a-hashed, lowercased) or a `0x…` hex hash.
///
/// ## Why the (class, field) pairing matters
/// The migration is **per property, not per file extension**, and the same field name
/// can migrate in one class and stay a string in another: `texture` is a string in
/// VfxSystem/particle classes but a file reference in mesh classes. Keying on the field
/// hash alone cannot represent that and corrupts VFX; keying on the pair does.
///
/// ## Per-target reporting
/// A single migration rule covers many properties whose defects differ in kind: an
/// unreadable animation path is a crash, an unresolved HUD asset merely renders missing.
/// [`ClassFieldTarget::reason`] lets one rule report both, since each reason carries its
/// own severity in the catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassFieldTarget {
    pub class: String,
    pub field: String,
    /// Reason id reported for this target specifically, overriding the rule's own
    /// `reason`. Absent means inherit from the rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Parent embed to create when EnsureField target doesn't exist yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParentEmbed {
    pub field: String,
    #[serde(rename = "type")]
    pub embed_type: String,
}

/// Target entry type for entry validation rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryValidationTarget {
    /// The entry type to validate (e.g. "ContextualActionData").
    pub entry_type: String,
    /// Optional hex type hash for direct matching (e.g. "0xCF3A2F44").
    #[serde(default)]
    pub type_hash: Option<String>,
    /// Field name in the main entry that references this type.
    pub reference_field: String,
    /// Hash of the link field (hex string like "0xd8f64a0d").
    pub link_field: String,
}

// ============================================================================
// WAD-LEVEL FIXES (File operations before BIN parsing)
// ============================================================================

/// A WAD-level fix rule for file operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WadFixRule {
    pub name: String,
    pub description: String,
    /// Legacy per-rule flag — ignored when the config has `enabled_fixes`.
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub severity: String,
    /// Reason id (see the `[reasons.*]` catalog) reported when this rule fires in check
    /// mode. Absent means fix-only.
    ///
    /// WAD-level rules need this as much as BIN-level ones: a binless mod that ships a
    /// single malformed texture has no BIN for a tree rule to inspect, so a WAD rule is
    /// the *only* thing standing between the player and a crash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Path fragments that route a file to [`WadFixRule::alternate_reason`].
    ///
    /// The same malformed file means different things depending on what it is. A
    /// block-misaligned texture on a champion faults on load; the identical fault on a
    /// summoner emote only faults when the emote is triggered, which is a different
    /// severity and a different sentence to show the player.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternate_path_markers: Vec<String>,
    /// Reason used for files matching `alternate_path_markers`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternate_reason: Option<String>,
    pub detect: WadDetectionRule,
    pub apply: WadTransformAction,
}

/// How to detect issues at the WAD file level.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WadDetectionRule {
    /// Match files by extension and optionally check binary headers.
    #[serde(rename = "file_extension")]
    FileExtension {
        extension: String,
        #[serde(default)]
        binary_check: Option<BinaryHeaderCheck>,
        /// List of filenames to exclude (e.g., ["sfx_events.bnk"])
        #[serde(default)]
        exclude_files: Vec<String>,
    },

    /// Match files by path pattern (glob-style).
    #[serde(rename = "file_pattern")]
    FilePattern {
        pattern: String,
        #[serde(default)]
        binary_check: Option<BinaryHeaderCheck>,
    },

    /// Always matches — every file in the WAD is a candidate. Used
    /// almost exclusively as a trigger for WAD-level actions that don't
    /// care about a specific input file (e.g. `add_files`). The pipeline
    /// short-circuits the per-file loop for actions that operate on the
    /// WAD as a whole, so this rule only fires once.
    #[serde(rename = "always")]
    Always,
}

/// Binary header validation for file format checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BinaryHeaderCheck {
    /// Check version number at specific offset.
    #[serde(rename = "version_at_offset")]
    VersionAtOffset {
        /// Byte offset in file
        offset: usize,
        /// Size in bytes (1, 2, or 4)
        size: usize,
        /// Byte order
        #[serde(default = "default_endian")]
        endian: Endian,
        /// List of allowed versions
        allowed_versions: Vec<u32>,
    },

    /// Check magic signature at start of file.
    #[serde(rename = "magic_signature")]
    MagicSignature {
        /// Expected bytes at start of file
        signature: Vec<u8>,
    },

    /// A block-compressed texture whose dimensions are not a multiple of the block size.
    ///
    /// Block compression stores fixed-size blocks of pixels, so a texture that is not a
    /// whole number of blocks across has no valid encoding. The engine sizes the payload
    /// from the dimensions and faults on upload.
    ///
    /// Only block-compressed formats are affected; an uncompressed texture of any size is
    /// fine, which is why the format byte gates the check.
    #[serde(rename = "block_alignment")]
    BlockAlignment {
        /// Expected bytes at the start of the file.
        magic: Vec<u8>,
        /// Offset of the `u16` little-endian width.
        width_offset: usize,
        /// Offset of the `u16` little-endian height.
        height_offset: usize,
        /// Offset of the format byte.
        format_offset: usize,
        /// Format values that are block-compressed. Anything else is not checked.
        block_formats: Vec<u8>,
        /// Block size in pixels.
        #[serde(default = "default_block_size")]
        block_size: u32,
    },
}

fn default_block_size() -> u32 {
    4
}

fn default_endian() -> Endian {
    Endian::Little
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Endian {
    Little,
    Big,
}

/// How to transform files at the WAD level.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WadTransformAction {
    /// Remove the file from WAD. With `unless_referenced`, files the mod's
    /// own BINs still point at (by path string OR xxh64 `file` hash) are
    /// kept — only unreferenced bloat is removed.
    #[serde(rename = "remove_file")]
    RemoveFile {
        #[serde(default)]
        unless_referenced: bool,
    },

    /// Convert file format (e.g. DDS→TEX, SCO→SCB).
    #[serde(rename = "convert_format")]
    ConvertFormat {
        /// Source extension
        from_ext: String,
        /// Target extension
        to_ext: String,
        /// Converter name (must be registered in converter registry)
        converter: String,
    },

    /// Rename file (change path/extension).
    #[serde(rename = "rename_file")]
    RenameFile {
        /// Regex pattern to match
        pattern: String,
        /// Replacement string (supports $1, $2 capture groups)
        replacement: String,
    },

    /// Apply an in-place byte transform to a matched file. Path and
    /// extension are preserved; only the contents change. Used for
    /// operations like mipmap stripping and TEX dimension fixes that
    /// don't produce a renamed output.
    #[serde(rename = "transform_bytes")]
    TransformBytes {
        /// Converter name (must be registered in the converter registry).
        /// The same registry serves [`Self::ConvertFormat`].
        converter: String,
    },

    /// Inject standalone files into the WAD. Used for fallback texture
    /// registries and similar "always present" assets. The `assets`
    /// list is materialised via the asset registry — a path here maps
    /// to embedded bytes inside `hematite-core`.
    ///
    /// Detection is intentionally not enforced for this action; a rule
    /// using `add_files` typically pairs with a `file_pattern` detection
    /// that always matches (e.g. matches the WAD's existence) so the
    /// pipeline only emits the assets once.
    #[serde(rename = "add_files")]
    AddFiles {
        /// Logical asset names → target WAD paths. The name is looked
        /// up in the embedded asset registry (see
        /// `hematite-core/src/assets/registry.rs`).
        assets: Vec<AssetInjection>,
    },
}

/// One entry in a `WadTransformAction::AddFiles` action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetInjection {
    /// Name of the embedded asset (e.g. `"invis_tex"`, `"toonshading_tex"`).
    pub asset: String,
    /// WAD path the asset bytes should appear at. Path-hashed via xxh64
    /// when written.
    pub path: String,
    /// Only inject when the WAD doesn't already contain `path`.
    /// `true` is the safe default — never overwrite an existing file.
    #[serde(default = "default_true")]
    pub only_if_missing: bool,
}

/// All BIN data types for value creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinDataType {
    Bool,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F32,
    Vector2,
    Vector3,
    Vector4,
    String,
    Hash,
    Link,
    Color,
}
