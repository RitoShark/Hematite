//! Hash dictionary loading from LMDB database.
//!
//! ## Schema (current — produced by `RitoShark/lmdb-hashes`)
//! A single LMDB environment with **two** named databases:
//!
//! | DB     | Key                       | Value                          |
//! |--------|---------------------------|--------------------------------|
//! | `wad`  | `u64` xxhash64 (BE bytes) | game asset path                |
//! | `bin`  | `u32` FNV1a   (BE bytes) | type / field / entry / generic |
//!
//! The `bin` database is a **merged** view of every `hashes.bin*.txt`
//! source. Riot's FNV1a hash is the same algorithm for every bin-side
//! namespace (class names, field names, entry paths, generic strings)
//! so collisions across categories are vanishingly rare and the
//! consumer doesn't need to know which category a hash came from —
//! the same key always resolves to the same name.
//!
//! ## Legacy schema (still supported via fallback)
//! Older cached databases used four named DBs (`wad`, `types`,
//! `fields`, `entries`) — when [`Self::load_from_path`] doesn't find
//! a `bin` DB it falls back to opening those and merging them
//! in-process. New downloads use the current schema.

use anyhow::{Context, Result};
use heed::types::{Bytes, Str};
use heed::{Database, Env, EnvOpenOptions, RoTxn};
use hematite_core::traits::HashProvider;
use hematite_types::hash::{FieldHash, GameHash, PathHash, TypeHash};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Hash provider backed by LMDB database.
///
/// Loads all hashes into memory at startup for O(1) lookups. The
/// in-memory state is intentionally schema-agnostic: a single `bin`
/// map covers types / fields / entries / generic, mirroring the
/// upstream merged-database design.
pub struct LmdbHashProvider {
    /// u64 xxhash64 → game asset path.
    game_paths: HashMap<u64, String>,
    /// u32 FNV1a → name (type / field / entry / generic — merged).
    bin_names: HashMap<u32, String>,
    /// Lower-cased name → hash (single reverse map serves both
    /// `type_hash` and `field_hash`).
    bin_name_to_hash: HashMap<String, u32>,
}

impl LmdbHashProvider {
    /// Get the on-disk LMDB directory path under `%APPDATA%`.
    pub fn get_hash_path() -> Result<PathBuf> {
        let appdata = std::env::var("APPDATA").context("APPDATA environment variable not set")?;
        Ok(PathBuf::from(appdata)
            .join("RitoShark")
            .join("Requirements")
            .join("Hashes")
            .join("hashes.lmdb"))
    }

    /// Load hash dictionaries from the standard install directory.
    pub fn load_from_appdata() -> Result<Self> {
        let lmdb_path = Self::get_hash_path()?;
        if !lmdb_path.exists() {
            anyhow::bail!("LMDB hash file not found: {}", lmdb_path.display());
        }
        Self::load_from_path(&lmdb_path)
    }

    /// Load hash dictionaries from a specific LMDB directory.
    pub fn load_from_path(lmdb_dir: &Path) -> Result<Self> {
        tracing::info!("Loading LMDB hashes from: {}", lmdb_dir.display());

        let env = open_env(lmdb_dir)?;
        let rtxn = env.read_txn().context("Failed to start read transaction")?;

        let game_paths = load_wad_db(&env, &rtxn)?;
        let bin_names = load_bin_db(&env, &rtxn)
            .or_else(|new_err| {
                // No `bin` DB → user might have a legacy 4-database
                // cache. Try opening the old schema and merging it
                // into the same in-memory shape.
                tracing::debug!(
                    "No 'bin' database (likely legacy schema); falling back to types/fields/entries — {}",
                    new_err
                );
                load_legacy_bin_dbs(&env, &rtxn)
            })
            .context(
                "Failed to load BIN hashes from either the current ('bin') \
                 or legacy ('types' + 'fields' + 'entries') schema. \
                 Delete %APPDATA%\\RitoShark\\Requirements\\Hashes\\hashes.lmdb \
                 and re-run to redownload.",
            )?;

        rtxn.commit().context("Failed to commit read transaction")?;

        let bin_name_to_hash = bin_names
            .iter()
            .map(|(hash, name)| (name.to_lowercase(), *hash))
            .collect();

        tracing::info!(
            "Loaded LMDB hashes: {} game paths, {} BIN names",
            game_paths.len(),
            bin_names.len()
        );

        Ok(Self {
            game_paths,
            bin_names,
            bin_name_to_hash,
        })
    }
}

// ---------------------------------------------------------------------------
// LMDB plumbing
// ---------------------------------------------------------------------------

fn open_env(lmdb_dir: &Path) -> Result<Env> {
    // map_size must be page-aligned AND large enough for the data
    // file. We snap to the actual data.mdb size + 25% headroom, with
    // a 100 MB floor.
    let data_mdb = lmdb_dir.join("data.mdb");
    let page = page_size::get();
    let map_size = if data_mdb.exists() {
        let file_size = std::fs::metadata(&data_mdb)
            .map(|m| m.len() as usize)
            .unwrap_or(0);
        let min_size = 100 * 1024 * 1024;
        let raw = std::cmp::max(file_size + file_size / 4, min_size);
        raw.div_ceil(page) * page
    } else {
        1024 * 1024 * 1024
    };

    // `max_dbs(4)` accommodates both the new 2-db schema and the
    // legacy 4-db schema without reopening the env.
    let mut opts = EnvOpenOptions::new();
    opts.max_dbs(4);
    opts.map_size(map_size);

    unsafe { opts.open(lmdb_dir) }.context("Failed to open LMDB environment")
}

