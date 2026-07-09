# Live Game Files + Pull Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a standalone `hematite-live` crate (League install auto-detection + TOC-only WAD reading + GameIndex), a `GameProvider` trait in hematite-core, five Celestial-ported fixes (gear pull, CAC pull, restore-anm, combo-bin relocation, dead-ref ladder), CLI wiring, RitoShark-Crates rev bump `fd2cb9d`→`daff556`, and ship v0.5.0.

**Architecture:** `hematite-live` is a 5th workspace crate with **zero** rs_*/hematite deps (TOC-only reader so huge game WADs are never loaded into RAM). hematite-core stays format-free: fixes consume live files only through a new `GameProvider` trait (methods take `&self`; impls use interior mutability). The trait impl (`LiveGameProvider`) lives in hematite-cli, wrapping `hematite_live::GameIndex` + `hematite_file` BIN parsing — the same crate-boundary pattern as `deep_repair`.

**Tech Stack:** Rust workspace, clap, serde, tracing; new deps for hematite-live: `sysinfo`, `flate2`, `zstd`, `xxhash-rust` (xxh64), `walkdir`, `serde_json`, `thiserror`.

## Global Constraints

- Repo: `E:\RitoShark\Skin-Fixer\hematite-v2`, branch `feat/ritoshark-migration`. All commands run from repo root.
- Lint gate (run after every task): `cargo clippy --lib --bins -- -D warnings -A clippy::needless_return`
- Test gate: `cargo test --workspace`
- Conventional commits, **never** add `Co-Authored-By`. Scopes: `types`, `core`, `file`, `cli`, `live`, `ci`.
- hematite-core must not import any file-format crate or hematite-live. hematite-live must not import rs_* or any hematite crate.
- Fail open: absence of a detected League install is never an error; live-dependent fixes log and skip.
- All new config enum variants are additive; do not set `deny_unknown_fields` anywhere.
- Reference sources (read-only, NEVER modify): Celestial `E:\RitoShark\Celestial`, Flint `E:\RitoShark\Flint\Flint - Main`, ltk_mod_core vendored under `C:\Users\emirf\.cargo\registry\src\index.crates.io-*\ltk_mod_core-0.1.0\src\league_path.rs`.

---

### Task 1: `hematite-live` crate scaffold + TOC reader + chunk reader

**Files:**
- Create: `crates/hematite-live/Cargo.toml`, `crates/hematite-live/src/lib.rs`, `crates/hematite-live/src/error.rs`, `crates/hematite-live/src/toc.rs`, `crates/hematite-live/src/chunk.rs`
- Modify: `Cargo.toml` (workspace members)
- Test: inline `#[cfg(test)]` in `toc.rs` and `chunk.rs`

**Interfaces:**
- Produces: `LiveError` (thiserror enum), `Compression { None, GZip, Satellite, Zstd, ZstdMulti }`, `TocChunk { path_hash: u64, offset: u64, compressed_size: u64, uncompressed_size: u64, compression: Compression }`, `WadToc { path: PathBuf, version: (u8, u8), chunks: Vec<TocChunk> }`, `pub fn read_toc(path: &Path) -> Result<WadToc, LiveError>`, `pub fn read_toc_from(reader: &mut (impl Read + Seek), path: PathBuf) -> Result<WadToc, LiveError>`, `pub fn read_chunk(file: &mut File, chunk: &TocChunk) -> Result<Vec<u8>, LiveError>`.

- [ ] **Step 1: Scaffold crate**

`crates/hematite-live/Cargo.toml`:
```toml
[package]
name = "hematite-live"
description = "Live League of Legends game file access: install detection, TOC-only WAD reading, game index"
version.workspace = true
edition.workspace = true

[dependencies]
thiserror = "1"
tracing = "0.1"
walkdir = "2"
serde_json = "1"
sysinfo = "0.30"
flate2 = "1"
zstd = "0.13"
xxhash-rust = { version = "0.8", features = ["xxh64"] }

[dev-dependencies]
tempfile = "3"
```

`crates/hematite-live/src/lib.rs`:
```rust
//! Live game file access for League of Legends.
//!
//! Standalone by design: no rs_* or hematite-* dependencies. Reads game WADs
//! TOC-only (never loads whole archives into memory) so it is safe to index
//! very large base-game WADs.

pub mod chunk;
pub mod detect;
pub mod error;
pub mod index;
pub mod toc;
pub mod wads;

pub use detect::{detect_league, LeagueInstall};
pub use error::LiveError;
pub use index::{wad_path_hash, GameIndex};
pub use toc::{read_toc, Compression, TocChunk, WadToc};
pub use wads::{champion_wad, enumerate_wads, GameWadInfo};
```
(Modules `detect`, `index`, `wads` are created in Tasks 2–3; create empty `pub mod` files now with `//! stub` so the crate compiles, or add the `pub mod` lines in the later tasks. Prefer: declare only `chunk`, `error`, `toc` now and extend `lib.rs` in Tasks 2–3.)

Add to workspace `Cargo.toml` members list: `"crates/hematite-live"`.

- [ ] **Step 2: Write failing TOC tests** (bottom of `toc.rs`, they drive the format)

Test helper builds a synthetic v3.4 WAD in memory. WAD layout (from Flint `wad_jade/reader.rs`):
header = magic `RW` (2 bytes) + major u8 + minor u8 + 256-byte ECDSA sig + 8-byte checksum + u32 chunk_count. v3.4+ chunk record (32 bytes): path_hash u64, data_offset u32, compressed_size u32, uncompressed_size u32, type_byte u8 (low nibble = compression, high nibble = subchunk count), duplicate u8, subchunk_start u16, checksum u64. v3.1 chunk record (32 bytes): path_hash u64, data_offset u32, compressed_size u32, uncompressed_size u32, type u8, duplicate u8, pad u16, checksum u64.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn synth_wad(major: u8, minor: u8, chunks: &[(u64, u32, u32, u32, u8)]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"RW");
        v.push(major);
        v.push(minor);
        v.extend_from_slice(&[0u8; 256]); // signature
        v.extend_from_slice(&[0u8; 8]);   // checksum
        v.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
        for &(hash, off, csz, usz, ty) in chunks {
            v.extend_from_slice(&hash.to_le_bytes());
            v.extend_from_slice(&off.to_le_bytes());
            v.extend_from_slice(&csz.to_le_bytes());
            v.extend_from_slice(&usz.to_le_bytes());
            v.push(ty);
            v.push(0);                       // duplicate
            v.extend_from_slice(&0u16.to_le_bytes()); // pad / subchunk_start
            v.extend_from_slice(&0u64.to_le_bytes()); // per-chunk checksum
        }
        v
    }

    #[test]
    fn parses_v3_4_toc() {
        let bytes = synth_wad(3, 4, &[(0xDEAD_BEEF, 400, 10, 20, 3)]);
        let toc = read_toc_from(&mut Cursor::new(bytes), "x.wad.client".into()).unwrap();
        assert_eq!(toc.version, (3, 4));
        assert_eq!(toc.chunks.len(), 1);
        let c = &toc.chunks[0];
        assert_eq!(c.path_hash, 0xDEAD_BEEF);
        assert_eq!(c.offset, 400);
        assert_eq!(c.compressed_size, 10);
        assert_eq!(c.uncompressed_size, 20);
        assert_eq!(c.compression, Compression::Zstd);
    }

    #[test]
    fn parses_v3_1_toc() {
        let bytes = synth_wad(3, 1, &[(1, 300, 5, 5, 0), (2, 305, 7, 9, 1)]);
        let toc = read_toc_from(&mut Cursor::new(bytes), "x.wad".into()).unwrap();
        assert_eq!(toc.chunks.len(), 2);
        assert_eq!(toc.chunks[0].compression, Compression::None);
        assert_eq!(toc.chunks[1].compression, Compression::GZip);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = synth_wad(3, 4, &[]);
        bytes[0] = b'X';
        assert!(matches!(
            read_toc_from(&mut Cursor::new(bytes), "x".into()),
            Err(LiveError::BadMagic)
        ));
    }

    #[test]
    fn rejects_unsupported_major() {
        let bytes = synth_wad(2, 0, &[]);
        assert!(matches!(
            read_toc_from(&mut Cursor::new(bytes), "x".into()),
            Err(LiveError::UnsupportedVersion { major: 2, minor: 0 })
        ));
    }

    #[test]
    fn zstd_multi_compression_kind() {
        // type byte low nibble 4 = ZstdMulti (subchunked)
        let bytes = synth_wad(3, 4, &[(9, 0, 1, 1, 4)]);
        let toc = read_toc_from(&mut Cursor::new(bytes), "x".into()).unwrap();
        assert_eq!(toc.chunks[0].compression, Compression::ZstdMulti);
    }
}
```

- [ ] **Step 3: Run tests to verify failure** — `cargo test -p hematite-live` → compile error (types not defined). Expected.

- [ ] **Step 4: Implement `error.rs` and `toc.rs`**

`error.rs`:
```rust
//! Error type for hematite-live.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a WAD file (bad magic)")]
    BadMagic,
    #[error("unsupported WAD version {major}.{minor}")]
    UnsupportedVersion { major: u8, minor: u8 },
    #[error("unsupported compression kind {0}")]
    UnsupportedCompression(u8),
    #[error("chunk not found: {0:016x}")]
    NotFound(u64),
    #[error("League installation not found or invalid: {0}")]
    InvalidInstall(PathBuf),
    #[error("decompression failed: {0}")]
    Decompress(String),
}
```

`toc.rs` — TOC-only reader:
```rust
//! TOC-only WAD reader. Reads header + chunk table, never chunk payloads.
//! Layout ported from Flint's `wad_jade/{format,reader}.rs` (RW v3.1 / v3.4).

