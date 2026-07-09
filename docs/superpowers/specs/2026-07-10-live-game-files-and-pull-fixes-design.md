# Live Game Files Module + Celestial Pull-Fix Port — Design

Date: 2026-07-10
Target release: v0.5.0
Sources studied: Celestial (`E:\RitoShark\Celestial`, post-pull @ 21a4d58), Flint - Main (`E:\RitoShark\Flint\Flint - Main`), hematite-v2 @ f66fb5d.

## Goal

1. Give hematite first-class access to the user's installed League of Legends game files ("live game files") via a new, fully standalone crate `hematite-live` — auto-detection of the install plus TOC-only WAD reading, so no flag is required.
2. Port the five highest-value Celestial repair capabilities that depend on live game files:
   - Animation restore (pull dead `.anm` from game instead of deleting)
   - Gear pull repair (dead `mGearSkinUpgrades` link = confirmed crash)
   - CAC voiceover pull repair
   - Combo-bin relocation (`data/<champ>_skins_<slots>.bin` → `_multi_skins_` re-key)
   - Game-TOC-aware dead-ref resolution ladder
3. Bump RitoShark-Crates rev `fd2cb9d` → `daff556`, version 0.4.1 → 0.5.0, tag and push the release.

Explicitly out of scope for v0.5.0 (deferred): Celestial's detect-only validators (replaced-bin crash rules, dead-UI-link ground truth, skin-mesh crash, dead-VFX thresholds, mapgeo/cubemap map checks, luabin security, DDS playability), VFX companion-bin pull, WAD auto-rename. Flint is reference-only — no changes there.

## 1. New crate: `crates/hematite-live`

Standalone library. **No dependency on rs_\* or any hematite crate.** Dependencies: `sysinfo` (process scan), `flate2` (gzip), `zstd`, `xxhash-rust/xxh64`, `walkdir`, `serde_json` (RiotClientInstalls.json), `log`.

### `detect.rs` — League install detection
Port of Flint's chain (`ltk_mod_core::league_path` + `flint-ltk/league/detector.rs`), in priority order:
1. `%SystemDrive%\ProgramData\Riot Games\RiotClientInstalls.json` → `associated_client` entries whose folder is exactly `League of Legends` (PBE excluded) → verify `<path>/Game/League of Legends.exe`.
2. Running processes via `sysinfo`: `LeagueClientUx.exe`, `LeagueClient.exe`, `League of Legends.exe` → exe parent → verify `Game/League of Legends.exe`.
3. Common paths on all disks (fallback drives `C:`–`H:`): `Riot Games/League of Legends`, `Program Files/Riot Games/League of Legends`, `Program Files (x86)/...`.
4. Registry: `reg query "HKLM\SOFTWARE\WOW6432Node\Riot Games, Inc\League of Legends" /v Location` (shell-out, no winreg dep).

These names/paths are unavoidable hardcoding; they live in one `consts` block at the top of `detect.rs`.

API:
```rust
pub struct LeagueInstall { pub root: PathBuf, pub game_dir: PathBuf, pub auto_detected: bool }
pub fn detect_league() -> Option<LeagueInstall>;
impl LeagueInstall {
    /// Accepts either the install root or the Game/ dir; validates.
    pub fn from_path(p: &Path) -> Result<LeagueInstall, LiveError>;
}
```
Validation: `Game/League of Legends.exe` exists (accept a path that IS the Game dir too).

