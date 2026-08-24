//! Trait abstractions for external dependencies.
//!
//! The fix engine operates exclusively against these traits. Implementations
//! live in `hematite-file` (or any future adapter crate).
//!
//! ## Design rationale
//! - `BinProvider`: Wraps BIN parsing/serialization. When rs_bin changes its BinTree API,
//!   only the adapter implementation changes.
//! - `HashProvider`: Wraps hash dictionary loading (LMDB or txt files).
//!   Reverse lookups (name → hash) are required for the fix engine.
//! - `WadProvider`: Wraps WAD path lookups. The fix engine only asks "does this path
//!   exist?" — it never reads WAD chunk data directly.

use anyhow::Result;
use hematite_types::bin::BinTree;
use hematite_types::hash::{FieldHash, GameHash, PathHash, TypeHash};

/// Abstraction over BIN file parsing and serialization.
///
/// Implementors handle the actual format (LTK today, something else tomorrow).
pub trait BinProvider: Send + Sync {
    /// Parse BIN from raw bytes into Hematite's domain types.
    fn parse_bytes(&self, data: &[u8]) -> Result<BinTree>;

    /// Serialize a BinTree back to bytes for writing.
    fn write_bytes(&self, tree: &BinTree) -> Result<Vec<u8>>;
}

/// Abstraction over hash dictionary loading.
///
/// Implementations can read from txt files, lmdb, embedded data, or network.
/// All reverse lookups (name → hash) must be pre-computed at load time for O(1) access.
pub trait HashProvider: Send + Sync {
    /// Resolve a class hash to its type name (e.g. 0xABCD → "SkinCharacterDataProperties").
    ///
    /// Owned rather than borrowed so an implementation can answer from the database on
    /// demand instead of holding every name in memory. The dictionary is millions of
    /// entries and a run touches a tiny fraction of it, so preloading was over a second
    /// of startup on every invocation. Every caller copied the result anyway.
    fn resolve_type(&self, hash: TypeHash) -> Option<String>;

    /// Resolve a field hash to its field name (e.g. 0x1234 → "UnitHealthBarStyle").
    fn resolve_field(&self, hash: FieldHash) -> Option<String>;

    /// Resolve an entry path hash to its path string.
    fn resolve_entry(&self, hash: PathHash) -> Option<String>;

    /// Resolve a game asset hash (xxhash64) to its path.
    fn resolve_game_path(&self, hash: GameHash) -> Option<String>;

    /// Reverse lookup: type name → type hash.
    fn type_hash(&self, name: &str) -> Option<TypeHash>;

    /// Reverse lookup: field name → field hash.
    fn field_hash(&self, name: &str) -> Option<FieldHash>;

    /// Check if a game asset path exists in the hash dictionary.
    ///
    /// Computes the xxhash64 of the path and checks if it's in the loaded hashes.
    /// Returns false if the path is not a known game asset (likely custom/repathed).
    fn has_game_path(&self, path: &str) -> bool;

    /// Whether any hashes are loaded (false if dictionary is empty/missing).
    fn is_loaded(&self) -> bool;
}

/// Abstraction over WAD file path lookups.
///
/// The fix engine uses this to check if assets exist in the mod's WAD file.
/// This prevents false positives (e.g. don't convert .dds→.tex if the .dds exists).
pub trait WadProvider: Send + Sync {
    /// Check if a file path exists in the WAD (hashes the path internally).
    fn has_path(&self, path: &str) -> bool;

    /// Check if a raw xxhash64 exists in the WAD.
    fn has_hash(&self, hash: u64) -> bool;
}

/// Abstraction over live base-game file access. Implementations wrap an
/// installed League of Legends client (see hematite-live) and are internally
/// mutable (&self methods) so they can share a FixContext with other borrows.
pub trait GameProvider: Send + Sync {
    /// Does the base game ship this path?
    fn has_path(&self, path: &str) -> bool;
    /// Raw bytes of a game file (None when absent or unreadable).
    fn pull_raw(&self, path: &str) -> Option<Vec<u8>>;
    /// Pull AND parse a game BIN into hematite's tree model.
    /// (Parsing happens inside the impl — core stays format-free.)
    ///
    /// Shared rather than owned so implementations can memoise. Resolving a dead link
    /// walks up to 64 game BINs, and that walk repeats for every BIN in the mod, so the
    /// same handful of game files were being decompressed and parsed hundreds of times
    /// per run. Every caller only reads the tree.
    fn game_bin(&self, path: &str) -> Option<std::sync::Arc<BinTree>>;

    /// Entry-key hashes of every shader definition the installed game ships.
    ///
    /// A material links its shader by entry key, so this is exactly the set of link
    /// targets that resolve at load time. A link to anything else makes the engine's
    /// resolver return null and takes the game down with it.
    ///
    /// `None` means the shader data could not be read, which callers MUST treat as
    /// "cannot validate" and never as "no shaders exist": an empty set would mark every
    /// shader link in every mod dead.
    ///
    /// Defaults to `None` so a provider that cannot supply it simply does not support
    /// the check rather than silently failing it.
    fn shader_defs(&self) -> Option<std::sync::Arc<std::collections::HashSet<u32>>> {
        None
    }
}

#[cfg(test)]
mod game_provider_tests {
    use super::*;

    struct NullGame;
    impl GameProvider for NullGame {
        fn has_path(&self, _p: &str) -> bool {
            false
        }
        fn pull_raw(&self, _p: &str) -> Option<Vec<u8>> {
            None
        }
        fn game_bin(&self, _p: &str) -> Option<std::sync::Arc<BinTree>> {
            None
        }
    }

    #[test]
    fn game_provider_is_object_safe() {
        let g: &dyn GameProvider = &NullGame;
        assert!(!g.has_path("data/x.bin"));
        assert!(g.pull_raw("x").is_none());
        assert!(g.game_bin("x").is_none());
    }
}
