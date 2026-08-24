//! Fix context — runtime state for a fix session.
//!
//! `FixContext` bundles together everything a detection rule or transform action
//! needs: the BIN tree being processed, hash lookups, WAD existence checks,
//! and champion relationship data.

use crate::detect::shader::ShaderValidator;
use crate::traits::{GameProvider, HashProvider, WadProvider};
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

    /// Live base-game file access for pull fixes (optional). `None` means
    /// fail open / skip any fix that depends on live game data.
    pub game: Option<&'a dyn GameProvider>,

    /// Additional BIN files produced by this fix session. Populated by
    /// transforms that split entries out of the source BIN into their own
    /// `(path, tree)` pair (e.g. VFX separation). Consumed by the WAD
    /// rebuild step in the caller — the pipeline itself just collects.
    pub additional_bins: Vec<(String, BinTree)>,

    /// Where this BIN sits in the mod, for rules that only apply to loaded BINs.
    ///
    /// Defaults to "unknown", which every consumer must read as "assume it loads".
    pub scope: BinScope<'a>,
}

/// Where the BIN under inspection sits in the mod's load graph.
///
/// Bundled rather than added as loose fields so the many `FixContext` constructions that
/// have no scope information stay a one-line `..Default::default()` rather than each
/// having to invent values they cannot know.
#[derive(Default, Clone, Copy)]
pub struct BinScope<'a> {
    /// Chunk path hash of this BIN.
    ///
    /// Carried explicitly because a repathed mod's BINs have custom paths no dictionary
    /// resolves, so `file_path` is often a bare hash string that cannot be re-hashed to
    /// recover this.
    pub chunk_hash: u64,

    /// What the mod actually loads, when it could be determined.
    ///
    /// `None` means it could not be, and consumers must then behave as if everything
    /// loads. This may only ever shrink what is considered, never grow it, and never by
    /// itself produce a finding.
    pub reachable: Option<&'a crate::reachability::Reachability>,

    /// Animation-BIN chunk hash to slot, for the mod's characters.
    ///
    /// Animation BINs are never reachability roots, so the gate would drop them. They
    /// have to be inspected anyway: a clip in an animation BIN that no shipped skin
    /// links is latent rather than fatal, and that distinction is invisible if the BIN
    /// is never looked at.
    pub animation_bins: Option<&'a std::collections::HashMap<u64, u32>>,
}

impl<'a> BinScope<'a> {
    /// Whether this BIN should be inspected under the given scope.
    ///
    /// Unknown reachability means yes. So does being an animation BIN, which is the
    /// carve-out that keeps latent findings visible.
    pub fn should_inspect(&self, require_reachable: bool) -> bool {
        if !require_reachable {
            return true;
        }
        let Some(reach) = self.reachable else {
            return true;
        };
        if self.is_animation_bin() {
            return true;
        }
        reach.loads(self.chunk_hash)
    }

    /// Whether this BIN is one of the mod's animation BINs.
    pub fn is_animation_bin(&self) -> bool {
        self.animation_bins
            .is_some_and(|m| m.contains_key(&self.chunk_hash))
    }

    /// Slot this BIN is the skin root of, if any.
    pub fn skin_slot(&self) -> Option<u32> {
        self.reachable.and_then(|r| r.slot_of(self.chunk_hash))
    }

    /// Slot of the animation BIN this is, if it is one.
    pub fn animation_slot(&self) -> Option<u32> {
        self.animation_bins
            .and_then(|m| m.get(&self.chunk_hash).copied())
    }
}