### `toc.rs` — TOC-only WAD reader
Port of Flint `wad_jade/{format,reader}.rs`. Magic `RW` (0x5752), major must be 3; v3.1 and v3.4+ chunk record layouts. Reads header + chunk table only — **never** the payload (rs_wad's full-load behavior is why this is reimplemented).
```rust
pub struct WadToc { pub path: PathBuf, pub version: (u8, u8), pub chunks: Vec<TocChunk> }
pub struct TocChunk { pub path_hash: u64, pub offset: u64, pub compressed_size: u64,
                      pub uncompressed_size: u64, pub compression: Compression }
pub enum Compression { None, GZip, Satellite, Zstd, ZstdMulti }
pub fn read_toc(path: &Path) -> Result<WadToc, LiveError>;
```

### `chunk.rs` — on-demand chunk read
`pub fn read_chunk(wad_path, &TocChunk) -> Result<Vec<u8>, LiveError>` — seek/read/decompress (None passthrough, GZip=flate2, Zstd/ZstdMulti=zstd; Satellite → error). One `File` handle cached per WAD inside `GameIndex`.

### `wads.rs` — enumeration
- `pub fn enumerate_wads(game_dir) -> Vec<GameWadInfo { path, name, category }>` — WalkDir over `<game_dir>/DATA/FINAL`, depth ≤ 5, `.wad`/`.wad.client`, category = parent dir name (`Champions`, `Maps`, ...).
- `pub fn champion_wad(game_dir, champion) -> PathBuf` — `<game_dir>/DATA/FINAL/Champions/<Champion>.wad.client` (case-insensitive lookup against the real dir listing so casing never breaks).

### `index.rs` — `GameIndex`
The consumer-facing façade:
```rust
pub struct GameIndex { /* install, loaded TOCs, hash→(wad_idx, chunk_idx) map, open file handles */ }
impl GameIndex {
    pub fn new(install: &LeagueInstall) -> GameIndex;          // no TOCs loaded yet
    pub fn add_wad(&mut self, path: &Path) -> Result<(), LiveError>;   // lazy, idempotent
    pub fn add_champion(&mut self, champion: &str) -> Result<(), LiveError>;
    pub fn has_hash(&self, h: u64) -> bool;
    pub fn has_path(&self, p: &str) -> bool;                   // xxh64(lowercase)
    pub fn pull_hash(&mut self, h: u64) -> Option<Vec<u8>>;
    pub fn pull_path(&mut self, p: &str) -> Option<Vec<u8>>;
}
pub fn wad_path_hash(p: &str) -> u64;   // xxh64 of lowercased path
```
Scoping rule for v0.5.0: the CLI adds only the champion WADs for detected seeds plus related forms (via existing `character_relations`). The API supports adding Maps/UI later.

## 2. hematite-core: `GameProvider` trait

`hematite-core` must stay format-free, so it consumes live files through a trait (added in `traits.rs` alongside `BinProvider`/`WadProvider`):
```rust
pub trait GameProvider {
    fn has_path(&self, path: &str) -> bool;
    fn pull_raw(&mut self, path: &str) -> Option<Vec<u8>>;
    /// Pull AND parse a game BIN into hematite's tree model (parsing happens in the impl).
    fn game_bin(&mut self, path: &str) -> Option<BinTree>;
}
```
`FixContext` gains `game: Option<&mut dyn GameProvider>`. The impl (`LiveGameProvider`) lives in **hematite-cli**, wrapping `hematite_live::GameIndex` + `hematite_file::FileBinProvider` for parsing — same pattern as `deep_repair` (which sits in the CLI for exactly this reason).

## 3. Ported fixes

### 3a. Gear pull repair (crash fix) — core transform, config-driven
New `DetectionRule::DeadEntryLink { main_entry_type, targets: [EntryValidationTarget] }`: fires when a link field on a main entry references a `path_hash` defined in neither the tree, the linked trees, nor the merge closure. (Inverse of the existing `UnreferencedEntryOfType`.)

New `TransformAction::PullEntriesFromGame { targets, nuke_fallback: Option<String> }`:
1. Collect dead link hashes per target class (gear: `mGearSkinUpgrades` / `0xcb522723` on `SkinCharacterDataProperties`).
2. For each seed `(champion, skin)` (already discovered by the pipeline), BFS the **game's** BIN closure: `game_bin(canonical skin path)` → follow `linked` via `game_bin` (bounded, cycle-guarded — same guards as deep_repair).
3. Objects in the game closure whose `path_hash` matches a dead link → clone into the mod tree.
4. Unpullable gear links → nuke fallback: remove the `skinUpgradeData` embed field from the referencing entry (Celestial's `nuke_gear_skin_data` semantics).

Config rule `gear_pull` (enabled, severity critical). Runs **before** `entry_validator` (pull first, then cleanup — no conflict: validator removes unreferenced, pull adds referenced).

### 3b. CAC voiceover pull — same machinery
Config rule `cac_pull` (enabled, severity medium): target `ContextualActionData` via `contextualActionData` / `0xd8f64a0d`. Unpullable → drop the dead link only (warning, no nuke) — matches Celestial.

### 3c. Animation restore — CLI, reuses deep-repair pull
New flag `--restore-anm` (off by default): collect `.anm` string refs from all mod BINs; for each ref missing from the mod WAD, pull from the game index using the existing resolution ladder in `wad_adapter` (exact hash → canonical reconstruction anchored at `characters/` → Riot suffix-strip rename). Implemented as a filtered mode over deep_repair's `pull_one` machinery. Mutually exclusive with `anm_remover`: passing `--restore-anm` disables the `anm_remover` WAD fix for that run (and vice versa `--remove-anm` wins if both given, with a warning).

### 3d. Combo-bin relocation — CLI WAD-stage, config-toggled
Rule `combo_bin_relocate` (enabled): if the mod ships `data/<champ>_skins_<slots>.bin` (regex on resolved paths), ships **no** per-skin bins (Celestial's gate — relocating otherwise orphans them), and the game index confirms `data/characters/<champ>/<champ>_multi_skins_<slots>.bin` exists → re-key the WAD entry to the new hash. Pure rename, content untouched.

### 3e. Game-TOC dead-ref ladder — core transform
New `TransformAction::ResolveDeadRefs { extensions }` (config rule `resolve_dead_refs`, enabled; extensions: dds, tex, anm, skn, skl, scb, sco): for each asset-path string:
- in mod WAD → skip; in game index (exact) → skip (game provides it);
- else ladder: extension twin (`.dds`↔`.tex`, `.sco`↔`.scb`) in mod → rewrite; twin in game → rewrite; inner-suffix-strip (`foo.theme.anm`→`foo.anm`) in game → rewrite; strip+twin → rewrite;
- nothing → leave (repath placeholders handle textures downstream).
Runs after format converters, before repath. Never rewrites strings whose exact file the mod ships (same guard as `ReplaceStringExtension`). No-op when no game index is available.

## 4. CLI integration

New flags (`args.rs`):
- `--game-path <DIR>` — explicit install root or Game dir.
- `--no-live` — disable all live-game features (also the graceful path when detection fails: warn once, skip live-dependent fixes, everything else still runs — Celestial's fail-open invariant).
- `--restore-anm` — see 3c.
- `--game-wad <PATH>` kept as narrow override (single-WAD deep repair, unchanged semantics).

Resolution order for the game index: `--game-wad` (single WAD) → `--game-path` → auto-detect → none (fail open). Deep repair no longer requires any flag: with `--repath` and a detected install, seeds → `add_champion` per seed champion + related forms → existing backfill/closure runs against the index. `ALL_FIX_IDS` gains `gear_pull`, `cac_pull`, `combo_bin_relocate`, `resolve_dead_refs` (not `restore_anm` — explicit flag only, since `anm_remover` is default-on).

## 5. Config schema changes

`fix_config.json` → version 2.2.0: add `gear_pull`, `cac_pull`, `resolve_dead_refs` (BIN fixes), `combo_bin_relocate` (WAD-stage). New serde variants: `DetectionRule::DeadEntryLink`, `TransformAction::{PullEntriesFromGame, ResolveDeadRefs}`. All new variants are additive — old configs keep deserializing (serde `deny_unknown_fields` is not set today; keep it that way).

## 6. Dependency bump + release

1. In `e:\RitoShark\RitoShark - Crate\RitoShark-Crates`: `git log fd2cb9d..daff556` and diff against hematite-file's actual API usage before bumping (established process — see memory `ritoshark-migration`). Bump `rev` in `crates/hematite-file/Cargo.toml` for all five `rs_*` deps, `cargo update` the lockfile, fix any breakage inside hematite-file only.
2. Workspace `version = "0.5.0"`; `config/version.json` `latest_cli_version: 0.5.0` (`min_cli_version` stays 0.4.1).
3. Conventional commits on `feat/ritoshark-migration` (no Co-Authored-By), merge → `main`, tag `v0.5.0`, push `main` + tag (CI: git-cliff changelog + Windows build + GitHub Release).

## 7. Error handling

- `hematite-live`: everything returns `Result<_, LiveError>` (thiserror-style enum: Io, BadMagic, UnsupportedVersion, UnsupportedCompression, NotFound, DetectFailed). No panics on malformed WADs.
- Pipeline: absence of a game index is never an error — live-dependent fixes log `skipped (no game files)` and the run proceeds.
- Pull failures (chunk missing, decompress error) are per-item: log, count in stats, continue (mirrors deep_repair's `missing_unresolved`).

## 8. Testing

- `hematite-live` unit tests: TOC parse against synthetic in-memory WAD fixtures (v3.1 + v3.4, all compression kinds via real flate2/zstd round-trips); `from_path` validation; `wad_path_hash` vectors; `GameIndex` pull against a temp-dir fixture WAD.
- `hematite-core`: `DeadEntryLink` detection + `PullEntriesFromGame` (mock `GameProvider` over fixture `BinTree`s — pull, nuke fallback, cycle guard); `ResolveDeadRefs` ladder cases (mod-twin, game-twin, suffix-strip, exact-in-game skip).
- CLI: combo-relocation gate tests; flag interactions (`--restore-anm` vs `--remove-anm`).
- Gate: `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --lib --bins -- -D warnings -A clippy::needless_return`.
