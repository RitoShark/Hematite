# Sub-project A — `fix_folder` library lift — Implementation Plan

> **For agentic workers:** implement task-by-task, in order. Steps use checkbox syntax. This is a REFACTOR: existing `hematite-cli` tests MUST stay green at every task boundary. Do NOT change fix behaviour.

**Goal:** Extract the WAD-folder fix orchestration out of the `hematite-cli` binary into a new library crate `hematite-orchestrate`, exposing `fix_folder(...)`, a detect-only mode, and `list_fixes(...)`, so Flint (and the CLI) can drive the full extract→detect→fix→recover→rebuild pipeline from a library.

**Architecture:** New crate `hematite-orchestrate` depends on `hematite-core` + `hematite-file` + `hematite-types`. It receives library-neutral inputs: a `ProgressSink` trait (replacing the CLI's `UiReporter`), a `FixOptions` struct, a `HashProvider` (constructed by the caller), and an optional `&dyn GameFileAccess` (trait over the live game, replacing the concrete `LiveGameProvider`). The CLI's `process_input` folder branch becomes a thin adapter that calls `fix_folder`. Spec: `docs/superpowers/specs/2026-07-21-fix-folder-library-lift-design.md`.

**Tech Stack:** Rust 2021, workspace crates. Test: `cargo test --workspace`. Lint: `cargo clippy --workspace -- -D warnings -A clippy::needless_return`. Format: `cargo fmt --all`.

## Global Constraints

- Rust edition 2021, Rust 1.75+.
- Every `rs_*` crate stays pinned at rev `daff556` (matches Flint's ritoshark rev — do NOT bump).
- `hematite-core` keeps its ZERO-format-crate-import invariant. The new orchestrate crate is the layer allowed to depend on `hematite-file`.
- No fix-behaviour change: this is a pure lift + one new detect-only code path.
- Conventional commits with scopes; no `Co-Authored-By`; never sign. Commit after each task.
- CLI tests green at every task boundary.

## File Structure

- `crates/hematite-orchestrate/Cargo.toml` — new crate manifest.
- `crates/hematite-orchestrate/src/lib.rs` — re-exports + crate docs.
- `crates/hematite-orchestrate/src/progress.rs` — `ProgressSink` trait + `NoopSink`.
- `crates/hematite-orchestrate/src/game_access.rs` — `GameFileAccess` trait (library-neutral live-game surface).
- `crates/hematite-orchestrate/src/options.rs` — `FixOptions` struct.
- `crates/hematite-orchestrate/src/list_fixes.rs` — `FixInfo` + `list_fixes`.
- `crates/hematite-orchestrate/src/fix_folder.rs` — the lifted `process_wad_folder` body → `fix_folder`.
- `crates/hematite-orchestrate/src/deep_repair.rs`, `anm_restore.rs`, `combo_relocate.rs` — MOVED from `hematite-cli/src/`.
- `crates/hematite-cli/src/process.rs` — folder branch calls `fix_folder`; add `UiReporter → ProgressSink` adapter; single-file (`.fantome`/`.bin`) paths unchanged.
- `crates/hematite-cli/src/main.rs` — build the `GameFileAccess` adapter around `LiveGameProvider`; wire deps.
- Root `Cargo.toml` — add `crates/hematite-orchestrate` to `members`.

---

### Task 1: Scaffold the `hematite-orchestrate` crate

**Files:**
- Create: `crates/hematite-orchestrate/Cargo.toml`, `crates/hematite-orchestrate/src/lib.rs`
- Modify: root `Cargo.toml` (workspace `members`)

**Produces:** an empty compiling crate other tasks fill in.

- [ ] **Step 1:** Read the root `Cargo.toml` and an existing leaf crate's `Cargo.toml` (e.g. `crates/hematite-core/Cargo.toml`) to copy the workspace-dependency style (`.workspace = true` or path deps, edition, license fields).
- [ ] **Step 2:** Create `crates/hematite-orchestrate/Cargo.toml`:

```toml
[package]
name = "hematite-orchestrate"
version.workspace = true
edition.workspace = true

[dependencies]
hematite-types = { path = "../hematite-types" }
hematite-core = { path = "../hematite-core" }
hematite-file = { path = "../hematite-file" }
anyhow = { workspace = true }
tracing = { workspace = true }
walkdir = { workspace = true }
# NOTE: match the version/workspace form the other crates use for these — if the
# workspace doesn't define them as workspace deps, copy the concrete versions
# from hematite-cli/Cargo.toml (anyhow, tracing, walkdir are all used in process.rs).
```

- [ ] **Step 3:** Create `crates/hematite-orchestrate/src/lib.rs`:

```rust
//! Folder-level fix orchestration for Hematite.
//!
//! Lifts the extract → detect → fix → recover → rebuild pipeline out of the
//! CLI so both the CLI and embedders (Flint) drive it from a library.

pub mod anm_restore;
pub mod combo_relocate;
pub mod deep_repair;
pub mod fix_folder;
pub mod game_access;
pub mod list_fixes;
pub mod options;
pub mod progress;

pub use fix_folder::fix_folder;
pub use game_access::GameFileAccess;
pub use list_fixes::{list_fixes, FixInfo};
pub use options::FixOptions;
pub use progress::{NoopSink, ProgressSink};
```

- [ ] **Step 4:** Add `"crates/hematite-orchestrate"` to the root `Cargo.toml` `[workspace] members` list.
- [ ] **Step 5:** Temporarily comment out the `pub mod` lines in `lib.rs` that don't exist yet (all of them) so the crate compiles empty, OR create empty stub files. Prefer empty stubs: `touch` each module file with a `//! stub` line. Run `cargo build -p hematite-orchestrate` → expect success.
- [ ] **Step 6:** Commit `chore(orchestrate): scaffold crate`.

---

### Task 2: `ProgressSink` trait + `NoopSink`

**Files:**
- Create: `crates/hematite-orchestrate/src/progress.rs`

**Interfaces — Produces:**
```rust
pub trait ProgressSink: Send + Sync {
    fn stage(&self, label: &str);
    fn fix_applied(&self, name: &str, count: Option<u32>);
    fn note(&self, message: &str);
}
pub struct NoopSink;
impl ProgressSink for NoopSink { /* all no-ops */ }
```
These three methods mirror exactly the `UiReporter` methods that `process_wad_folder` and its helpers call (`ui.stage`, `ui.fix_applied`, `ui.note`). Confirm by grepping `process.rs`/`deep_repair.rs`/`anm_restore.rs`/`combo_relocate.rs` for `ui.` — if any OTHER `UiReporter` method is called inside the code being moved (e.g. `ui.tick`, `ui.set_length`), ADD it to the trait with a defaulted no-op body so callers compile.

- [ ] **Step 1:** Grep the to-be-moved code for every `ui.<method>(` call: `rg 'ui\.\w+\(' crates/hematite-cli/src/process.rs crates/hematite-cli/src/deep_repair.rs crates/hematite-cli/src/anm_restore.rs crates/hematite-cli/src/combo_relocate.rs`. List the distinct method names.
- [ ] **Step 2:** Write `progress.rs` with `ProgressSink` containing one method per distinct call found (at minimum `stage`, `fix_applied`, `note`), each with a sensible signature matching `UiReporter`'s. Give `set_length`/`tick`-style methods **default empty bodies** so an embedder needn't implement them.
- [ ] **Step 3:** Add `NoopSink` implementing the trait (all bodies empty).
- [ ] **Step 4:** `cargo build -p hematite-orchestrate` → expect success.
- [ ] **Step 5:** Commit `feat(orchestrate): add ProgressSink trait`.

---

### Task 3: `GameFileAccess` trait (neutralise `LiveGameProvider`)

**Files:**
- Create: `crates/hematite-orchestrate/src/game_access.rs`

**Context:** the moved code uses `live: Option<&LiveGameProvider>` (a CLI type wrapping `hematite-live` and implementing `hematite_core::traits::GameProvider`). The library can't depend on the CLI. Determine the minimal surface the moved code actually needs from `live`.

**Interfaces — Produces:** a `GameFileAccess` trait exposing only the methods `deep_repair`/`combo_relocate`/`anm_restore` call on `live`.

- [ ] **Step 1:** Grep the to-be-moved code for `live.` / `LiveGameProvider` usages: `rg 'live[\.:]|LiveGameProvider' crates/hematite-cli/src/deep_repair.rs crates/hematite-cli/src/anm_restore.rs crates/hematite-cli/src/combo_relocate.rs crates/hematite-cli/src/process.rs`. Note the exact methods/traits used.
- [ ] **Step 2:** Read `crates/hematite-cli/src/live_provider.rs` and `crates/hematite-core/src/traits.rs` (the `GameProvider` trait). Decide: if the moved code only ever uses `live` as a `&dyn GameProvider` (core's trait), then **reuse `hematite_core::traits::GameProvider`** as the boundary — no new trait needed; `FixOptions.live` becomes `Option<&'a dyn GameProvider>`. If it calls CLI-specific inherent methods on `LiveGameProvider`, define `GameFileAccess` with exactly those methods and have the CLI impl it for `LiveGameProvider`.
- [ ] **Step 3:** Write `game_access.rs`. If reusing `GameProvider`, make this module a thin re-export (`pub use hematite_core::traits::GameProvider as GameFileAccess;`) so `lib.rs`'s `pub use` stays valid. Otherwise define the minimal trait.
- [ ] **Step 4:** `cargo build -p hematite-orchestrate` → success.
- [ ] **Step 5:** Commit `feat(orchestrate): add GameFileAccess boundary`.

---

### Task 4: Move `deep_repair`, `anm_restore`, `combo_relocate` into the crate

**Files:**
- Move: `crates/hematite-cli/src/deep_repair.rs` → `crates/hematite-orchestrate/src/deep_repair.rs`
- Move: `crates/hematite-cli/src/anm_restore.rs` → `crates/hematite-orchestrate/src/anm_restore.rs`
- Move: `crates/hematite-cli/src/combo_relocate.rs` → `crates/hematite-orchestrate/src/combo_relocate.rs`
- Modify: `crates/hematite-cli/src/main.rs` (remove `mod` decls for the moved modules), and any `crate::deep_repair::`/`crate::anm_restore::`/`crate::combo_relocate::` references in remaining CLI files (they become `hematite_orchestrate::…`).

**Consumes:** Tasks 2 (`ProgressSink`) and 3 (`GameFileAccess`).

- [ ] **Step 1:** `git mv` the three files into `crates/hematite-orchestrate/src/`. Keep filenames identical.
- [ ] **Step 2:** In each moved file, fix imports: `crate::ui::UiReporter` → `crate::progress::ProgressSink` (change the parameter type from `&UiReporter`/`UiReporter` to `&dyn ProgressSink`); `crate::live_provider::LiveGameProvider` → the Task-3 boundary type; any `crate::<other-cli-module>::` reference that ISN'T one of the three moved modules must be resolved (see Step 3).
- [ ] **Step 3:** For any remaining `crate::`-reference in the moved files pointing at a CLI module that is NOT moving (e.g. a small helper in `process.rs`), either (a) move that helper too if it's pure orchestration, or (b) inline it into the orchestrate crate. Grep first: `rg 'crate::' crates/hematite-orchestrate/src/deep_repair.rs crates/hematite-orchestrate/src/anm_restore.rs crates/hematite-orchestrate/src/combo_relocate.rs`. Resolve each.
- [ ] **Step 4:** In `hematite-cli/src/main.rs`, delete the `mod deep_repair;` / `mod anm_restore;` / `mod combo_relocate;` declarations. In `process.rs`, change `crate::deep_repair::` → `hematite_orchestrate::deep_repair::` (and the other two).
- [ ] **Step 5:** Add `hematite-orchestrate = { path = "../hematite-orchestrate" }` to `crates/hematite-cli/Cargo.toml` `[dependencies]`.
- [ ] **Step 6:** `cargo build -p hematite-orchestrate` then `cargo build -p hematite-cli`. Resolve errors until both compile. The unit tests that lived inside those three files now run under the orchestrate crate — `cargo test -p hematite-orchestrate` must pass.
- [ ] **Step 7:** `cargo test -p hematite-cli` — expect green (CLI still works, now calling into the moved modules).
- [ ] **Step 8:** Commit `refactor(orchestrate): move deep-repair/anm-restore/combo-relocate out of CLI`.

---

### Task 5: `FixOptions` struct + `list_fixes`

**Files:**
- Create: `crates/hematite-orchestrate/src/options.rs`
- Create: `crates/hematite-orchestrate/src/list_fixes.rs`

**Interfaces — Produces:**
```rust
// options.rs
pub struct FixOptions<'a> {
    pub dry_run: bool,
    pub detect_only: bool,
    pub repath: Option<&'a hematite_types::repath::RepathOptions>,
    pub restore_anm: bool,
    pub relocate_combo_bins: bool,
    pub game_wad: Option<&'a std::path::Path>,
    pub live: Option<&'a dyn crate::game_access::GameFileAccess>,
}

// list_fixes.rs
pub struct FixInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub severity: String,
    pub wad_level: bool,
}
pub fn list_fixes(config: &hematite_types::config::FixConfig) -> Vec<FixInfo>;
```

- [ ] **Step 1:** Read `hematite_types::config::FixConfig` to confirm the field names on a fix entry (`name`, `description`, `severity`, `enabled`) and that `fixes` (BIN) + `wad_fixes` (WAD) are the two maps. The spec + DEVELOPER.md say each entry has `name`/`description`/`enabled`/`severity`.
- [ ] **Step 2:** Write `options.rs` with the `FixOptions` struct above.
- [ ] **Step 3:** Write `list_fixes.rs`: iterate `config.fixes` (wad_level=false) then `config.wad_fixes` (wad_level=true), mapping each to `FixInfo`. Include disabled ones too (Flint may show them), but you MAY add an `enabled: bool` field to `FixInfo` if convenient — keep it minimal.
- [ ] **Step 4: Write the failing test** in `list_fixes.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lists_every_fix_with_name_and_description() {
        // Load the repo's embedded config the same way the CLI test does.
        let raw = std::fs::read_to_string(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/fix_config.json"),
        ).unwrap();
        let config: hematite_types::config::FixConfig = serde_json::from_str(&raw).unwrap();
        let infos = list_fixes(&config);
        assert!(!infos.is_empty());
        for i in &infos {
            assert!(!i.name.trim().is_empty(), "fix {} has empty name", i.id);
            assert!(!i.description.trim().is_empty(), "fix {} has empty description", i.id);
        }
    }
}
```
(Add `serde_json` as a dev-dependency of orchestrate if not already present — copy the version the CLI uses.)

- [ ] **Step 5:** Run `cargo test -p hematite-orchestrate list_fixes` → expect FAIL (function empty/unimplemented).
- [ ] **Step 6:** Implement `list_fixes` fully → run again → expect PASS.
- [ ] **Step 7:** Commit `feat(orchestrate): add FixOptions + list_fixes`.

---

### Task 6: Lift `process_wad_folder` → `fix_folder`

**Files:**
- Create: `crates/hematite-orchestrate/src/fix_folder.rs`
- Modify: `crates/hematite-cli/src/process.rs` (delete `process_wad_folder`; folder branch calls `fix_folder`; add adapter)

**Interfaces — Produces:**
```rust
pub fn fix_folder(
    folder: &std::path::Path,
    config: &hematite_types::config::FixConfig,
    selected_fixes: &[String],
    champions: &hematite_types::champion::CharacterRelations,
    hash_provider: &std::sync::Arc<dyn hematite_core::traits::HashProvider>,
    opts: &crate::options::FixOptions<'_>,
    progress: &dyn crate::progress::ProgressSink,
) -> anyhow::Result<hematite_types::result::ProcessResult>;
```

- [ ] **Step 1:** Copy the ENTIRE body of `process_wad_folder` (currently `crates/hematite-cli/src/process.rs` ~line 1182 to its closing brace) into `fix_folder.rs` as the body of `fix_folder`. Read it fully first to capture every helper it calls.
- [ ] **Step 2:** Rewrite the parameter surface: the old fn took `(folder, ctx: &ProcessContext, hash_provider)`. Replace every `ctx.<field>` access with the matching `opts.<field>` or the direct param: `ctx.config`→`config`, `ctx.selected_fixes`→`selected_fixes`, `ctx.champions`→`champions`, `ctx.dry_run`→`opts.dry_run`, `ctx.check`→ (see Step 4, detect-only), `ctx.repath_opts`→`opts.repath`, `ctx.ui`→`progress`, `ctx.live`→`opts.live`, `ctx.restore_anm`→`opts.restore_anm`, `ctx.game_wad`→`opts.game_wad`, `ctx.relocate_combo_bins`→`opts.relocate_combo_bins`.
- [ ] **Step 3:** Fix `use` paths in `fix_folder.rs`: the helpers it calls (`run_combo_bin_relocate`, deep-repair/anm wrappers) now live in `crate::combo_relocate` / `crate::deep_repair` / `crate::anm_restore` (same crate). `FileBinProvider`/`FileWadProvider`/`wad_path_hash` come from `hematite_file::…` (already deps). `apply_fixes`, `wad_pipeline`, `ConverterRegistry` from `hematite_core::…`. Any small local helper still in `process.rs` that `fix_folder` needs must be moved into `fix_folder.rs` (or a shared module) — grep and resolve.
- [ ] **Step 4: detect-only.** The old code branched on `check`/`dry_run`. Add `opts.detect_only`: when true, run detection and record each fired fix as an `AppliedFix` with `changes_count` = detection count, but SKIP every write/remove/rebuild step (no WAD rebuilt, no files written). The simplest correct implementation: treat `detect_only` like the existing `dry_run` path for the BIN/WAD apply loops (which already record `AppliedFix` without mutating), AND additionally short-circuit before the WAD-rebuild/output-write stage. Keep `dry_run` behaviour exactly as-is; `detect_only` is the stronger "never touch disk, but still report counts" mode. If `dry_run` already never writes, `detect_only` can be `opts.dry_run || opts.detect_only` at the write guard — but ensure counts are still populated.
- [ ] **Step 5:** In `process.rs`, DELETE the `process_wad_folder` fn. Wherever it was called (the folder branches in `process_input`), call `hematite_orchestrate::fix_folder(path, ctx.config, ctx.selected_fixes, ctx.champions, hash_provider, &opts, &sink)` where `opts` is a `FixOptions` built from `ctx` fields and `sink` is a `UiReporter`-backed `ProgressSink` adapter (Step 6).
- [ ] **Step 6:** Add a `UiReporter → ProgressSink` adapter in the CLI. In `crates/hematite-cli/src/ui.rs` (or a new `progress_adapter.rs`), define `struct UiSink<'a>(&'a UiReporter);` and `impl hematite_orchestrate::ProgressSink for UiSink<'_>` forwarding each method to the wrapped `UiReporter`. Build the `GameFileAccess` value in `process.rs`/`main.rs` from `ctx.live` per Task 3's decision (if the boundary is `GameProvider`, `ctx.live` already is one — pass `ctx.live.map(|l| l as &dyn _)`).
- [ ] **Step 7:** `cargo build -p hematite-orchestrate` then `cargo build -p hematite-cli`. Resolve until green.
- [ ] **Step 8:** Commit `feat(orchestrate): add fix_folder; CLI folder path delegates to it`.