fn load_wad_db(env: &Env, rtxn: &RoTxn<'_>) -> Result<HashMap<u64, String>> {
    let wad_db: Database<Bytes, Str> = env
        .open_database(rtxn, Some("wad"))
        .context("Failed to query 'wad' database")?
        .context("'wad' database not found — LMDB doesn't look like a valid hashes bundle")?;

    let mut out = HashMap::new();
    for item in wad_db
        .iter(rtxn)
        .context("Failed to iterate 'wad' database")?
    {
        let (key_bytes, name) = item.context("Failed to read 'wad' entry")?;
        if let Some(hash) = read_u64_be(key_bytes) {
            out.insert(hash, name.to_string());
        }
    }
    Ok(out)
}

/// Load the current-schema merged `bin` database.
fn load_bin_db(env: &Env, rtxn: &RoTxn<'_>) -> Result<HashMap<u32, String>> {
    let bin_db: Database<Bytes, Str> = env
        .open_database(rtxn, Some("bin"))
        .context("Failed to query 'bin' database")?
        .context("'bin' database not found")?;

    let mut out = HashMap::new();
    for item in bin_db
        .iter(rtxn)
        .context("Failed to iterate 'bin' database")?
    {
        let (key_bytes, name) = item.context("Failed to read 'bin' entry")?;
        if let Some(hash) = read_u32_be(key_bytes) {
            out.insert(hash, name.to_string());
        }
    }
    Ok(out)
}

/// Load the legacy schema (`types` + `fields` + `entries`) and merge
/// it into the same map shape as the new `bin` database.
fn load_legacy_bin_dbs(env: &Env, rtxn: &RoTxn<'_>) -> Result<HashMap<u32, String>> {
    let mut out = HashMap::new();
    let mut any_found = false;

    for db_name in &["types", "fields", "entries"] {
        let db: Option<Database<Bytes, Str>> = env
            .open_database(rtxn, Some(db_name))
            .with_context(|| format!("Failed to query legacy '{db_name}' database"))?;
        let Some(db) = db else {
            continue;
        };
        any_found = true;
        for item in db
            .iter(rtxn)
            .with_context(|| format!("Failed to iterate legacy '{db_name}' database"))?
        {
            let (key_bytes, name) =
                item.with_context(|| format!("Failed to read legacy '{db_name}' entry"))?;
            if let Some(hash) = read_u32_be(key_bytes) {
                // First writer wins — consistent with the upstream
                // builder's dedup-by-key behaviour after sorting.
                out.entry(hash).or_insert_with(|| name.to_string());
            }
        }
    }

    if !any_found {
        anyhow::bail!("no legacy databases present either");
    }
    Ok(out)
}

fn read_u32_be(bytes: &[u8]) -> Option<u32> {
    let arr: [u8; 4] = bytes.try_into().ok()?;
    Some(u32::from_be_bytes(arr))
}

fn read_u64_be(bytes: &[u8]) -> Option<u64> {
    let arr: [u8; 8] = bytes.try_into().ok()?;
    Some(u64::from_be_bytes(arr))
}

// ---------------------------------------------------------------------------
// Provider impl
// ---------------------------------------------------------------------------

impl HashProvider for LmdbHashProvider {
    fn resolve_type(&self, hash: TypeHash) -> Option<&str> {
        self.bin_names.get(&hash.0).map(|s| s.as_str())
    }

    fn resolve_field(&self, hash: FieldHash) -> Option<&str> {
        self.bin_names.get(&hash.0).map(|s| s.as_str())
    }

    fn resolve_entry(&self, hash: PathHash) -> Option<&str> {
        self.bin_names.get(&hash.0).map(|s| s.as_str())
    }

    fn resolve_game_path(&self, hash: GameHash) -> Option<&str> {
        self.game_paths.get(&hash.0).map(|s| s.as_str())
    }

    fn type_hash(&self, name: &str) -> Option<TypeHash> {
        self.bin_name_to_hash
            .get(&name.to_lowercase())
            .copied()
            .map(TypeHash)
    }

    fn field_hash(&self, name: &str) -> Option<FieldHash> {
        self.bin_name_to_hash
            .get(&name.to_lowercase())
            .copied()
            .map(FieldHash)
    }

    fn has_game_path(&self, path: &str) -> bool {
        use xxhash_rust::xxh64::xxh64;
        let normalized = path.to_lowercase().replace('\\', "/");
        let hash = xxh64(normalized.as_bytes(), 0);
        self.game_paths.contains_key(&hash)
    }

    fn is_loaded(&self) -> bool {
        !self.bin_names.is_empty() || !self.game_paths.is_empty()
    }
}
