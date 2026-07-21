# Sub-project A — Lift `fix_folder` into a library (`hematite-orchestrate`)

## Context

This is the first of three sub-projects for the **Flint Skin Fixer** feature.
Flint will embed Hematite in-process to detect and fix broken skins in a
project folder. Today the full fix orchestration — extract WAD chunks → detect
issues → apply BIN/WAD transforms → recover from the live game → rebuild the
`.wad.client` — lives in a **2178-line binary module**
(`crates/hematite-cli/src/process.rs`) behind one `process_input()` fn.
`hematite-core::apply_fixes()` only mutates a single BIN tree, so nothing
reusable can fix a whole project. Flint (and the CLI) need that orchestration as
a **library** function.

Sub-projects B (Flint `hematite-flint` crate + Tauri commands) and C (the modal)
depend on this one and cannot compile without it.

## Goal

Extract the folder-processing orchestration out of `hematite-cli` into a new
crate `hematite-orchestrate`, exposing:

- `fix_folder(dir, config, selected_fixes, providers, opts, progress) -> FixReport`
  — the full extract→detect→fix→recover→rebuild pipeline for one WAD folder or a
  directory tree of them (the shape a Flint project `content/` dir has).
- A **detect-only** mode (drives Flint's "show only detected fixes" step):
  returns per-fix detection counts without writing anything.
- `list_fixes(config) -> Vec<FixInfo>` — id, name, description, severity for
  every fix in the config (BIN-level `fixes` ∪ WAD-level `wad_fixes`).

The CLI's `process_input` is rewired to call `fix_folder` for the folder case
(single source of truth — no behaviour drift, existing CLI tests stay green).

## Non-goals

- No new fix rules or engine behaviour changes. Pure refactor + one new
  detect-only code path (which mostly already exists as `--check`/`--dry-run`).
- No Flint code in this sub-project.
- `.fantome`/`.bin` single-file inputs can stay in the CLI for now — Flint only
  needs the **folder** path. (If cheap, `process_fantome_file` may also move, but
  it's not required.)

## Architecture

New crate `hematite-orchestrate`:

```
hematite-orchestrate  (new)
  → hematite-core     (detect + transform engine)
  → hematite-file     (BIN/WAD/texture providers — the rebuild needs these)
  → hematite-types    (FixConfig, ProcessResult, RepathOptions)
```

`hematite-core` stays format-crate-free (its stated invariant). The
orchestrator is the layer allowed to depend on `hematite-file`.

### What moves out of `hematite-cli/src/process.rs`

Into `hematite-orchestrate`:

- `process_wad_folder` → becomes the body of `fix_folder`.
- The WAD-folder helpers it calls: combo-bin relocation glue
  (`run_combo_bin_relocate`), deep-repair invocation
  (`resolve_from_game_wad`/`resolve_from_live` wrappers), restore-anm glue.
- The `deep_repair`, `anm_restore`, `combo_relocate` modules move from
  `hematite-cli/src/` into `hematite-orchestrate/src/` (they are pure
  orchestration over providers, no CLI concerns). `live_provider.rs`'s
  `LiveGameProvider` also moves (or the trait it implements does), since
  recovery needs it.

Stays in `hematite-cli`:

- Arg parsing, logging, remote config fetch, the version gate, the
  `UiReporter` (CLI progress bar), `main.rs` routing, `.fantome`/`.bin`
  single-file paths (unless trivially movable).

### Neutralising CLI-specific dependencies

`process_wad_folder` currently takes a `ProcessContext` bundling CLI types.
Replace with library-neutral inputs:

1. **Progress** — `UiReporter` (a CLI progress bar) → a `ProgressSink` trait:
   ```rust
   pub trait ProgressSink: Send + Sync {
       fn stage(&self, label: &str);          // "Extracting…", "Rebuilding WAD…"
       fn fix_applied(&self, name: &str, count: Option<u32>);
       fn note(&self, message: &str);
   }
   ```
   Provide a `NoopSink`. The CLI supplies an adapter wrapping its `UiReporter`;
   Flint supplies one emitting Tauri events (in sub-project B).

2. **Options** — a plain `FixOptions` struct replaces the loose `ctx` fields:
   ```rust
   pub struct FixOptions<'a> {
       pub dry_run: bool,
       pub detect_only: bool,     // NEW: detect + count, never write
       pub repath: Option<&'a RepathOptions>,
       pub restore_anm: bool,
       pub relocate_combo_bins: bool,
       pub game_wad: Option<&'a Path>,
       pub live: Option<&'a dyn LiveGameAccess>,  // trait, not the CLI struct
   }
   ```

3. **Providers** — `hash_provider: &Arc<dyn HashProvider>`, plus the
   `FileBinProvider`/`FileWadProvider` from `hematite-file` (constructed inside
   `fix_folder`, as `process_wad_folder` does today).

### Report shape

Reuse `hematite_types::result::ProcessResult` (already carries
`applied_fixes: Vec<AppliedFix{fix_id,fix_name,changes_count,file_path}>`,
`fixes_applied/failed`, `files_removed`, `errors`, `check_info`). `fix_folder`
returns it. In `detect_only` mode each detected fix is recorded as an
`AppliedFix` with `changes_count` = detection count and **nothing is written to
disk** (mirrors the existing `dry_run` branch in `apply_fixes`, extended to the
WAD-level fixes and the folder walk).

`FixInfo` for `list_fixes`:
```rust
pub struct FixInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub severity: String,   // low|medium|high|critical
    pub wad_level: bool,     // came from wad_fixes vs fixes
}
```

## Rev-lock

Hematite ALREADY pins every `rs_*` crate at `daff556` — the exact rev Flint
pins `ritoshark` at. Do not bump it in this sub-project; keeping them identical
is what lets Flint embed Hematite without compiling two copies of `rs_*`. If a
future change bumps one, both must move together (ecosystem library-first rule).

## Testing

- **Golden-path parity**: a test that runs `fix_folder` over a fixture WAD
  folder and asserts the resulting `ProcessResult` matches what the pre-refactor
  `process_wad_folder` produced (same `applied_fixes` ids + counts). Fixture
  under the gitignored test-data dir (`.gitkeep` committed; no real game
  assets).
- **detect_only writes nothing**: run `fix_folder` with `detect_only: true`
  over a folder with known issues; assert issues are reported with counts AND
  the folder bytes are byte-identical afterwards (no file written/removed).
- **list_fixes coverage**: assert `list_fixes(embedded_config)` returns every id
  in `ALL_FIX_IDS` with a non-empty name/description (folds the existing
  `all_fix_ids_exist_in_repo_config` guard into the new API).
- **CLI still green**: existing `hematite-cli` tests pass unchanged after
  `process_input` is rewired to call `fix_folder`.
- `cargo test --workspace`, `cargo clippy --workspace -- -D warnings
  -A clippy::needless_return`, `cargo fmt --all -- --check`.

## Files

- Create: `crates/hematite-orchestrate/Cargo.toml`, `.../src/lib.rs`,
  `.../src/fix_folder.rs` (the lifted `process_wad_folder`),
  `.../src/progress.rs` (`ProgressSink` + `NoopSink`),
  `.../src/options.rs` (`FixOptions`, `LiveGameAccess` trait),
  `.../src/list_fixes.rs`.
- Move: `crates/hematite-cli/src/deep_repair.rs`, `anm_restore.rs`,
  `combo_relocate.rs`, and the `LiveGameProvider`/`LiveGameAccess` glue →
  `crates/hematite-orchestrate/src/`.
- Modify: `crates/hematite-cli/src/process.rs` (folder branch → call
  `fix_folder`; add a `UiReporter`→`ProgressSink` adapter), `main.rs`
  (dependency wiring), root `Cargo.toml` (add the new workspace member).

## Commit / push

Conventional commits with scopes (`refactor(orchestrate): …`,
`feat(orchestrate): add fix_folder`). After it's green, commit + push Hematite
`main` so Flint can pin the new crate.