---

### Task 7: Parity + detect-only tests

**Files:**
- Create/modify: tests in `crates/hematite-orchestrate/src/fix_folder.rs` (`#[cfg(test)]`) or `crates/hematite-orchestrate/tests/`
- Test fixture dir: `crates/hematite-orchestrate/tests/fixtures/` (gitignored real assets; commit only a `.gitkeep` and/or a tiny synthetic WAD folder if one can be built without game assets)

**Consumes:** Task 6.

- [ ] **Step 1:** Check how the existing CLI/core tests obtain a WAD-folder fixture without shipping game assets (grep `tests/` in `hematite-cli`/`hematite-core`/`hematite-file` for fixture construction; there may be a synthetic-WAD helper). Reuse that. If no asset-free fixture is feasible, write the detect-only test against a **synthetic minimal folder** (a hand-built `.wad.client`-shaped dir with one tiny BIN that triggers exactly one known fix), and mark the full golden-path parity test `#[ignore]` with a comment pointing at the gitignored real-fixtures dir.
- [ ] **Step 2: detect-only writes nothing test.** Build/copy a fixture folder with a known issue into a `tempfile::TempDir`. Snapshot the folder's file bytes (path → sha/len map). Run `fix_folder` with `FixOptions { detect_only: true, .. }`. Assert: the returned `ProcessResult.applied_fixes` is non-empty (the known fix fired with a count > 0) AND the folder's file-byte snapshot is UNCHANGED afterwards (no file added/removed/modified). Add `tempfile` as a dev-dep if needed.
- [ ] **Step 3:** Run `cargo test -p hematite-orchestrate` → expect PASS (the detect-only test; parity test may be `#[ignore]`).
- [ ] **Step 4:** Commit `test(orchestrate): detect-only writes nothing + parity scaffold`.