use crate::error::LiveError;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};

pub const WAD_MAGIC: [u8; 2] = *b"RW";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    GZip,
    Satellite,
    Zstd,
    ZstdMulti,
}

impl Compression {
    fn from_type_byte(b: u8) -> Result<Self, LiveError> {
        match b & 0x0F {
            0 => Ok(Self::None),
            1 => Ok(Self::GZip),
            2 => Ok(Self::Satellite),
            3 => Ok(Self::Zstd),
            4 => Ok(Self::ZstdMulti),
            other => Err(LiveError::UnsupportedCompression(other)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TocChunk {
    pub path_hash: u64,
    pub offset: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub compression: Compression,
}

#[derive(Debug)]
pub struct WadToc {
    pub path: PathBuf,
    pub version: (u8, u8),
    pub chunks: Vec<TocChunk>,
}

fn read_u16(r: &mut impl Read) -> Result<u16, LiveError> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}
fn read_u32(r: &mut impl Read) -> Result<u32, LiveError> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn read_u64(r: &mut impl Read) -> Result<u64, LiveError> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

/// Read the TOC from an open reader. `path` is stored for diagnostics only.
pub fn read_toc_from(r: &mut (impl Read + Seek), path: PathBuf) -> Result<WadToc, LiveError> {
    let mut magic = [0u8; 2];
    r.read_exact(&mut magic)?;
    if magic != WAD_MAGIC {
        return Err(LiveError::BadMagic);
    }
    let mut ver = [0u8; 2];
    r.read_exact(&mut ver)?;
    let (major, minor) = (ver[0], ver[1]);
    if major != 3 {
        return Err(LiveError::UnsupportedVersion { major, minor });
    }
    // 256-byte ECDSA signature + 8-byte XXH64 checksum
    let mut skip = [0u8; 264];
    r.read_exact(&mut skip)?;
    let count = read_u32(r)? as usize;
    // Sanity cap: a chunk record is 32 bytes; refuse absurd counts.
    if count > 4_000_000 {
        return Err(LiveError::Decompress(format!("chunk count {count} too large")));
    }
    let mut chunks = Vec::with_capacity(count);
    for _ in 0..count {
        let path_hash = read_u64(r)?;
        let offset = read_u32(r)? as u64;
        let compressed_size = read_u32(r)? as u64;
        let uncompressed_size = read_u32(r)? as u64;
        let mut tail = [0u8; 2]; // type byte + duplicate flag
        r.read_exact(&mut tail)?;
        let compression = Compression::from_type_byte(tail[0])?;
        let _pad_or_subchunk_start = read_u16(r)?;
        let _chunk_checksum = read_u64(r)?;
        chunks.push(TocChunk {
            path_hash,
            offset,
            compressed_size,
            uncompressed_size,
            compression,
        });
    }
    Ok(WadToc {
        path,
        version: (major, minor),
        chunks,
    })
}

/// Read the TOC of a WAD file on disk.
pub fn read_toc(path: &Path) -> Result<WadToc, LiveError> {
    let file = File::open(path)?;
    let mut r = BufReader::new(file);
    read_toc_from(&mut r, path.to_path_buf())
}
```
Note: v3.1 and v3.4 records are both 32 bytes with the same field order for the fields we keep, so a single loop handles both (the 2-byte field after type/duplicate is padding on 3.1 and subchunk_start on 3.4 — ignored either way; ZstdMulti subchunk data decompresses fine with plain zstd stream decode-all in League WADs written after subchunking, matching Flint which routes both 3 and 4 to `zstd::stream::decode_all`).

- [ ] **Step 5: Implement `chunk.rs` with its tests**

```rust
//! On-demand chunk payload read + decompress.

use crate::error::LiveError;
use crate::toc::{Compression, TocChunk};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// Per-chunk sanity limit (matches hematite-file's WadFile guard).
pub const MAX_CHUNK_SIZE: u64 = 1024 * 1024 * 1024;

/// Read and decompress a single chunk from an open WAD file handle.
pub fn read_chunk(file: &mut File, chunk: &TocChunk) -> Result<Vec<u8>, LiveError> {
    if chunk.uncompressed_size > MAX_CHUNK_SIZE || chunk.compressed_size > MAX_CHUNK_SIZE {
        return Err(LiveError::Decompress(format!(
            "chunk {:016x} exceeds size limit",
            chunk.path_hash
        )));
    }
    file.seek(SeekFrom::Start(chunk.offset))?;
    let mut raw = vec![0u8; chunk.compressed_size as usize];
    file.read_exact(&mut raw)?;
    decompress(&raw, chunk)
}

pub(crate) fn decompress(raw: &[u8], chunk: &TocChunk) -> Result<Vec<u8>, LiveError> {
    match chunk.compression {
        Compression::None => Ok(raw.to_vec()),
        Compression::GZip => {
            let mut out = Vec::with_capacity(chunk.uncompressed_size as usize);
            flate2::read::GzDecoder::new(raw)
                .read_to_end(&mut out)
                .map_err(|e| LiveError::Decompress(e.to_string()))?;
            Ok(out)
        }
        Compression::Zstd | Compression::ZstdMulti => zstd::stream::decode_all(raw)
            .map_err(|e| LiveError::Decompress(e.to_string())),
        Compression::Satellite => Err(LiveError::UnsupportedCompression(2)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn chunk(comp: Compression, csz: usize, usz: usize) -> TocChunk {
        TocChunk {
            path_hash: 1,
            offset: 0,
            compressed_size: csz as u64,
            uncompressed_size: usz as u64,
            compression: comp,
        }
    }

    #[test]
    fn decompress_none_passthrough() {
        let data = b"hello".to_vec();
        let out = decompress(&data, &chunk(Compression::None, 5, 5)).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn decompress_gzip_roundtrip() {
        let plain = b"league of legends".repeat(10);
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&plain).unwrap();
        let comp = enc.finish().unwrap();
        let out = decompress(&comp, &chunk(Compression::GZip, comp.len(), plain.len())).unwrap();
        assert_eq!(out, plain);
    }

    #[test]
    fn decompress_zstd_roundtrip() {
        let plain = b"wad chunk payload".repeat(100);
        let comp = zstd::stream::encode_all(&plain[..], 3).unwrap();
        let out = decompress(&comp, &chunk(Compression::Zstd, comp.len(), plain.len())).unwrap();
        assert_eq!(out, plain);
    }

    #[test]
    fn satellite_is_unsupported() {
        assert!(decompress(b"x", &chunk(Compression::Satellite, 1, 1)).is_err());
    }
}
```

- [ ] **Step 6: Run** `cargo test -p hematite-live` → all TOC + chunk tests PASS. Run clippy gate.

- [ ] **Step 7: Commit** — `git add crates/hematite-live Cargo.toml Cargo.lock && git commit -m "feat(live): add hematite-live crate with TOC-only WAD reader"`

---

### Task 2: `hematite-live` League install detection

**Files:**
- Create: `crates/hematite-live/src/detect.rs`
- Modify: `crates/hematite-live/src/lib.rs` (add `pub mod detect;` + re-exports)
- Test: inline in `detect.rs` (only pure/validation parts — process/registry/json probing is environment-dependent and stays untested)

**Interfaces:**
- Produces: `LeagueInstall { root: PathBuf, game_dir: PathBuf, auto_detected: bool }`, `pub fn detect_league() -> Option<LeagueInstall>`, `LeagueInstall::from_path(p: &Path) -> Result<LeagueInstall, LiveError>`.

- [ ] **Step 1: Write failing tests** (validation logic; use `tempfile` to fabricate an install tree)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fake_install(root: &std::path::Path) {
        let game = root.join("Game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("League of Legends.exe"), b"").unwrap();
    }

    #[test]
    fn from_path_accepts_install_root() {
        let dir = tempfile::tempdir().unwrap();
        fake_install(dir.path());
        let li = LeagueInstall::from_path(dir.path()).unwrap();
        assert_eq!(li.game_dir, dir.path().join("Game"));
        assert!(!li.auto_detected);
    }

    #[test]
    fn from_path_accepts_game_dir_directly() {
        let dir = tempfile::tempdir().unwrap();
        fake_install(dir.path());
        let li = LeagueInstall::from_path(&dir.path().join("Game")).unwrap();
        assert_eq!(li.game_dir, dir.path().join("Game"));
    }

    #[test]
    fn from_path_rejects_random_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(LeagueInstall::from_path(dir.path()).is_err());
    }

    #[test]
    fn riot_installs_json_parses_associated_client() {
        let json = r#"{
            "associated_client": {
                "C:/Riot Games/League of Legends/": "C:/Riot Games/Riot Client/x.exe",
                "C:/Riot Games/League of Legends (PBE)/": "C:/Riot Games/Riot Client/x.exe"
            }
        }"#;
        let paths = league_paths_from_riot_installs_json(json);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with("League of Legends") || paths[0].ends_with("League of Legends/"));
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p hematite-live detect` → compile error. Expected.

- [ ] **Step 3: Implement `detect.rs`**

```rust
//! League of Legends install auto-detection.
//!
//! Ported from Flint's chain (`ltk_mod_core::league_path` + flint-ltk's
//! `league/detector.rs`), in priority order:
//!  1. RiotClientInstalls.json (ProgramData)
//!  2. Running League processes (sysinfo)
//!  3. Common install paths across drives
//!  4. Windows registry (reg query shell-out)
//!
//! All the Riot names/paths below are inherently hardcoded — they are Riot's
//! install layout, and there is nothing we can do about that.

use crate::error::LiveError;
use std::path::{Path, PathBuf};

const GAME_EXE: &str = "League of Legends.exe";
const GAME_DIR: &str = "Game";
const PROCESS_NAMES: &[&str] = &["LeagueClientUx.exe", "LeagueClient.exe", "League of Legends.exe"];
const FALLBACK_DRIVES: &[&str] = &["C:", "D:", "E:", "F:", "G:", "H:"];
const COMMON_SUBPATHS: &[&str] = &[
    "Riot Games\\League of Legends",
    "Program Files\\Riot Games\\League of Legends",
    "Program Files (x86)\\Riot Games\\League of Legends",
];
const REGISTRY_KEY: &str = r"HKLM\SOFTWARE\WOW6432Node\Riot Games, Inc\League of Legends";

#[derive(Debug, Clone)]
pub struct LeagueInstall {
    /// Install root (contains LeagueClient.exe and Game/).
    pub root: PathBuf,
    /// `<root>/Game` — where DATA/FINAL lives.
    pub game_dir: PathBuf,
    pub auto_detected: bool,
}

impl LeagueInstall {
    /// Accept either the install root or the Game/ dir itself; validate that
    /// `Game/League of Legends.exe` exists.
    pub fn from_path(p: &Path) -> Result<Self, LiveError> {
        // Case 1: p is the Game dir (contains the game exe directly).
        if p.join(GAME_EXE).is_file() {
            let root = p.parent().unwrap_or(p).to_path_buf();
            return Ok(Self { root, game_dir: p.to_path_buf(), auto_detected: false });
        }
        // Case 2: p is the install root.
        let game = p.join(GAME_DIR);
        if game.join(GAME_EXE).is_file() {
            return Ok(Self { root: p.to_path_buf(), game_dir: game, auto_detected: false });
        }
        Err(LiveError::InvalidInstall(p.to_path_buf()))
    }

    fn from_root_detected(root: PathBuf) -> Option<Self> {
        let game = root.join(GAME_DIR);
        if game.join(GAME_EXE).is_file() {
            Some(Self { root, game_dir: game, auto_detected: true })
        } else {
            None
        }
    }
}

/// Parse League install roots out of RiotClientInstalls.json content.
/// PBE installs are excluded (folder must be exactly "League of Legends").
pub(crate) fn league_paths_from_riot_installs_json(content: &str) -> Vec<PathBuf> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(content) else {
        return Vec::new();
    };
    let Some(map) = v.get("associated_client").and_then(|c| c.as_object()) else {
        return Vec::new();
    };
    map.keys()
        .filter(|k| {
            let trimmed = k.trim_end_matches(['/', '\\']);
            trimmed.ends_with("League of Legends")
        })
        .map(PathBuf::from)
        .collect()
}

fn detect_from_riot_client_installs() -> Option<LeagueInstall> {
    let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let json_path = PathBuf::from(format!(
        "{}\\ProgramData\\Riot Games\\RiotClientInstalls.json",
        system_drive
    ));
    let content = std::fs::read_to_string(json_path).ok()?;
    league_paths_from_riot_installs_json(&content)
        .into_iter()
        .find_map(LeagueInstall::from_root_detected)
}

fn detect_from_running_process() -> Option<LeagueInstall> {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_processes();
    for wanted in PROCESS_NAMES {
        for proc_ in sys.processes().values() {
            if !proc_.name().eq_ignore_ascii_case(wanted) {
                continue;
            }
            let Some(exe) = proc_.exe() else { continue };
            let Some(dir) = exe.parent() else { continue };
            // Game exe: parent is Game/, root is one above.
            if wanted.eq_ignore_ascii_case(GAME_EXE) {
                if let Some(root) = dir.parent() {
                    if let Some(li) = LeagueInstall::from_root_detected(root.to_path_buf()) {
                        return Some(li);
                    }
                }
            } else if let Some(li) = LeagueInstall::from_root_detected(dir.to_path_buf()) {
                // Client exes live in the install root.
                return Some(li);
            }
        }
    }
    None
}

fn detect_from_common_paths() -> Option<LeagueInstall> {
    for drive in FALLBACK_DRIVES {
        for sub in COMMON_SUBPATHS {
            let root = PathBuf::from(format!("{}\\{}", drive, sub));
            if let Some(li) = LeagueInstall::from_root_detected(root) {
                return Some(li);
            }
        }
    }
    None
}

fn detect_from_registry() -> Option<LeagueInstall> {
    let out = std::process::Command::new("reg")
        .args(["query", REGISTRY_KEY, "/v", "Location"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Line shape: "    Location    REG_SZ    C:\Riot Games\League of Legends"
    let loc = text.lines().find_map(|l| {
        let l = l.trim();
        l.contains("REG_SZ").then(|| l.split("REG_SZ").nth(1).map(str::trim))?
    })?;
    LeagueInstall::from_root_detected(PathBuf::from(loc))
}

/// Try all detection mechanisms in priority order.
pub fn detect_league() -> Option<LeagueInstall> {
    detect_from_riot_client_installs()
        .or_else(detect_from_running_process)
        .or_else(detect_from_common_paths)
        .or_else(detect_from_registry)
        .inspect(|li| tracing::info!("Detected League install at {}", li.root.display()))
}
```
(If `sysinfo` 0.30's API differs — `refresh_processes` signature or `proc.name()` returning `&OsStr` in newer versions — adapt to the version that resolves; pin what compiles. If Option combinators around `?` in `detect_from_registry` fight the borrow checker, rewrite as a plain `for` loop; the behavior contract is only "extract the REG_SZ value".)

- [ ] **Step 4: Run tests** — `cargo test -p hematite-live` → PASS. Clippy gate.

- [ ] **Step 5: Commit** — `git commit -am "feat(live): League install auto-detection (json/process/common-paths/registry)"`

---

### Task 3: `hematite-live` WAD enumeration + GameIndex

**Files:**
- Create: `crates/hematite-live/src/wads.rs`, `crates/hematite-live/src/index.rs`
- Modify: `crates/hematite-live/src/lib.rs`
- Test: inline in both files (tempdir fixture WADs built with the Task 1 synth helper + real zstd payloads)

**Interfaces:**
- Produces:
  - `GameWadInfo { path: PathBuf, name: String, category: String }`
  - `pub fn enumerate_wads(game_dir: &Path) -> Vec<GameWadInfo>`
  - `pub fn champion_wad(game_dir: &Path, champion: &str) -> Option<PathBuf>` (case-insensitive against real dir listing)
  - `pub fn wad_path_hash(path: &str) -> u64` (xxh64 of lowercased, `\`→`/` normalized)
  - `GameIndex::new(install: &LeagueInstall) -> GameIndex`
  - `GameIndex::add_wad(&mut self, path: &Path) -> Result<(), LiveError>` (idempotent)
  - `GameIndex::add_champion(&mut self, champion: &str) -> bool`
  - `GameIndex::has_hash(&self, h: u64) -> bool`, `has_path(&self, p: &str) -> bool`
  - `GameIndex::pull_hash(&mut self, h: u64) -> Option<Vec<u8>>`, `pull_path(&mut self, p: &str) -> Option<Vec<u8>>`
  - `GameIndex::hash_set(&self) -> std::collections::HashSet<u64>`

- [ ] **Step 1: Write failing tests**

`wads.rs` tests: create `tempdir/DATA/FINAL/Champions/Aatrox.wad.client` + `tempdir/DATA/FINAL/Maps/Shipping/Map11.wad.client` as empty files → `enumerate_wads` returns 2 entries with categories `Champions`/`Shipping`; `champion_wad(dir, "aatrox")` finds `Aatrox.wad.client` case-insensitively; returns `None` for missing champion.

`index.rs` tests: write a real fixture WAD via a `fn write_fixture_wad(path, entries: &[(&str, &[u8])])` helper that builds a valid v3.4 WAD (header from Task 1 synth layout, payloads zstd-compressed, offsets computed after the 268+32*n byte header/TOC):
```rust
#[cfg(test)]
pub(crate) fn write_fixture_wad(path: &std::path::Path, entries: &[(&str, &[u8])]) {
    let mut payloads: Vec<Vec<u8>> = Vec::new();
    for (_, data) in entries {
        payloads.push(zstd::stream::encode_all(&data[..], 3).unwrap());
    }
    let header_len = 2 + 2 + 256 + 8 + 4 + 32 * entries.len();
    let mut out = Vec::new();
    out.extend_from_slice(b"RW");
    out.push(3);
    out.push(4);
    out.extend_from_slice(&[0u8; 264]);
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    let mut offset = header_len as u32;
    for ((p, data), comp) in entries.iter().zip(&payloads) {
        out.extend_from_slice(&wad_path_hash(p).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&(comp.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.push(3); // zstd
        out.push(0);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
        offset += comp.len() as u32;
    }
    for comp in &payloads {
        out.extend_from_slice(comp);
    }
    std::fs::write(path, out).unwrap();
}

#[test]
fn index_pulls_by_path_and_hash() {
    let dir = tempfile::tempdir().unwrap();
    let champs = dir.path().join("Game/DATA/FINAL/Champions");
    std::fs::create_dir_all(&champs).unwrap();
    let wad = champs.join("Yone.wad.client");
    write_fixture_wad(&wad, &[("data/characters/yone/skins/skin0.bin", b"PROPdata")]);

    std::fs::write(dir.path().join("Game").join("League of Legends.exe"), b"").unwrap();
    let install = crate::detect::LeagueInstall::from_path(dir.path()).unwrap();
    let mut idx = GameIndex::new(&install);
    assert!(idx.add_champion("yone"));
    assert!(idx.has_path("data/characters/yone/skins/skin0.bin"));
    assert!(!idx.has_path("data/characters/yone/skins/skin1.bin"));
    assert_eq!(
        idx.pull_path("data/characters/yone/skins/skin0.bin").unwrap(),
        b"PROPdata"
    );
    // add_champion for a champion with no WAD is a no-op returning false
    assert!(!idx.add_champion("nonexistent_champ"));
}
```

- [ ] **Step 2: Run to verify failure** — compile error. Expected.

- [ ] **Step 3: Implement `wads.rs`**

```rust
//! Base-game WAD enumeration under <Game>/DATA/FINAL.

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const DATA_FINAL: &str = "DATA/FINAL";
const CHAMPIONS_DIR: &str = "Champions";
const MAX_DEPTH: usize = 5;

#[derive(Debug, Clone)]
pub struct GameWadInfo {
    pub path: PathBuf,
    pub name: String,
    pub category: String,
}

fn is_wad_name(name: &str) -> bool {
    let n = name.to_lowercase();
    n.ends_with(".wad.client") || n.ends_with(".wad")
}

pub fn enumerate_wads(game_dir: &Path) -> Vec<GameWadInfo> {
    let root = game_dir.join(DATA_FINAL);
    let mut out = Vec::new();
    for entry in WalkDir::new(&root).max_depth(MAX_DEPTH).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_wad_name(&name) {
            continue;
        }
        let category = entry
            .path()
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        out.push(GameWadInfo { path: entry.path().to_path_buf(), name, category });
    }
    out
}

/// Find `<game_dir>/DATA/FINAL/Champions/<Champion>.wad.client`, matching the
/// champion name case-insensitively against the real directory listing.
pub fn champion_wad(game_dir: &Path, champion: &str) -> Option<PathBuf> {
    let dir = game_dir.join(DATA_FINAL).join(CHAMPIONS_DIR);
    let wanted = format!("{}.wad.client", champion.to_lowercase());
    let entries = std::fs::read_dir(&dir).ok()?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.to_lowercase() == wanted {
            return Some(e.path());
        }
    }
    None
}
```

- [ ] **Step 4: Implement `index.rs`**

```rust
//! GameIndex — lazy, multi-WAD hash index with on-demand chunk pulls.

use crate::chunk::read_chunk;
use crate::detect::LeagueInstall;
use crate::error::LiveError;
use crate::toc::{read_toc, TocChunk};
use crate::wads::champion_wad;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use xxhash_rust::xxh64::xxh64;

/// League indexes WAD chunks by xxh64 of the lowercased, forward-slashed path.
pub fn wad_path_hash(path: &str) -> u64 {
    xxh64(path.to_lowercase().replace('\\', "/").as_bytes(), 0)
}

struct LoadedWad {
    path: PathBuf,
    chunks: Vec<TocChunk>,
    file: Option<File>, // opened lazily on first pull
}

pub struct GameIndex {
    game_dir: PathBuf,
    wads: Vec<LoadedWad>,
    /// hash → (wad idx, chunk idx). First WAD to define a hash wins.
    by_hash: HashMap<u64, (usize, usize)>,
    loaded_paths: HashSet<PathBuf>,
}

impl GameIndex {
    pub fn new(install: &LeagueInstall) -> Self {
        Self {
            game_dir: install.game_dir.clone(),
            wads: Vec::new(),
            by_hash: HashMap::new(),
            loaded_paths: HashSet::new(),
        }
    }

    pub fn game_dir(&self) -> &Path {
        &self.game_dir
    }

    /// Load a WAD's TOC into the index. Idempotent per path.
    pub fn add_wad(&mut self, path: &Path) -> Result<(), LiveError> {
        let canonical = path.to_path_buf();
        if !self.loaded_paths.insert(canonical.clone()) {
            return Ok(());
        }
        let toc = read_toc(path)?;
        let wad_idx = self.wads.len();
        for (i, c) in toc.chunks.iter().enumerate() {
            self.by_hash.entry(c.path_hash).or_insert((wad_idx, i));
        }
        tracing::debug!("GameIndex: loaded {} chunks from {}", toc.chunks.len(), path.display());
        self.wads.push(LoadedWad { path: canonical, chunks: toc.chunks, file: None });
        Ok(())
    }

    /// Add the champion's base WAD, if it exists. Returns whether it was found.
    pub fn add_champion(&mut self, champion: &str) -> bool {
        match champion_wad(&self.game_dir, champion) {
            Some(p) => match self.add_wad(&p) {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!("GameIndex: failed to load {}: {}", p.display(), e);
                    false
                }
            },
            None => {
                tracing::debug!("GameIndex: no champion WAD for '{}'", champion);
                false
            }
        }
    }

    pub fn has_hash(&self, h: u64) -> bool {
        self.by_hash.contains_key(&h)
    }

    pub fn has_path(&self, p: &str) -> bool {
        self.has_hash(wad_path_hash(p))
    }

    /// Snapshot of every indexed hash (for suffix-strip resolution helpers).
    pub fn hash_set(&self) -> HashSet<u64> {
        self.by_hash.keys().copied().collect()
    }

    pub fn pull_hash(&mut self, h: u64) -> Option<Vec<u8>> {
        let &(wi, ci) = self.by_hash.get(&h)?;
        let wad = &mut self.wads[wi];
        if wad.file.is_none() {
            match File::open(&wad.path) {
                Ok(f) => wad.file = Some(f),
                Err(e) => {
                    tracing::warn!("GameIndex: open {} failed: {}", wad.path.display(), e);
                    return None;
                }
            }
        }
        let chunk = wad.chunks[ci];
        match read_chunk(wad.file.as_mut().unwrap(), &chunk) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                tracing::warn!("GameIndex: chunk {:016x} read failed: {}", h, e);
                None
            }
        }
    }

    pub fn pull_path(&mut self, p: &str) -> Option<Vec<u8>> {
        self.pull_hash(wad_path_hash(p))
    }
}
```

- [ ] **Step 5: Update `lib.rs` re-exports** (as in Task 1 listing), run `cargo test -p hematite-live` → PASS, clippy gate.

- [ ] **Step 6: Commit** — `git commit -am "feat(live): WAD enumeration and lazy multi-WAD GameIndex"`

---

### Task 4: `GameProvider` trait in hematite-core + FixContext plumbing

**Files:**
- Modify: `crates/hematite-core/src/traits.rs` (append trait), `crates/hematite-core/src/context.rs` (new field), every `FixContext { ... }` literal construction site (find with `grep -rn "FixContext {" crates/`) — add `game: None` (CLI sites get real values in Task 8).
- Test: `crates/hematite-core/src/traits.rs` inline (mock impl compiles + object safety)

**Interfaces:**
- Produces (in `hematite_core::traits`):
```rust
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
    fn game_bin(&self, path: &str) -> Option<BinTree>;
}
```
- `FixContext` gains `pub game: Option<&'a dyn GameProvider>` after `shader_validator`.

- [ ] **Step 1: Write failing test** (in `traits.rs`):
```rust
#[cfg(test)]
mod game_provider_tests {
    use super::*;

    struct NullGame;
    impl GameProvider for NullGame {
        fn has_path(&self, _p: &str) -> bool { false }
        fn pull_raw(&self, _p: &str) -> Option<Vec<u8>> { None }
        fn game_bin(&self, _p: &str) -> Option<BinTree> { None }
    }

    #[test]
    fn game_provider_is_object_safe() {
        let g: &dyn GameProvider = &NullGame;
        assert!(!g.has_path("data/x.bin"));
        assert!(g.pull_raw("x").is_none());
        assert!(g.game_bin("x").is_none());
    }
}
```
- [ ] **Step 2: Run** `cargo test -p hematite-core game_provider` → compile error. Expected.
- [ ] **Step 3: Implement** trait in `traits.rs`; add `pub game: Option<&'a dyn GameProvider>` to `FixContext` (`context.rs`), import `crate::traits::GameProvider`. Then `cargo build --workspace` and fix every construction-site error by adding `game: None,`. (Sites are in `hematite-cli/src/process.rs` and core/cli tests — mechanical.)
- [ ] **Step 4: Run** `cargo test --workspace` → PASS. Clippy gate.
- [ ] **Step 5: Commit** — `git commit -am "feat(core): GameProvider trait + FixContext.game slot"`

---

### Task 5: `DeadEntryLink` detection rule

**Files:**
- Modify: `crates/hematite-types/src/config.rs` (new DetectionRule variant), `crates/hematite-core/src/detect/rules.rs` (dispatch + impl)
- Test: inline in `rules.rs` (follow the existing test style there — build a `BinTree` with objects, a fake HashProvider; grep for existing `#[cfg(test)]` in `detect/` for the established fixtures and reuse them)

**Interfaces:**
- Config variant (in `DetectionRule`):
```rust
/// Link fields on the main entry reference target entries that are defined
/// nowhere: not in this tree, not in mod-shipped linked trees, and not in
/// any game-resolvable `linked:` BIN. The lethal inverse of
/// `UnreferencedEntryOfType` (e.g. dead GearSkinUpgrade links crash).
#[serde(rename = "dead_entry_link")]
DeadEntryLink {
    main_entry_type: String,
    targets: Vec<EntryValidationTarget>,
},
```
- Core produces `pub(crate) fn collect_dead_links(ctx: &FixContext, main_entry_type: &str, targets: &[EntryValidationTarget]) -> Vec<(usize, u32)>` in a new shared helper module `crates/hematite-core/src/detect/dead_links.rs` — returns `(target_index, dead_path_hash)` pairs. Task 6's transform reuses it.

**Algorithm (`dead_links.rs`):**
1. Resolve `main_entry_type` → type hash via `ctx.hashes.type_hash()`; collect main objects.
2. Per target: parse `target.link_field` hex (e.g. `"0xcb522723"`) → field hash. Recursively search each main object's property values (Struct/Embedded/Container/Optional/Map descent — mirror the recursion in `transform/remove_unreferenced.rs`) for properties whose `name_hash` equals the link field; collect all `PropertyValue::Link(u32)` values inside them (a direct Link or a Container of Links).
3. Build the defined set: keys of `ctx.tree.objects` ∪ keys of every `ctx.linked_trees` value ∪ (if `ctx.game` is Some) keys of `game.game_bin(linked_path)` for each `linked_path` in `ctx.tree.linked` (cache per path within the call; missing game bins contribute nothing).
4. Dead = referenced values ∉ defined set and ≠ 0. Return them.
5. `detect` returns `!collect_dead_links(...).is_empty()`. Bail early (return false) if `!ctx.hashes.is_loaded()`. **When `ctx.game` is `None`, return false** — without game knowledge we cannot distinguish "dead" from "resolved by the game", and Celestial's fail-open invariant applies.

- [ ] **Step 1: Write failing tests** — three cases: (a) main entry links hash `0x1234` of GearSkinUpgrade class, tree does not define it, mock game provider returns no bins → detected; (b) tree defines the entry → not detected; (c) `ctx.game = None` → not detected (fail open). Use the existing rules.rs test fixtures for building objects with `PropertyValue::Container(vec![PropertyValue::Link(0x1234)])` under the gear link field hash.
- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement** `dead_links.rs` + variant + dispatch arm in `detect_issue()` (`rules.rs`), `pub mod dead_links;` in `detect/mod.rs`.
- [ ] **Step 4: Run tests + clippy** → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(core): dead_entry_link detection (referenced-but-missing entries)"`

---

### Task 6: `PullEntriesFromGame` transform (gear + CAC pull)

**Files:**
- Create: `crates/hematite-core/src/transform/pull_entries.rs`
- Modify: `crates/hematite-types/src/config.rs` (variant), `crates/hematite-core/src/transform/mod.rs` (module + dispatch)
- Test: inline in `pull_entries.rs` with a mock `GameProvider`

**Interfaces:**
- Config variant:
```rust
/// Pull referenced-but-missing target entries out of the live game's BIN
/// closure and inject them into this tree. Unpullable links either nuke a
/// fallback field on the main entry (gear: "skinUpgradeData") or drop the
/// dead link value (CAC).
#[serde(rename = "pull_entries_from_game")]
PullEntriesFromGame {
    main_entry_type: String,
    targets: Vec<EntryValidationTarget>,
    /// Field (by name) on the main entry to REMOVE when a target link cannot
    /// be pulled. None = drop only the dead link value from its container.
    #[serde(default)]
    nuke_fallback_field: Option<String>,
},
```
- Dispatch arm in `apply_transform`:
```rust
TransformAction::PullEntriesFromGame { main_entry_type, targets, nuke_fallback_field } =>
    pull_entries::apply(ctx, main_entry_type, targets, nuke_fallback_field.as_deref()),
```
- `pub fn apply(ctx: &mut FixContext, main_entry_type: &str, targets: &[EntryValidationTarget], nuke_fallback_field: Option<&str>) -> u32`

**Algorithm (`pull_entries.rs`):**
1. `let Some(game) = ctx.game else { return 0 }` (log debug: skipped, no game files).
2. `let dead = detect::dead_links::collect_dead_links(ctx, main_entry_type, targets);` — empty → 0.
3. Build the game closure once: seed = `seeds::discover_seeds([ctx.file_path])`; start paths = seed canonical bin paths (reuse `SkinSeed::bin_path()`) **plus** every path in `ctx.tree.linked`. BFS with `seen: HashSet<String>` and a hard cap of 64 bins: `game.game_bin(path)` → collect `(path_hash → BinObject)` for all objects, enqueue that tree's `.linked`. Store `closure: HashMap<u32, BinObject>`.
4. For each dead `(target_idx, hash)`: if `closure` has the object AND (target's `type_hash` parses to the same class hash, or `entry_type` resolves to it, or neither is resolvable — accept) → `ctx.tree.objects.insert(hash, obj.clone())`, count += 1, log `pulled <entry_type> {hash:08x} from game closure`.
5. Remaining unpullable dead links: if `nuke_fallback_field` is Some → resolve field name → field hash via `ctx.hashes.field_hash()`; for each main-entry object remove that property (recursively: also inside embeds — search top-level first, then one level of Struct/Embedded) when present; count += removals; log. Else → walk the link containers found in step 2's recursion and `retain` out the dead Link values; count += drops.
6. Return count.

- [ ] **Step 1: Write failing tests** with a `MockGame` implementing `GameProvider` over a `HashMap<String, BinTree>`:
  - `pulls_missing_gear_entry_from_game_closure`: mod tree = SkinCharacterDataProperties with gear link `0x1234` (inside a `skinUpgradeData` embed) but no `0x1234` object; MockGame maps `data/characters/x/skins/skin0.bin` (ctx.file_path names the same path so seeds resolve) → a game tree defining object `0x1234` (class GearSkinUpgrade). After `apply`, `ctx.tree.objects` contains `0x1234`, return ≥ 1.
  - `nukes_fallback_field_when_unpullable`: same but MockGame has no defining tree and `nuke_fallback_field = Some("skinUpgradeData")` → the embed property is removed from the main object.
  - `drops_dead_link_when_no_nuke_field`: CAC-shaped: dead link in a container under `0xd8f64a0d`, no nuke field → container no longer contains the dead value.
  - `noop_without_game_provider`: `ctx.game = None` → returns 0, tree untouched.
  Use a mock HashProvider that resolves "SkinCharacterDataProperties"/"GearSkinUpgrade" type hashes and "skinUpgradeData" field hash (copy the mock provider pattern already used in core's existing transform tests — grep `struct MockHash` / `impl HashProvider` under `crates/hematite-core/src`).
- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement** (module + variant + dispatch).
- [ ] **Step 4: Run tests + clippy** → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(core): pull_entries_from_game transform (gear/CAC pull with nuke fallback)"`

---

### Task 7: `ResolveDeadRefs` transform (game-TOC dead-ref ladder)

**Files:**
- Create: `crates/hematite-core/src/transform/resolve_refs.rs`
- Modify: `crates/hematite-types/src/config.rs` (variant), `crates/hematite-core/src/transform/mod.rs` (module + dispatch)
- Test: inline in `resolve_refs.rs`

**Interfaces:**
- Config variant:
```rust
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
```
- `pub fn apply(ctx: &mut FixContext, extensions: &[String]) -> u32`
- Helper `pub(crate) fn ext_twin(path: &str) -> Option<String>` (dds↔tex, sco↔scb) and `pub(crate) fn strip_inner_suffix(path: &str) -> Option<String>` (same contract as `hematite_file::wad_adapter::strip_inner_suffix` — copy the 3-segment implementation and its doc; core cannot import hematite-file).

**Implementation:** use the `PropertyWalker`/visitor from `walk.rs` the way `replace_ext.rs` does (read that file first and mirror its mutation pattern). For each visited string ending in `.{ext}` for some configured ext:
```text
if ctx.wad.has_path(s)            -> skip
if game.has_path(s)               -> skip
t = ext_twin(s):        if ctx.wad.has_path(t) or game.has_path(t) -> rewrite to t
st = strip_inner_suffix(s): if game.has_path(st)                   -> rewrite to st
stt = st.and_then(ext_twin): if game.has_path(stt)                 -> rewrite to stt
else                              -> leave unchanged
```
Also apply the same ladder to `ctx.tree.linked` entries ending in `.bin`? **No** — linked bins are not asset strings; leave them (deep repair handles them).

- [ ] **Step 1: Write failing tests** — mock `WadProvider` (HashSet-backed, same as core tests use) + `MockGame` with a configurable `has: HashSet<String>`:
  - `skips_when_mod_ships_file`, `skips_when_game_ships_file`
  - `rewrites_to_tex_twin_in_game` (`foo.dds` dead, game has `foo.tex` → string becomes `foo.tex`)
  - `rewrites_suffix_stripped_anm` (`attack1.matcha.anm` dead, game has `attack1.anm`)
  - `noop_without_game` (returns 0)
  - `ignores_unlisted_extensions` (`.bnk` string untouched)
- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run tests + clippy** → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(core): resolve_dead_refs transform (game-aware ref ladder)"`

---

### Task 8: CLI — `LiveGameProvider` adapter + flags + game-source resolution

**Files:**
- Create: `crates/hematite-cli/src/live_provider.rs`
- Modify: `crates/hematite-cli/src/args.rs` (3 flags), `crates/hematite-cli/src/main.rs` (module decl, resolution, plumb into process), `crates/hematite-cli/src/process.rs` (`ProcessContext` gains the provider; `FixContext` sites set `game`), `crates/hematite-cli/Cargo.toml` (`hematite-live` path dep)
- Test: inline in `live_provider.rs` (fixture WAD from hematite-live's test helper — re-export `write_fixture_wad` under `#[doc(hidden)] pub mod test_util` in hematite-live behind `#[cfg(any(test, feature = "test-util"))]`; simplest: duplicate the tiny helper in the CLI test)

**Interfaces:**
- `args.rs` additions:
```rust
#[arg(long, value_name = "DIR",
      help = "Path to the League of Legends install (root or Game dir). \
              If omitted, hematite auto-detects the install. Live-game \
              features (deep repair, gear/CAC pull, ref ladder, --restore-anm) \
              use this.")]
pub game_path: Option<std::path::PathBuf>,

#[arg(long, help = "Disable all live-game features (no install detection, no game pulls)")]
pub no_live: bool,

#[arg(long, help = "Restore missing .anm animation files by pulling them from the game \
                    (disables anm_remover for this run)")]
pub restore_anm: bool,
```
- `live_provider.rs`:
```rust
//! GameProvider impl backed by hematite-live's GameIndex.
//! Interior mutability (Mutex) because core's GameProvider takes &self.

use hematite_core::traits::{BinProvider, GameProvider};
use hematite_live::GameIndex;
use hematite_types::bin::BinTree;
use std::sync::Mutex;

pub struct LiveGameProvider {
    index: Mutex<GameIndex>,
    bin: Box<dyn BinProvider>,
}

impl LiveGameProvider {
    pub fn new(index: GameIndex, bin: Box<dyn BinProvider>) -> Self {
        Self { index: Mutex::new(index), bin }
    }

    /// Direct access for CLI-side machinery (deep repair, restore-anm,
    /// relocation) that wants hashes/pulls without trait indirection.
    pub fn with_index<R>(&self, f: impl FnOnce(&mut GameIndex) -> R) -> R {
        f(&mut self.index.lock().expect("GameIndex mutex poisoned"))
    }
}

impl GameProvider for LiveGameProvider {
    fn has_path(&self, path: &str) -> bool {
        self.index.lock().expect("poisoned").has_path(path)
    }
    fn pull_raw(&self, path: &str) -> Option<Vec<u8>> {
        self.index.lock().expect("poisoned").pull_path(path)
    }
    fn game_bin(&self, path: &str) -> Option<BinTree> {
        let bytes = self.pull_raw(path)?;
        match self.bin.parse_bytes(&bytes) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::debug!("game_bin parse failed for {}: {}", path, e);
                None
            }
        }
    }
}
```
- `main.rs` resolution (before `process::process_input`, near RepathOptions assembly at main.rs:243-271):
```rust
let live_provider: Option<LiveGameProvider> = if cli.no_live {
    None
} else {
    let install = match &cli.game_path {
        Some(p) => match hematite_live::LeagueInstall::from_path(p) {
            Ok(i) => Some(i),
            Err(e) => { tracing::warn!("--game-path invalid: {e}"); None }
        },
        None => hematite_live::detect_league(),
    };
    install.map(|i| {
        LiveGameProvider::new(
            hematite_live::GameIndex::new(&i),
            Box::new(hematite_file::bin_adapter::FileBinProvider::new()),
        )
    })
};
if live_provider.is_none() && !cli.no_live {
    tracing::info!("No League install detected — live-game fixes will be skipped (fail open)");
}
```
(Check the actual `FileBinProvider` constructor name via `grep -n "FileBinProvider" crates/hematite-file/src` — use whatever exists.)
- `ProcessContext` (`process.rs`, struct near the top — find `pub struct ProcessContext`) gains `pub live: Option<&'a LiveGameProvider>`. Every `FixContext` literal in process.rs sets `game: pctx.live.map(|l| l as &dyn GameProvider)` (adjust to local variable names at each site).
- **Champion index priming:** wherever seeds are discovered in process.rs before the per-BIN fix loop (grep `discover_seeds` in process.rs), add: for each seed champion + each related form from `champions.related(...)` (grep `CharacterRelations` methods for the exact name), call `live.with_index(|i| { i.add_champion(&champ); })`.
- `collect_selected_fixes` / `ALL_FIX_IDS`: append `"gear_pull"`, `"cac_pull"`, `"resolve_dead_refs"`, `"combo_bin_relocate"` to `ALL_FIX_IDS` (order: `gear_pull`, `cac_pull` right after `entry_validator`; `resolve_dead_refs` after `dds_to_tex`; `combo_bin_relocate` after `champion_bin_remover`). New flag mapping: `--restore-anm` is NOT a fix id (handled as pipeline step); no new per-fix flags needed beyond existing selection (config-driven).
- **anm interplap** in `main.rs` (or wherever selected fixes are finalized): if `cli.restore_anm && cli.remove_anm` → `tracing::warn!("--remove-anm and --restore-anm both set; --remove-anm wins")`, force restore off. Else if `cli.restore_anm` → remove `"anm_remover"` from the selected fix list.

- [ ] **Step 1: Write failing test** in `live_provider.rs`: build fixture WAD in a tempdir fake install (reuse Task 3 test shape), construct `LiveGameProvider` with the real `FileBinProvider`, assert `has_path` true/false and `pull_raw` bytes round-trip. (For `game_bin`, feed a real minimal BIN? Skip — parse is delegated; test `game_bin` returns None on garbage bytes instead.)
- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement** all modifications above. `cargo build --workspace` to chase compile errors through process.rs construction sites.
- [ ] **Step 4: Run** `cargo test --workspace` + clippy → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(cli): live game provider, --game-path/--no-live/--restore-anm flags"`

---

### Task 9: Deep repair via GameIndex (flag-less deep repair)

**Files:**
- Modify: `crates/hematite-cli/src/deep_repair.rs`, `crates/hematite-cli/src/process.rs` (call sites: repath gating at ~process.rs:783-803 and `extract_missing_from_game_wad` ~process.rs:1886)
- Test: existing deep_repair tests keep passing; new test for the source abstraction

**Interfaces:**
- New trait in `deep_repair.rs`:
```rust
/// Anything deep repair can pull game bytes from.
pub trait GamePullSource {
    fn hashes(&self) -> &std::collections::HashSet<u64>;
    fn extract(&mut self, hash: u64) -> anyhow::Result<Option<Vec<u8>>>;
}
```
- Impl 1 (existing behavior): `struct WadFileSource { wad: WadFile, hashes: HashSet<u64> }` with `WadFileSource::open(path) -> Result<Self>` (wraps `WadFile::open` + `chunk_hash_set()`).
- Impl 2: `struct LiveSource<'a> { provider: &'a LiveGameProvider, hashes: HashSet<u64> }` — `LiveSource::new(p: &LiveGameProvider)` snapshots `p.with_index(|i| i.hash_set())`; `extract` = `Ok(self.provider.with_index(|i| i.pull_hash(hash)))`.
- `pull_one` and `resolve_from_game_wad` refactor: `pull_one(requested, source: &mut dyn GamePullSource, all_files)` (drop the separate `game_hashes` param — use `source.hashes()`); public entry points:
  - `pub fn resolve_from_game_wad(game_wad_path: &Path, ...) -> Result<DeepRepairStats>` — kept, wraps `WadFileSource::open` + `resolve_from_source`.
  - `pub fn resolve_from_source(source: &mut dyn GamePullSource, all_files, bin_provider, opts) -> Result<DeepRepairStats>` — the moved body (drop the unused `_hash_provider` param while here).

**process.rs call-site change:** where deep repair currently gates on `opts.game_wad` (repath pipeline): keep that branch (explicit `--game-wad` wins); add an `else if let Some(live) = pctx.live` branch that builds `LiveSource::new(live)` and calls `resolve_from_source` — same conditions otherwise (repath active, not dry-run). Before building `LiveSource`, prime champion WADs from discovered seeds (already done in Task 8 priming; ensure ordering: priming happens before the repath pipeline runs).

- [ ] **Step 1: Write failing test** — in-memory `GamePullSource` mock (`HashMap<u64, Vec<u8>>`-backed) + assert `resolve_from_source` pulls a seed bin: mod ships `assets/characters/yone/base/yone.skn` only (asset-only mod), source contains `data/characters/yone/skins/skin0.bin` (fake `PROP` bytes via the existing `FakeBinProvider` fixture in deep_repair tests, linking one dep that the source also has) → stats.seed_bins_added == 1, both files appended. Note: seeds only discover from `skins/skinN.bin` paths — check `seeds::discover_seeds` matching; if an asset-only path can't seed, instead ship `data/characters/yone/skins/skin0.bin` in the mod referencing a missing dep and assert closure pull (files_pulled == 1). Read the existing tests first and extend in their style.
- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement refactor + call sites.**
- [ ] **Step 4: Run** `cargo test --workspace` + clippy → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(cli): deep repair pulls from auto-detected game install (GamePullSource)"`

---

### Task 10: `--restore-anm` (pull missing animations)

**Files:**
- Create: `crates/hematite-cli/src/restore_anm.rs`
- Modify: `crates/hematite-cli/src/main.rs` (module decl), `crates/hematite-cli/src/process.rs` (pipeline step)
- Test: inline in `restore_anm.rs` (mock GamePullSource, fake BIN provider)

**Interfaces:**
```rust
/// Pull .anm files referenced by the mod's BINs but missing from the mod,
/// out of the live game. Returns (restored, still_missing).
pub fn restore_missing_anms(
    all_files: &mut Vec<(u64, String, Vec<u8>)>,
    bin_provider: &dyn BinProvider,
    source: &mut dyn deep_repair::GamePullSource,
    skip_vo: bool,
) -> (u32, u32)
```
**Algorithm:** collect refs: for each BIN in `all_files` (path ends `.bin` or `looks_like_bin`), parse, `repath_core::collect_bin_asset_paths(&tree, skip_vo)`, filter `.anm` (lowercase). Build mod `WadIndex` (same as deep_repair's `rebuild_index`). For each ref missing from the index: `deep_repair::pull_one(ref, source, all_files)` → Some → restored += 1 else still_missing += 1. Dedup refs first (HashSet).

**Pipeline wiring (process.rs):** in the WAD-processing flow after WAD-level fixes & BIN collection but before repath (find where `all_files` is complete and `selected_fixes` is available): if the run was invoked with `--restore-anm` (plumb a `restore_anm: bool` through `ProcessContext`) and a live provider or `--game-wad` source exists → run it, log stats line `Restored N animation(s), M unresolved`. Works in both `process_wad_file` and `process_wad_folder` flows (call from the shared spot both use — if none exists, call in both).

- [ ] **Step 1: Write failing test** — mod ships one fake BIN referencing `x/attack1.anm` (extend the FakeBinProvider idea: since `collect_bin_asset_paths` needs real tree strings, build the `BinTree` with a `PropertyValue::String("x/attack1.anm")` property object directly and a mock BinProvider returning it), source has the anm bytes → restored == 1, `all_files` contains it; second ref `x/gone.anm` not in source → still_missing == 1.
- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run tests + clippy** → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(cli): --restore-anm pulls missing animations from game"`

---

### Task 11: Combo-bin relocation

**Files:**
- Create: `crates/hematite-cli/src/relocate.rs`
- Modify: `crates/hematite-cli/src/main.rs` (module decl), `crates/hematite-cli/src/process.rs` (pipeline step, gated on fix id `combo_bin_relocate`)
- Test: inline in `relocate.rs`

**Interfaces:**
```rust
/// Re-key legacy combo bins to Riot's relocated path.
/// `data/<champ>_skins_<slots>.bin` → `data/characters/<champ>/<champ>_multi_skins_<slots>.bin`
/// Gates: mod ships NO per-skin bins (`skins/skin<N>.bin`), and the relocated
/// path exists in the live game. Returns number of relocated entries.
pub fn relocate_combo_bins(
    all_files: &mut Vec<(u64, String, Vec<u8>)>,
    game_has_path: &dyn Fn(&str) -> bool,
) -> u32
```
**Algorithm:**
```rust
use regex::Regex; // already a workspace dep (used by core transforms) — confirm in cli Cargo.toml, add if absent
let combo_re = Regex::new(r"^data/(?P<champ>[a-z0-9_.]+)_skins_(?P<slots>[0-9_]+)\.bin$").unwrap();
let perskin_re = Regex::new(r"skins/skin\d+\.bin$").unwrap();
// Gate 1: any per-skin bin → return 0.
// For each file whose lowercased path matches combo_re:
//   new_path = format!("data/characters/{champ}/{champ}_multi_skins_{slots}.bin")
//   if game_has_path(&new_path) { entry.0 = wad_path_hash(&new_path); entry.1 = new_path; count += 1; log }
```
Call site: right after WAD extraction into `all_files` in both WAD flows, before BIN fix loop (relocated path affects seed discovery? No — combo bins aren't seeds; order vs restore_anm irrelevant). Gate: `selected_fixes.contains("combo_bin_relocate")` && live provider present (`game_has_path` = closure over `live.has_path`). With `--game-wad` but no live install: also allow via WadFileSource hash set closure — simplest is to pass whatever game lookup exists; if none → skip with debug log.

- [ ] **Step 1: Write failing tests** — (a) combo bin + no per-skin bins + game has new path → relocated (hash updated to `wad_path_hash(new_path)`, path rewritten); (b) per-skin bin present → 0; (c) game lacks new path → 0; (d) non-combo names untouched.
- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement + wire.**
- [ ] **Step 4: Run tests + clippy** → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(cli): combo-bin relocation to Riot's multi_skins path"`

---

### Task 12: Config rules for the new fixes

**Files:**
- Modify: `config/fix_config.json`
- Test: `cargo test --workspace` (config deserialization is covered by remote-config tests; additionally `cargo run -p hematite-cli -- --check-version` smoke-loads config)

- [ ] **Step 1: Add rules to `fix_config.json`** — bump `"version": "2.2.0"`, `"last_updated": "2026-07-10"`. Insert into `"fixes"` (after `entry_validator`):
```json
"gear_pull": {
    "name": "Dead Gear Link Repair",
    "description": "Pulls GearSkinUpgrade entries referenced by mGearSkinUpgrades but missing from the mod out of the live game BIN closure (confirmed crash otherwise). Nukes skinUpgradeData as last resort.",
    "enabled": true,
    "severity": "critical",
    "detect": {
        "type": "dead_entry_link",
        "main_entry_type": "SkinCharacterDataProperties",
        "targets": [
            { "entry_type": "GearSkinUpgrade", "type_hash": "0x27E0C761", "reference_field": "skinUpgradeData", "link_field": "0xcb522723" }
        ]
    },
    "apply": {
        "type": "pull_entries_from_game",
        "main_entry_type": "SkinCharacterDataProperties",
        "targets": [
            { "entry_type": "GearSkinUpgrade", "type_hash": "0x27E0C761", "reference_field": "skinUpgradeData", "link_field": "0xcb522723" }
        ],
        "nuke_fallback_field": "skinUpgradeData"
    }
},
"cac_pull": {
    "name": "Voiceover (CAC) Restore",
    "description": "Pulls ContextualActionData entries referenced by the skin but missing from the mod out of the live game BIN closure, restoring voiceovers. Unpullable links are dropped.",
    "enabled": true,
    "severity": "medium",
    "detect": {
        "type": "dead_entry_link",
        "main_entry_type": "SkinCharacterDataProperties",
        "targets": [
            { "entry_type": "ContextualActionData", "type_hash": "0xCF3A2F44", "reference_field": "contextualActionData", "link_field": "0xd8f64a0d" }
        ]
    },
    "apply": {
        "type": "pull_entries_from_game",
        "main_entry_type": "SkinCharacterDataProperties",
        "targets": [
            { "entry_type": "ContextualActionData", "type_hash": "0xCF3A2F44", "reference_field": "contextualActionData", "link_field": "0xd8f64a0d" }
        ]
    }
},
"resolve_dead_refs": {
    "name": "Dead Reference Ladder",
    "description": "Rewrites dead asset references to a live form using the installed game as ground truth: .dds↔.tex / .sco↔.scb twins and Riot's inner-suffix-stripped renames. Skips anything the mod or game actually ships.",
    "enabled": true,
    "severity": "high",
    "detect": {
        "type": "recursive_string_extension_not_in_wad",
        "extension": ".dds",
        "path_prefixes": []
    },
    "apply": {
        "type": "resolve_dead_refs",
        "extensions": ["dds", "tex", "anm", "skn", "skl", "scb", "sco"]
    }
}
```
Note on `resolve_dead_refs.detect`: the transform re-checks everything itself and is a safe no-op; the detection just needs to fire often enough. `recursive_string_extension_not_in_wad` with `.dds` misses pure-`.anm` cases — acceptable for v0.5.0 (detection triggers on the overwhelmingly common case; the transform then fixes all extensions in that BIN). Add a code comment? No — config JSON has no comments; this note lives in the rule's `description`.

`combo_bin_relocate` needs no JSON rule (CLI pipeline step keyed off the fix id) — but `--check`/reporting reads names from config; follow how `anm_remover`-style WAD rules surface and, if a rule entry is required for reporting, add a `wad_fixes` entry with `"detect": {"type": "file_pattern", "pattern": "data/*_skins_*.bin"}` and `"apply": {"type": "rename_file", "pattern": "^$", "replacement": ""}` acting as a descriptor only — otherwise skip. Prefer skipping if reporting works without it.

- [ ] **Step 2: Validate config loads** — `cargo test --workspace` (remote/fetch tests parse the local config) and run `cargo run -p hematite-cli -- --check-version` (loads config, exits) → no deserialization errors.
- [ ] **Step 3: Commit** — `git commit -am "feat(cli): config rules for gear_pull, cac_pull, resolve_dead_refs (v2.2.0)"`

---

### Task 13: RitoShark-Crates rev bump `fd2cb9d` → `daff556`

**Files:**
- Modify: `crates/hematite-file/Cargo.toml` (5 × `rev = "fd2cb9d"` → `rev = "daff556"`), `Cargo.lock`

- [ ] **Step 1: Diff the intervening commits first** (established process — cheap and de-risks the bump):
```powershell
git -C "e:\RitoShark - Crate\RitoShark-Crates" log --oneline fd2cb9d..daff556
git -C "e:\RitoShark - Crate\RitoShark-Crates" diff --stat fd2cb9d..daff556
```
(NOTE: the crates repo lives at `e:\RitoShark - Crate\RitoShark-Crates` per project memory — verify with `Test-Path`, fall back to `E:\RitoShark\Flint\RitoShark-Crates` which also exists.) For every API-facing change in `rs_io`/`rs_bin`/`rs_wad`/`rs_tex`/`rs_mesh`, grep hematite-file for usage of the touched items. Record findings in the commit message body.
- [ ] **Step 2: Bump** all five `rev = "fd2cb9d"` occurrences in `crates/hematite-file/Cargo.toml` to `daff556`, then `cargo update -p rs_io -p rs_bin -p rs_wad -p rs_tex -p rs_mesh` (or delete the five stale `Cargo.lock` blocks and `cargo build`).
- [ ] **Step 3: Build + full test + clippy** — `cargo build --workspace && cargo test --workspace` + clippy gate. Fix any breakage **inside hematite-file only** (adapter crate absorbs API churn — that's its job).
- [ ] **Step 4: Commit** — `git commit -am "chore(file): bump RitoShark-Crates rev fd2cb9d -> daff556"` (body: one line per intervening commit + impact).

---

### Task 14: End-to-end verification + release v0.5.0

**Files:**
- Modify: `Cargo.toml` (workspace version), `config/version.json`, `README.md`/`DEVELOPER.md` (new flags + crate mention — short)

- [ ] **Step 1: Full gate** — `cargo build --workspace --release`, `cargo test --workspace`, clippy gate. All green.
- [ ] **Step 2: Live smoke test** (machine has no League install? then verify fail-open): run `target/release/hematite-cli.exe <any test .fantome or .bin> --check` and confirm (a) no crash, (b) log line about live detection outcome, (c) fixes still run. If a League install IS present, additionally run with `--repath` on a champion mod and confirm deep repair pulls without `--game-wad`, and `--restore-anm` logs its stats line. Use any sample mod file available (ask the user if none is on disk — do not fabricate).
- [ ] **Step 3: Version bump** — workspace `version = "0.5.0"`; `config/version.json`: `latest_cli_version: "0.5.0"` (leave `min_cli_version` at 0.4.1). Update README flag table + DEVELOPER crate list (hematite-live). Commit: `git commit -am "chore: bump version to 0.5.0"`.
- [ ] **Step 4: Merge + tag + push**:
```powershell
git checkout main
git merge feat/ritoshark-migration
git tag v0.5.0
git push origin main
git push origin feat/ritoshark-migration
git push origin v0.5.0
```
CI (release.yml) builds and publishes the GitHub release from the tag. Confirm the Actions run goes green (`gh run watch` or check `gh run list --limit 1`).
- [ ] **Step 5: Update project memory** — update `v2-architecture.md` memory (new crate, new flags, 0.5.0) and `ritoshark-migration.md` (rev daff556).

---

## Self-review notes

- Spec coverage: crate (Tasks 1–3), trait (4), gear/CAC (5–6), ladder (7), CLI+auto-detect (8), deep repair flag-less (9), restore-anm (10), relocation (11), config (12), rev bump (13), release (14). ✔
- Type consistency: `GamePullSource::hashes()`/`extract()` used by Tasks 9–10; `LiveGameProvider::with_index` used by Tasks 8–11; `wad_path_hash` exists in BOTH hematite-live and hematite-file (deliberate duplication across the format boundary — do not unify).
- Known judgment calls for the implementer: exact `sysinfo` API per resolved version (Task 2), `FileBinProvider` constructor name (Task 8), seed-discovery reuse in deep repair test (Task 9), whether combo-relocation needs a config descriptor for reporting (Task 12). Each is bounded and verifiable by grep/compile.
