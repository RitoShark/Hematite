//! Hash dictionary loading from CDragon txt files.

use anyhow::{Context, Result};
use hematite_core::traits::HashProvider;
use hematite_types::hash::{FieldHash, GameHash, PathHash, TypeHash};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

/// Hash provider backed by CDragon txt files.
pub struct TxtHashProvider {
    /// class_hash → type name
    types: HashMap<u32, String>,
    /// field_hash → field name
    fields: HashMap<u32, String>,
    /// path_hash → entry path
    entries: HashMap<u32, String>,
    /// game_hash → asset path
    game_paths: HashMap<u64, String>,

    // Reverse maps (pre-computed at load time)
    /// type name (lowercase) → class_hash
    type_name_to_hash: HashMap<String, TypeHash>,
    /// field name (lowercase) → field_hash
    field_name_to_hash: HashMap<String, FieldHash>,
}

impl TxtHashProvider {
    /// Create empty hash provider.
    pub fn new() -> Self {
        Self {
            types: HashMap::new(),
            fields: HashMap::new(),
            entries: HashMap::new(),
            game_paths: HashMap::new(),
            type_name_to_hash: HashMap::new(),
            field_name_to_hash: HashMap::new(),
        }
    }

    /// Get the RitoShark hash directory path.
    pub fn get_hash_dir() -> Result<PathBuf> {
        let appdata = std::env::var("APPDATA").context("APPDATA environment variable not set")?;
        Ok(PathBuf::from(appdata)
            .join("RitoShark")
            .join("Requirements")
            .join("Hashes"))
    }

    /// Load hash dictionaries from the standard RitoShark directory.
    /// Returns Ok with empty dicts if files don't exist (graceful fallback).
    pub fn load_from_appdata() -> Result<Self> {
        let hash_dir = Self::get_hash_dir()?;

        if !hash_dir.exists() {
            return Ok(Self::new());
        }

        let mut provider = Self::new();

        let types_file = hash_dir.join("hashes.bintypes.txt");
        if types_file.exists() {
            provider.types = load_u32_hash_file(&types_file).context("Failed to load bintypes")?;
        }

        let fields_file = hash_dir.join("hashes.binfields.txt");
        if fields_file.exists() {
            provider.fields =
                load_u32_hash_file(&fields_file).context("Failed to load binfields")?;
        }

        let entries_file = hash_dir.join("hashes.binentries.txt");
        if entries_file.exists() {
            provider.entries =
                load_u32_hash_file(&entries_file).context("Failed to load binentries")?;
        }

        let game_file = hash_dir.join("hashes.game.txt");
        if game_file.exists() {
            provider.game_paths =
                load_u64_hash_file(&game_file).context("Failed to load game hashes")?;
        }

        provider.type_name_to_hash = provider
            .types
            .iter()
            .map(|(hash, name)| (name.to_lowercase(), TypeHash(*hash)))
            .collect();

        provider.field_name_to_hash = provider
            .fields
            .iter()
            .map(|(hash, name)| (name.to_lowercase(), FieldHash(*hash)))
            .collect();

        Ok(provider)
    }
}

impl Default for TxtHashProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl HashProvider for TxtHashProvider {
    fn resolve_type(&self, hash: TypeHash) -> Option<&str> {
        self.types.get(&hash.0).map(|s| s.as_str())
    }

    fn resolve_field(&self, hash: FieldHash) -> Option<&str> {
        self.fields.get(&hash.0).map(|s| s.as_str())
    }

    fn resolve_entry(&self, hash: PathHash) -> Option<&str> {
        self.entries.get(&hash.0).map(|s| s.as_str())
    }

    fn resolve_game_path(&self, hash: GameHash) -> Option<&str> {
        self.game_paths.get(&hash.0).map(|s| s.as_str())
    }

    fn type_hash(&self, name: &str) -> Option<TypeHash> {
        self.type_name_to_hash.get(&name.to_lowercase()).copied()
    }

    fn field_hash(&self, name: &str) -> Option<FieldHash> {
        self.field_name_to_hash.get(&name.to_lowercase()).copied()
    }

    fn has_game_path(&self, path: &str) -> bool {
        use xxhash_rust::xxh64::xxh64;

        let normalized = path.to_lowercase().replace('\\', "/");
        let hash = xxh64(normalized.as_bytes(), 0);
        self.game_paths.contains_key(&hash)
    }

    fn is_loaded(&self) -> bool {
        !self.types.is_empty() || !self.fields.is_empty()
    }
}

/// Load a hash file with u32 hashes (format: "<hex_hash> <name>").
fn load_u32_hash_file(path: &PathBuf) -> Result<HashMap<u32, String>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut map = HashMap::new();

    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Format: "<hex_hash> <name>" or "<hex_hash>\t<name>"
        if let Some((hash_str, name)) = line.split_once([' ', '\t']) {
            let hash_str = hash_str.trim_start_matches("0x");
            if let Ok(hash) = u32::from_str_radix(hash_str, 16) {
                map.insert(hash, name.to_string());
            }
        }
    }

    Ok(map)
}

/// Load a hash file with u64 hashes (format: "<hex_hash> <name>").
fn load_u64_hash_file(path: &PathBuf) -> Result<HashMap<u64, String>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut map = HashMap::new();

    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Format: "<hex_hash> <name>" or "<hex_hash>\t<name>"
        if let Some((hash_str, name)) = line.split_once([' ', '\t']) {
            let hash_str = hash_str.trim_start_matches("0x");
            if let Ok(hash) = u64::from_str_radix(hash_str, 16) {
                map.insert(hash, name.to_string());
            }
        }
    }

    Ok(map)
}
