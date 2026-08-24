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
use hematite_core::strings::fnv1a_hash;
use hematite_core::traits::HashProvider;
use hematite_types::hash::{FieldHash, GameHash, PathHash, TypeHash};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Hash provider backed by LMDB database.
///
/// Loads all hashes into memory at startup for O(1) lookups. The
/// in-memory state is intentionally schema-agnostic: a single `bin`
/// map covers types / fields / entries / generic, mirroring the
/// upstream merged-database design.
pub struct LmdbHashProvider {
    /// The open environment. Lookups run against it directly.
    env: Env,
    /// `u64` xxhash64 (BE) -> game asset path.
    wad_db: Option<Database<Bytes, Str>>,
    /// `u32` FNV1a (BE) -> type / field / entry / generic name.
    bin_db: Option<Database<Bytes, Str>>,
    /// Legacy four-database caches only. Those predate the merged `bin` DB and are small
    /// and rare, so they are still read eagerly rather than growing a second lazy path.
    legacy_bin: Option<HashMap<u32, String>>,
    /// Answers already fetched, misses included.
    ///
    /// A run touches a tiny fraction of the dictionary but touches the same hashes over
    /// and over, so this recovers nearly all of the constant-time lookup the preloaded
    /// maps gave without paying to build them. Misses are cached too: not finding a hash
    /// costs the same query as finding one.
    wad_cache: Mutex<HashMap<u64, Option<Arc<str>>>>,
    bin_cache: Mutex<HashMap<u32, Option<Arc<str>>>>,
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

    /// Hash of a BIN name, when the dictionary knows that name.
    ///
    /// The hash is computed rather than looked up. It used to come from a reverse map
    /// built at load time, which meant lowercasing and allocating half a million strings
    /// on every startup for a value that is a pure function of the name. The map is only
    /// consulted to preserve the original contract: an unknown name resolves to `None`
    /// rather than to a hash nothing will match.
    fn known_hash(&self, name: &str) -> Option<u32> {
        let hash = fnv1a_hash(name);
        self.lookup_bin(hash).map(|_| hash)
    }

    /// Open the dictionary for on-demand lookups.
    ///
    /// This used to read all 2.8 million entries into memory before any work started,
    /// costing over a second on every invocation and dominating the runtime of checking a
    /// small mod. Nothing needs the whole dictionary: a run resolves the few thousand
    /// hashes its own mod mentions.
    pub fn load_from_path(lmdb_dir: &Path) -> Result<Self> {
        tracing::debug!("Opening LMDB hashes at: {}", lmdb_dir.display());

        let env = open_env(lmdb_dir)?;
        let rtxn = env.read_txn().context("Failed to start read transaction")?;

        let wad_db: Option<Database<Bytes, Str>> = env
            .open_database(&rtxn, Some("wad"))
            .context("Failed to query 'wad' database")?;
        let bin_db: Option<Database<Bytes, Str>> = env
            .open_database(&rtxn, Some("bin"))
            .context("Failed to query 'bin' database")?;

        // Only pay for the legacy read when the merged DB is genuinely absent.
        let legacy_bin = if bin_db.is_none() {
            tracing::debug!("No 'bin' database (legacy schema); reading types/fields/entries");
            load_legacy_bin_dbs(&env, &rtxn).ok()
        } else {
            None
        };

        if wad_db.is_none() && bin_db.is_none() && legacy_bin.is_none() {
            anyhow::bail!(
                "LMDB has neither a 'wad'/'bin' database nor the legacy \
                 'types'/'fields'/'entries' schema. Delete the hashes.lmdb folder under \
                 %APPDATA%\\RitoShark\\Requirements\\Hashes and re-run to redownload."
            );
        }

        rtxn.commit().context("Failed to commit read transaction")?;

        Ok(Self {
            env,
            wad_db,
            bin_db,
            legacy_bin,
            wad_cache: Mutex::new(HashMap::new()),
            bin_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Look one game path up, going to the database only on a first miss.
    fn lookup_wad(&self, hash: u64) -> Option<Arc<str>> {
        if let Ok(cache) = self.wad_cache.lock() {
            if let Some(hit) = cache.get(&hash) {
                return hit.clone();
            }
        }
        let db = self.wad_db?;
        let found = self.env.read_txn().ok().and_then(|rtxn| {
            db.get(&rtxn, &hash.to_be_bytes())
                .ok()
                .flatten()
                .map(Arc::<str>::from)
        });
        if let Ok(mut cache) = self.wad_cache.lock() {
            cache.insert(hash, found.clone());
        }
        found
    }

    /// Look one BIN name up, from the merged database or a legacy cache.
    fn lookup_bin(&self, hash: u32) -> Option<Arc<str>> {
        if let Some(legacy) = &self.legacy_bin {
            return legacy.get(&hash).map(|s| Arc::from(s.as_str()));
        }
        if let Ok(cache) = self.bin_cache.lock() {
            if let Some(hit) = cache.get(&hash) {
                return hit.clone();
            }
        }
        let db = self.bin_db?;
        let found = self.env.read_txn().ok().and_then(|rtxn| {
            db.get(&rtxn, &hash.to_be_bytes())
                .ok()
                .flatten()
                .map(Arc::<str>::from)
        });
        if let Ok(mut cache) = self.bin_cache.lock() {
            cache.insert(hash, found.clone());
        }
        found
    }
}

// ---------------------------------------------------------------------------
// LMDB plumbing
// ---------------------------------------------------------------------------

/// Open the shared hash database.
///
/// The single definition of how this database is opened, and it has to stay that way.
/// LMDB refuses to open one path twice in a process with different options, so a second
/// definition anywhere (another crate, another app in the same process) does not merely
/// duplicate this one, it makes whichever opens second fail outright with "an environment
/// is already opened with different options". That is exactly what happened when Celestial
/// moved onto the shared path with its own `map_size` and `max_dbs`.
///
/// Public for that reason: anything in-process that needs this database calls this rather
/// than writing its own options. Opening twice through here is fine, heed hands back the
/// same environment.
pub fn open_env(lmdb_dir: &Path) -> Result<Env> {
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


// ---------------------------------------------------------------------------
// Provider impl
// ---------------------------------------------------------------------------

impl HashProvider for LmdbHashProvider {
    fn resolve_type(&self, hash: TypeHash) -> Option<String> {
        self.lookup_bin(hash.0).map(|s| s.to_string())
    }

    fn resolve_field(&self, hash: FieldHash) -> Option<String> {
        self.lookup_bin(hash.0).map(|s| s.to_string())
    }

    fn resolve_entry(&self, hash: PathHash) -> Option<String> {
        self.lookup_bin(hash.0).map(|s| s.to_string())
    }

    fn resolve_game_path(&self, hash: GameHash) -> Option<String> {
        self.lookup_wad(hash.0).map(|s| s.to_string())
    }

    fn type_hash(&self, name: &str) -> Option<TypeHash> {
        self.known_hash(name).map(TypeHash)
    }

    fn field_hash(&self, name: &str) -> Option<FieldHash> {
        self.known_hash(name).map(FieldHash)
    }

    fn has_game_path(&self, path: &str) -> bool {
        use xxhash_rust::xxh64::xxh64;
        let normalized = path.to_lowercase().replace('\\', "/");
        let hash = xxh64(normalized.as_bytes(), 0);
        self.lookup_wad(hash).is_some()
    }

    fn is_loaded(&self) -> bool {
        self.wad_db.is_some() || self.bin_db.is_some() || self.legacy_bin.is_some()
    }
}