---

### Task 8: Whole-workspace green + final review

**Files:** none (verification).

- [ ] **Step 1:** `cargo fmt --all`.
- [ ] **Step 2:** `cargo clippy --workspace -- -D warnings -A clippy::needless_return` → resolve any warnings.
- [ ] **Step 3:** `cargo test --workspace` → ALL green, including the pre-existing `hematite-cli` tests (esp. `all_fix_ids_exist_in_repo_config`) and `hematite-core` tests. If any CLI test regressed, the lift changed behaviour — fix it, do not delete the test.
- [ ] **Step 4:** Sanity-run the CLI on a real folder if a fixture is available: `cargo run -p hematite-cli -- --check --json <some wad folder>` and confirm the JSON output is unchanged in shape from before the refactor (the folder path now goes through `fix_folder`).
- [ ] **Step 5:** Commit `chore(orchestrate): fmt + clippy clean`.
- [ ] **Step 6:** STOP. Do NOT push. Report back: the final `git log --oneline` of new commits, the `cargo test --workspace` summary line, and any place where a behaviour decision had to be made (esp. Task 3's boundary choice and Task 6 Step 4's detect-only guard). The parent will review before pushing Hematite `main`.

---

## Self-review

- Spec §"What moves" → Tasks 4 (modules) + 6 (fix_folder). §"Neutralising CLI deps" → Task 2 (ProgressSink) + Task 3 (GameFileAccess) + Task 5 (FixOptions). §"Report shape" → Task 6 Step 4 + reuse of `ProcessResult`. §detect-only → Task 6 Step 4 + Task 7. §`list_fixes` → Task 5. §rev-lock → Global Constraints. §testing → Tasks 5/7/8.
- Type consistency: `fix_folder` signature in Task 6 matches the `FixOptions`/`ProgressSink`/`FixInfo` defined in Tasks 2/3/5. `ProcessResult` is the existing type (not redefined).
- No placeholders — every step names the file, the grep, or the code. The two genuinely code-dependent decisions (live-provider boundary; detect-only guard) are called out as explicit sub-steps with a decision procedure, not left vague.
