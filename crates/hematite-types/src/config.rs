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
    /// BIN-level fixes (operate on parsed BIN trees)
    pub fixes: HashMap<String, FixRule>,
    /// WAD-level fixes (operate on files before BIN parsing)
    #[serde(default)]
    pub wad_fixes: HashMap<String, WadFixRule>,
    /// Default repath settings (can be overridden by CLI flags).
    /// When `enabled` is true, drag-and-drop runs repathing automatically.
    #[serde(default)]
    pub repath: RepathConfig,
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
        }
    }
}

/// A single fix rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixRule {
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub severity: String,
    pub detect: DetectionRule,
    pub apply: TransformAction,
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
    pub enabled: bool,
    pub severity: String,
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
    /// Remove the file from WAD.
    #[serde(rename = "remove_file")]
    RemoveFile,

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
