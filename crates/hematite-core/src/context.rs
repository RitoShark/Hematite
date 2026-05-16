//! Fix context — runtime state for a fix session.
//!
//! `FixContext` bundles together everything a detection rule or transform action
//! needs: the BIN tree being processed, hash lookups, WAD existence checks,
//! and champion relationship data.

use crate::detect::shader::ShaderValidator;
use crate::traits::{HashProvider, WadProvider};
use hematite_types::bin::BinTree;
use hematite_types::champion::CharacterRelations;
use std::collections::HashMap;

/// Runtime state for a fix session on a single BIN file.
///
/// Passed to detection rules and transform actions. The BIN tree is mutable
/// so transforms can modify it in-place.
pub struct FixContext<'a> {
    /// The BIN tree being processed (mutable for transforms).
    pub tree: BinTree,

    /// Hash dictionary for name ↔ hash resolution.
    pub hashes: &'a dyn HashProvider,

    /// WAD cache for asset existence checks.
    pub wad: &'a dyn WadProvider,

    /// Champion → subchamp relationships.
    pub champions: &'a CharacterRelations,

    /// Path of the current file being processed (for logging/context).
    pub file_path: String,

    /// Files marked for removal from the WAD (populated by RemoveFromWad transforms).
    pub files_to_remove: Vec<String>,

    /// Linked BIN trees resolved via BFS (dependencies from BIN headers).
    pub linked_trees: HashMap<String, BinTree>,

    /// Shader validator for shader fallback fixes (optional).
    pub shader_validator: Option<&'a ShaderValidator>,

    /// Additional BIN files produced by this fix session. Populated by
    /// transforms that split entries out of the source BIN into their own
    /// `(path, tree)` pair (e.g. VFX separation). Consumed by the WAD
    /// rebuild step in the caller — the pipeline itself just collects.
    pub additional_bins: Vec<(String, BinTree)>,
}
