# Hematite rework: one engine for detection, reporting and repair

---

## STATUS (phases 1-3 landed)

`--crashcheck` works end to end, human-readable and `--json`. Workspace compiles, 178
tests pass.

**Built**
- `hematite-types::diagnostic` — `Severity`, `ReasonDef`, `ReasonCatalog`, `Diagnostic`,
  `CheckReport`, `SkipReason`
- Reason catalog as **data** in `fix_config.toml` under `[reasons.*]`, 30 entries. Not a
  Rust enum: adding a crash class or changing a severity is a config edit.
- `reason` on `FixRule`, `WadFixRule`, and `ClassFieldTarget` (all optional, so existing
  configs load unchanged)
- `retype_file::detect_hits` / `target_key` — per-property migration detection
- `check::diagnose_fired_rule`, wired into `pipeline` and `wad_pipeline`, so every
  existing path produces diagnostics rather than a parallel check-only code path
- `hematite-cli --crashcheck`

**Per-target severity works.** One migration rule reports `crash` on
`AnimationResourceData.mAnimationFilePath` and `warning` on icon fields, because each
target names its own reason and each reason carries its own severity.

### Fixture results

All 291 workspace tests pass.

| Fixture | Reported | |
|---|---|---|
| Alucard Jhin | `crash unmigrated_animation_path` x76 + 4 warning classes | partial |
| anakin viego | `crash dead_gear_link` x43, `crash unmigrated_animation_path` x2, + 4 warnings | exact |
| Spirit Blossom Rift | `crash unmigrated_animation_path` x5 + 5 warning classes | partial |
| Emerald Gains | `crash replaced_bin_crash` (FloatTextStyle dropped `889a59de`, `f67ba35a`) | **exact** |
| Klee Gragas | 1 crash + 2 warnings | partial |
| Motorbike Gragas | 3 warnings | partial |

Every finding is reported, not just the worst: `anakin viego` surfaces two distinct crash
classes and four warning classes at once. `worst()` only picks the headline sentence and
never filters the list.

### replaced_bin: what it took

The Emerald Gains false negative needed three separate fixes, only one of which was the
check itself:

1. **`TransformAction::ReportOnly`.** Every rule previously had to claim a fix, so an
   unfixable crash class could not be expressed at all without giving it a sham
   transform. The pipeline now treats a detected `ReportOnly` rule as reported, neither
   an applied fix nor a failure.
2. **`GameIndex::add_shared_wads`.** The index only ever held champion WADs. Interface
   BINs live in `UI.wad.client` / `Global.wad.client`, so the vanilla copy was
   unreachable and every replaced interface BIN looked like original mod content.
3. **Priming outside the seed gate.** `prime_champion_wads` ran only when skin seeds were
   discovered. An interface mod has no seeds, so the index stayed empty for exactly the
   mods that needed it.

The whole rules table (14 class targets, added/dropped, lethal-key subsets) is config
data in `fix_config.toml`, not Rust.

### dead_shader_link: the crash Hematite could not see

A `StaticMaterialDef` links its shader by entry key. When a patch renames or removes a
shader, the link resolves to null and the client dies with no error code recorded, so it
cannot be diagnosed from the log afterwards.

Hematite could not detect this at all, for two independent reasons:

1. **The shader list was never installed.** `ShaderValidator` reads
   `hashes.shaders.txt`, and the downloader only ever fetches `hashes.lmdb`. The file is
   absent on a normal install, so `shader_fallback` (marked `critical`) could never run.
2. **Its detection would not have worked anyway.** `detect_invalid_shader` returns true
   whenever a `CustomShaderDef` carries *any* non-zero link. It never compares against a
   shader set: validity is decided later, inside the transform. As a crash check it
   fires on every mod that has a shader at all.

The replacement reads the valid set from the installed game at runtime
(`GameProvider::shader_defs`, built from `DATA/FINAL/Shaders/Shaders.wad.client`), so it
needs no hash list and cannot go stale: the set changes every patch and is re-read from
whatever is installed. Total run cost on the Yuumi fixture, including building the set,
is 3.3s.

Two scoping rules carry the whole result and both are load-bearing:
- Only links under the `shader` property of a `StaticMaterialDef` count. Collecting every
  ObjectLink would flag every interface and skin mod, since BINs are full of links that
  legitimately point outside the mod.
- Entry keys the mod defines itself are not dead. A mod shipping its own shader links to
  itself, and that resolves.

Verified discriminating: fires on the Yuumi fixture only, clean on the other six.

`shader_fallback` (the repair) still reports `no_shader_list`, which is correct and now
visible: the check works without the file, the automatic repair does not.

### Dead asset references, and the false-positive that nearly shipped

`dead_asset_reference` reports a path present in neither the mod nor the game, and backs
both `dead_animation` (`.anm`) and `dead_skin_mesh` (`.skn`/`.skl`).

The first version compared literal strings and reported **64 dead animations** on one
fixture. Measured against all 456 game WADs, **62 of those 64 were wrong**. A repathed
mod rewrites `ASSETS/Characters/...` to `ASSETS/<prefix>/Characters/...` and ships only
what it replaces, so every untouched reference carries a prefix while the file sits at
the stock path. Re-anchoring on the `characters/` segment resolves them, leaving the 2
that are genuinely dead.

What is deliberately NOT rescued: an inner-suffix rename
(`Recall.SKINS_Jhin_Skin55.anm` where the game now ships `Recall.anm`). The engine
resolves by literal path, so that clip really is missing. Suffix-stripping belongs to
repair, which needs a source to pull FROM; it is not a resolution rule.

Diagnostics are also deduped by (reason, field, detail) before rendering. Rules run per
BIN and this fixture ships 37 clones of one skin, so the same two missing clips were
reported 37 times.

### Reachability

`hematite-core::reachability` is built and tested but **not yet wired into the pipeline**.

Roots are the skin BINs the WAD ships; edges are each root's declared dependency list,
followed exactly once. Deliberately not a transitive closure: widening it re-admits the
orphan BINs the model exists to exclude. A champion with no slot 0 contributes nothing.
`None` means "could not establish what loads" and callers must scan everything, so
reachability can only ever SHRINK the considered set, never grow it and never itself
cause a finding.

Animation BINs need an exemption when it is wired: they are never roots, only
dependencies, and a clip in an animation BIN that no shipped skin links is *latent*
(selecting the original skin would crash) rather than an active crash. That distinction
is a warning-vs-crash split, so dropping those BINs at the gate loses it entirely.

### Reachability, wired

`BinScope` on `FixContext` carries the BIN's chunk hash, the load graph, and the mod's
animation BINs. Rules opt in with `loaded_bins_only`, which gates **reporting only**: the
transform still runs, so this changes what a user is told rather than what gets repaired.

Effect on the map fixture: missing animations 5 to 3, map meshes 22 to 20, as orphan BINs
fall out.

The animation carve-out matters. Animation BINs are never roots, so a plain gate drops
them, and with them the crash-versus-latent distinction: a clip in an animation BIN that
no shipped skin links is only dead for someone selecting the *original* skin with the mod
active. `latent_reason` on the rule reports those separately instead of overstating them.

### Loose textures

Ports the ratio check. Not a `FixRule`, because it cannot be answered one BIN at a time:
it needs the archive's file list, every BIN's references and the game together. Parameters
still live in config.

Two things had to be right:

- **It runs before the WAD pipeline.** Measured afterwards, the archive appears to ship
  only `.tex` (the conversions already happened in the working set) and the count is
  zero, which reads as a clean mod. The question is what the mod *shipped*.
- **It only applies to mods defining no skin.** A mod shipping a real skin BIN is wiring
  its art up and its loose files are intermediates. Judging those by this ratio condemns
  working mods.

Klee Gragas reports `73/73 (100%)`, matching Celestial exactly.

### Performance

Benchmarked across all seven fixtures. Two fixes landed, one was reverted:

- **`extract_strings` deep-cloned the entire BIN tree** on every call, purely to satisfy
  a `&mut` signature, then copied every string out of the clone and discarded it. Five to
  seven calls per BIN. Measured at roughly ten times the traversal it existed to perform.
  Now borrows.
- **The LMDB adapter built a 536,846-entry reverse name map at startup**, lowercasing and
  allocating every name, for a value that is a pure function of the name. The hash is
  computed on demand; the forward map is consulted only to tell known from unknown.
- **A closure cache was tried and reverted.** Building the game closure is the single
  largest cost (1.8s on one fixture), but a process-wide cache keyed on seeds plus linked
  list is unsound: it shares one game install's answer with another provider, which the
  test suite caught immediately. It also barely helped, since the key includes the
  per-BIN seed. The correct fix is a cache owned by the `GameProvider`; the reasoning is
  recorded at the function.

Remaining known costs, unfixed: `extract_all_files` decompresses every chunk upfront
including multi-MB textures; `build_shader_defs` fully decompresses all 147 shader-WAD
chunks then discards non-PROP ones; the format-conversion loop has no dry-run guard, so
`--crashcheck` performs conversions and throws them away.

### Batch checking

`--batch <DIR>` checks every mod in a directory, sharing the loaded data and running them
concurrently (`--jobs N`, default one per core).

The sharing matters more than the parallelism. The hash dictionary, game index and shader
set are identical for every mod and cost well over a second to build, so a process per mod
pays that over and over. Batched, it is paid once:

| | per mod | 7 mods |
|---|---|---|
| one process each | ~2800 ms | ~19.7 s |
| batched | **522 ms** | **3.7 s** |

One unreadable archive is reported and the batch continues; aborting on the first bad file
defeats the purpose. `--json` emits an array with one entry per mod.

Two things this exposed:

- **`Mode::Silent` was not silent.** Every message checked for a progress bar and fell
  through to `eprintln!` when there was none, so seven concurrent workers interleaved
  per-mod chatter over the summary. Output now funnels through one `emit`, which is the
  only place the flag has to be honoured.
- **A batch defaults to quiet.** An explicit `-v` still turns per-mod logging back on.

### Performance, measured

`--timings` reports real spans rather than gaps between log lines. That distinction
mattered immediately: an unscoped span made the WAD pipeline look like 2.4 s when it was
actually 45 ms, and the real cost was elsewhere.

The dominant cost was parsing the same BINs repeatedly. Four phases each parsed every BIN
independently, then the fix pipeline parsed them all again. Parsing once and sharing took
loose textures 657 to 53 ms, referenced assets 1023 to 302 ms, and removed the pipeline's
second parse entirely.

| Fixture | Before | After |
|---|---|---|
| anakin viego (30 MB) | 8604 ms | 5622 ms |
| Spirit Blossom (44 MB) | 4943 ms | 3536 ms |
| Alucard Jhin (12 MB) | 2870 ms | 2515 ms |

Also landed: `extract_strings` no longer deep-clones the tree; `game_bin` is memoised on
the provider behind an `Arc`; the LMDB reverse name map is gone (the hash is a pure
function of the name); and a run that writes nothing no longer performs texture
conversions it discards.

**The remaining floor is startup**: roughly a second loading 2.8 M dictionary entries into
maps, paid once per process. Batch mode amortises it; a single-mod run still pays it. The
structural fix is lazy point lookups against the open LMDB instead of a full preload,
which means `resolve_game_path` returning owned strings rather than a borrow into a
preloaded map. Not attempted here: it is a provider API change, not a tweak.

### Remaining gaps

1. **Missing animations is not detected as such.** Alucard Jhin should crash for *two*
   reasons: unmigrated animation paths (caught) and animation clips absent from both mod
   and game (not caught). `anm_validation` is the largest Celestial validator at ~1,069
   lines and is the last big port.
2. **Missing `.skn` / `.skl`.** The likely real cause of the Spirit Blossom crash.
   Partially expressible with `string_extension_not_in_wad`, but that consults only the
   mod WAD, and a mesh the *game* ships is not missing.
3. **Ratio checks.** `dds_to_tex` detects dead `.dds` *references*; the Gragas defect is
   unreferenced loose `.dds` *files* above a threshold. Different defects, and the ratio
   evaluator does not exist yet, so the unplayable verdict cannot be reached honestly.

### Dropped from the port: mapgeo

Celestial treats an outdated `.mapgeo` format version as a crash. A dev reports that an
outdated mapgeo loads fine, and the evidence here agrees: Spirit Blossom Rift already
carries a detected crash cause (unmigrated animation paths) plus a probable second one
(missing mesh), so the mapgeo attribution is not needed to explain the crash. Porting it
would import a false positive **and** attach a remedy the launcher cannot deliver. Left
out pending evidence that it crashes on its own.

### Operational note

The CLI loads `fix_config.toml` from a **GitHub-fetched cache**
(`%APPDATA%/Hematite/cache`), with the repo copy only as an embedded fallback. The local
config was copied into that cache for these runs; the original is saved beside it as
`fix_config.toml.upstream-backup`. **The reasons must land in the upstream Hematite repo
or none of this fires for real users**, and the cache will overwrite the test copy when
it expires.

---

**Goal.** Make Hematite the single engine behind Celestial's mod check, Quick Repair and
Deep Repair, and have it cure the `string` -> `file` migration on every import.

Supersedes `PORT-PLAN-crashcheck.md`.

---

## 0. The shape of the problem

Hematite today is **half an engine**. It can rewrite a mod, but it cannot say anything
about one.

| | Hematite | Celestial |
|---|---|---|
| Detection rules as data | yes, 11 `DetectionRule` variants | no, hardcoded Rust |
| Transforms as data | yes, `TransformAction` | no |
| Severity taxonomy | `severity: String` per rule, unused for reporting | **yes, 25 reasons in 3 tiers** |
| Emits a report | **no** | yes (`ImportValidation`) |
| Works on loose folders | yes, by construction | **no, fails open silently** |
| Provider abstraction | **yes, 3 traits, no I/O** | no, concrete paths |

Neither side is complete. The rework is not "port Celestial into Hematite", it is
**join the two halves**: Hematite's architecture with Celestial's semantics.

Concretely:
- Hematite contributes the trait boundary, the rule format, the transforms.
- Celestial contributes the 25-reason taxonomy, the 15 checks, the ordering knowledge.

---

## 1. Architecture

### 1.1 Two modes over one rule set

The central idea: **a rule declares both what it detects and what that means.** Running
detection alone gives you the mod check. Running detection plus transform gives you
repair. Same rules, same code path, one flag.

```
                    ┌─ detect ─┐
  providers ───────▶│  rules   │──▶ Vec<Diagnostic> ──▶ CheckReport   (mod check)
                    └────┬─────┘
                         │ (if mode == Repair)
                         ▼
                     transform ────────────────────▶ mutated BinTree  (repair)
```

Today `detect_issue()` returns a bool used only to gate a transform, and
`apply_transform()` returns a `u32` change count. Both become byproducts of a richer
detection result.

### 1.2 Crate layout

```
hematite-types/
  diagnostic.rs     NEW   Severity, Reason, Diagnostic, CheckReport, SkipReason
  config.rs         EDIT  per-target severity, reason mapping on rules
hematite-core/
  check/            NEW   the ported checks that cannot be expressed as rules
  detect/           EDIT  return diagnostics, not bools
  pipeline.rs       EDIT  Mode::Detect | Mode::Repair
hematite-ltk/               unchanged (adapter, not used by Celestial)
hematite-cli/
  checkcrash        NEW   --json, exit code by worst severity
```

Celestial depends on **`hematite-core` + `hematite-types` only**. Not `hematite-ltk`,
not the CLI. It implements `BinProvider` / `HashProvider` / `WadProvider` over its own
stack.

### 1.3 Why this fixes loose folders for free

`WadProvider` is only `has_path(&str)` and `has_hash(u64)`. A directory answers those as
easily as an archive. Every check written against the trait becomes form-agnostic.

Today `decompress_mod_wads` (`celestial/src/mods/mod_scan.rs:62`) returns `None` on an
unpacked folder, which fails open and skips validation for the entire mod. One provider
impl covering both forms retires that bug for all 15 checks at once, instead of patching
each call site.

---

## 2. Schema changes

### 2.1 Severity must be per-target, not per-rule

This is forced by the requirement that one migration rule yields **crash** for an
animation path and **warning** for a HUD element.

Today:

```rust
pub struct FixRule { pub severity: String, /* one for the whole rule */ }
pub struct ClassFieldTarget { pub class: String, pub field: String }
```

All 7 targets of `file_ref_migration` share one severity. Change to:

```rust
pub struct ClassFieldTarget {
    pub class: String,
    pub field: String,
    /// Overrides the rule severity. Absent = inherit.
    #[serde(default)]
    pub severity: Option<Severity>,
    /// Overrides the rule reason. Absent = inherit.
    #[serde(default)]
    pub reason: Option<Reason>,
}
```

Then:

```toml
targets = [
  { class = "AnimationResourceData", field = "mAnimationFilePath",
    severity = "crash",   reason = "unmigrated_animation_path" },
  { class = "HudMenuData",           field = "iconPath",
    severity = "warning", reason = "unmigrated_hud_asset" },
]
```

Apply the same optional override to `EntryValidationTarget`, which has the same
one-rule-many-targets shape.

### 2.2 Rules declare a reason

```rust
pub struct FixRule {
    pub severity: Severity,       // was String
    pub reason: Reason,           // NEW: what to report when this fires
    pub detect: DetectionRule,
    pub apply: TransformAction,
    pub phase: FixPhase,
    ...
}
```

`Severity` becomes an enum (`Crash | Unplayable | Warning | Info`) rather than a free
string. The existing `critical/high/medium/low` values map onto it during the migration.

### 2.3 The diagnostic type

```rust
pub struct Diagnostic {
    pub reason: Reason,
    pub severity: Severity,
    pub rule_id: String,        // which rule fired: provenance
    pub entry: Option<String>,  // entry path / key
    pub field: Option<String>,
    pub detail: Option<String>, // names the specific asset
    pub fixable: bool,          // is there a transform for this
}

pub struct CheckReport {
    pub diagnostics: Vec<Diagnostic>,
    pub ran: Vec<String>,
    pub skipped: Vec<(String, SkipReason)>,  // NOT silently absent
    pub worst: Severity,
}
```

`skipped` is not optional polish. Today a missing hash DB silently downgrades several
checks and the result is indistinguishable from a pass. **A check that could not run must
say so**, or the report lies.

### 2.4 Reason taxonomy

Move Celestial's three enums into `hematite-types::diagnostic` largely as-is. They are
the best-documented part of either codebase: each variant carries a comment explaining
the failure and the remedy. Those comments become user-facing explain text.

- `BrokenReason` (14) -> `Severity::Crash`
- `UnplayableReason` (4) -> `Severity::Unplayable`
- `WarningReason` (10) -> `Severity::Warning`

Plus the new migration reasons from section 3.

---

## 3. The `string` -> `file` migration program (top priority)

### 3.1 Why it is urgent

Riot is converting bin asset-reference properties from `string =` to `file =` (xxh64).
A mod that still stores the old form is not merely stale: the engine cannot read it.
For an animation path that is a **hard crash**. For a HUD or loading-screen asset it is a
**missing element**, degraded but playable.

Celestial has **no detection for this whatsoever**. No reason variant, no check. After
the patch it will pass mods that crash.

### 3.2 What exists

| Asset | Where | State |
|---|---|---|
| Derived field table | `pyritocrash/migrations.json` | 46 migrated hashes, 12 conflicts |
| Detection rule | `class_field_is_string` | exists, **7 targets** |
| Transform | `retype_string_to_file` | exists |
| Report | nowhere | **missing** |

### 3.3 The keying insight

The derived table keys on bare fnv1a32 field hashes. That forced us to **exclude**
context-dependent properties entirely: `texture` (`3c6468f4`) and `TextureName`
(`b311d4ef`) are STRING in VfxSystem/particle classes but FILE in others, so a flat table
cannot represent them without corrupting VFX.

Hematite keys on **(class, field)**, which represents exactly that distinction. So the
config format is strictly more expressive than the table we derived, and the 12
"conflicts" become expressible rather than excluded.

**This is the argument for keeping the migration list in config**: when Riot widens the
migration, coverage is a config edit, not a rebuild and release.

### 3.4 Work

1. **Resolve the 46 hashes to class/field pairs.** `migrations.json` holds bare fnv1a32.
   The config needs names, and needs to know which classes carry each field. Requires a
   dictionary pass plus a class-scoping pass over a bin corpus.
2. **Widen `file_ref_migration` targets** from 7 to full coverage, each tagged with its
   own severity and reason per 2.1.
3. **Re-derive after the patch ships.** The 12 conflicts may resolve to clean migrations
   once Live catches up to PBE.
4. **Ship `hematite derive-migrations`** (section 6.3) so the table regenerates from a
   Live/PBE WAD diff instead of being hand-maintained.

### 3.5 Cure on import: the ordering constraint

Curing every imported mod is the top-priority behaviour, and it has a hard constraint
already encoded in the codebase:

> `FixPhase::PostRepath` rules run in a second pass after the repath stage, because they
> destroy the string paths repath needs to see (e.g. retyping asset path strings into
> xxh64 `file` hashes).

So `retype_string_to_file` **must** run `PostRepath`. Running it early silently breaks
repath, because repath can no longer see the strings it needs to rewrite. Any
cure-on-import wiring must respect the phase split rather than calling the transform
directly.

Import flow becomes:

```
import -> detect (report) -> repath -> PostRepath fixes (cure) -> re-detect (verify) -> store
```

The re-detect is what lets the UI say "imported and repaired" with evidence rather than
assertion.

### 3.6 Safety rules for mutation on import

Curing mutates the user's mod, so:

- **Back up before first mutation.** Non-negotiable.
- **Idempotent.** Running cure twice must produce a byte-identical result the second
  time. This is a test, not an aspiration.
- **Never touch a correct mod.** A mod with no diagnostics must come out byte-identical.
- **Report what changed**, per field, so a bad cure is diagnosable after the fact.

---

## 4. Porting the checks

### 4.1 Split by what the rule format can express

Every `DetectionRule` variant today is a **per-bin boolean predicate**. That covers a
large share of the checks and none of the rest.

**Group A. Already expressible as rules (no new Rust).**

| Celestial check | Existing variant |
|---|---|
| `DeadGearLink` | `dead_entry_link` (its doc comment names this case) |
| `OutdatedMaterial` | `invalid_shader_reference` |
| `BuggedHpBar`, `StaleCharacterStats`, `NoAbilities` | `missing_or_wrong_field` |
| deadlinks, `SkinMeshCrash` | `string_extension_not_in_wad` (+ recursive) |
| `MissingVoiceover` (CAC) | `dead_entry_link` with `require_pullable` |
| migration crashes | `class_field_is_string` |

**Group B. Needs a new evaluator variant (Rust once, then configurable forever).**

| Need | Why data cannot express it | Proposed variant |
|---|---|---|
| `NoAbilityVfx` >=80%, `VisuallyBuggedVfx` 30-80%, `MissingTextures`, `LooseTexturesUnplayable`, `MissingParticles` | aggregate ratio across the mod, not a per-bin bool | `missing_ratio_over` { scope, extensions, warn_at, fail_at } |
| `MapgeoOutdated` | binary header version compare | `asset_format_version_below` |
| `TexDimCrash`, `EmotesCrash`, dds playability | block-alignment math on the texture header | `texture_header_invalid` |
| `ExcessiveWadFanout` | mod-level count across game WADs | `wad_fanout_over` { threshold } |
| `MissingAnimations` vs `LatentDeadAnimation`; `DeadUiLink` vs `ScreenGatedDeadLink`; `ReplacedBinCrash` vs `ReplacedBinWarning` | same detection, severity depends on reachability | see 4.2 |

`bnk_version_not_in` is the precedent: the parser is Rust, the thresholds are data.

### 4.2 Reachability-gated severity

Three reason pairs share one detection and differ only in whether the affected bin is
reachable from a loaded skin. This is not expressible as a flat severity, so the rule
schema needs:

```toml
severity = { reachable = "crash", gated = "warning" }
```

`Reachability` currently lives inside Celestial's `mod_scan` and several checks depend on
it. It has to move into the module with them, most likely as a fourth provider or as part
of the check context.

### 4.3 Port order

Cheapest and safest first. Of the 15 Celestial validators, **8 already perform zero file
I/O** (`cac`, `cubemap`, `dead_vfx`, `particle`, `replaced_bin`, `skin_mesh`,
`stale_character`, `talon_edgemesh`, about 1,980 lines). They are already pure functions
over decompressed data, so porting is mostly swapping the concrete parameter for the
traits.

The I/O-heavy ones (`anm` at ~1,069 lines, `luabin` 738, `mapgeo`, `shader`, `gear`,
`tex`, `deadlink`) come after, each needing its reads rerouted through a provider.

### 4.4 Ordering constraint to preserve

`validate_imported_mod_ex` runs mapgeo **first**, deliberately: it is a header-only read,
it is terminal (an outdated map format is not repairable in-launcher), and running it
before the size gate and before decompression avoids decompressing a 500 MB WAD that can
never load. Keep that. It is a real performance decision.

---

## 5. Providers

Celestial implements three traits. The only one with a design decision in it:

```rust
impl WadProvider for ModWads {
    fn has_hash(&self, h: u64) -> bool { self.chunk_hashes.contains(&h) }
    fn has_path(&self, p: &str) -> bool { self.has_hash(wad_path_hash(p)) }
}
```

`chunk_hashes` is built either by mounting a packed `.wad.client` **or by walking a
`<Name>.wad.client/` directory** and hashing each relative path, using the established
rule:

```rust
let hash = hex_chunk_hash(&rel).unwrap_or_else(|| wad_path_hash(&rel));
```

(root-level 16-hex filenames are literal hashes, everything else is hashed by path).

Celestial already has `DecompressedModWad { chunk_hashes: HashSet<u64>, bins, .. }`,
which is this abstraction in all but name. The port formalises it.

### 5.1 Raw folders are the canonical storage form, not a preference

**Packing a mod to `.wad.client` destroys information that cannot be recovered.**

A WAD chunk is keyed by `path_hash` (xxh64 of the lowercased path) and stores **no path
string**. Confirmed in `mods/wad_utils.rs:157`:

```rust
wad.chunks().iter().map(|c| c.path_hash).collect()
```

For a stock Riot path that is survivable, because a hash dictionary can reverse it. For a
**custom path** invented by a modder it is not: no dictionary will ever contain it. Once
that mod is packed, the path exists nowhere. The mod can never again be repathed,
repaired, migrated, or diagnosed by path. It is permanently opaque.

That is a data-loss bug wearing the costume of a storage format. So:

> **Every fantome (and every other packed import) is converted to a raw
> `<Name>.wad.client/` folder on import, and stored that way. Packing is an export-time
> operation only.**

The filesystem path *is* the record. Nothing else preserves it.

This is the same problem the bin-layer work already solved, one level up. The two form a
single preservation story:

| Layer | What is lost when packed/converted | Preserved by |
|---|---|---|
| WAD | asset path -> xxh64, unrecoverable if custom | **raw folder storage** |
| BIN | custom path -> hash across bin/py conversion | `CELMAP` trailer + `files.txt` |

Both must hold, or a custom-pathed mod degrades on the first round trip.

### 5.2 The sequencing hazard this creates

Today raw folders are rare (only the keep-loose setting produces them), so
`decompress_mod_wads` returning `None` on a directory silently skips validation for a
handful of mods.

**Make raw the default and that fail-open disables validation for the entire library.**

Scope of the assumption: `Wad::mount` appears at **73 sites across 30 files**. Not all
need changing, since many mount *game* WADs, which stay packed forever. But every site
that reads a *mod* WAD does: `mods/*_validation.rs`, `mod_scan.rs`, `skinlite` (4),
`repair/*`, `commands/creator*`, `cascade.rs`.

Two consequences for the plan:

1. **Provider support must land before or with raw-by-default, never after.** Shipping
   raw-by-default onto today's fail-open would mean nothing in the library is checked.
2. **Do not fix 73 call sites.** Introduce one loose-aware reader behind `WadProvider`
   and route mod-WAD access through it. That is the entire reason the trait boundary is
   worth having.

---

## 6. Making it genuinely good

Beyond parity, the things that decide whether this survives contact with live patches.

### 6.1 Every diagnostic explains itself

The reason enums already carry doc comments describing cause and remedy. Surface them:
`Reason::explain()` and `Reason::remedy()`. The UI stops needing its own copy of the
knowledge, and Quartz/Flint get it free.

### 6.2 Dry-run diffs

`checkcrash --explain` shows what repair *would* change, per field, before touching
anything. Deep Repair mutating a mod with no preview is the scariest operation in the
launcher.

### 6.3 Self-updating migration table

`hematite derive-migrations --live <path> --pbe <path>` regenerates the (class, field)
migration set from a WAD diff, using the strict rule: a property migrates only if the new
build stores it as FILE/LINK/HASH and **never** as STRING in that class.

Without this, the table rots every patch and someone has to notice. With it, the response
to a patch is running one command.

Encode the two known traps in the deriver, because both silently produce an empty or
wrong table:
- **Containers**: `option[string]` / `list[string]` migrate by changing the container's
  `value_type`, not the field's own type. The differ must record `value_type` for
  LIST/OPTION/MAP or these are invisible.
- **Scalar OPTION shape**: an OPTION's single value can be a bare scalar, not a list.

### 6.4 Corpus regression

Run the engine across a mod library, snapshot the reports, diff on every change. This is
the only way to know a rule widening did not start flagging half the library. Ship
`hematite corpus <dir> --snapshot`.

### 6.5 Stable machine-readable output

Version the JSON report schema from day one, since Celestial, Quartz and Flint will all
consume it and they update independently.

### 6.6 Invariants worth asserting in CI

- Repair is **idempotent**: fix twice, second run is a no-op.
- Repair is **conservative**: a clean mod comes out byte-identical.
- Detection is **form-agnostic**: packed and loose forms of the same mod produce
  identical diagnostics.

The third is the one that proves the loose-folder gap actually closed rather than moved.

---

## 7. Phasing

| Phase | Content | Unblocks |
|---|---|---|
| **1. Types** | `diagnostic.rs`, `Severity`/`Reason`/`Diagnostic`/`CheckReport`; `severity: String` -> enum | everything |
| **2. Schema** | per-target severity + reason; rule-level `reason`; reachability-gated severity | 3, 4 |
| **3. Detect mode** | `detect_issue` returns diagnostics; `Mode::Detect \| Mode::Repair`; `checkcrash` verb | first shippable value |
| **4. Migration program** | resolve 46 hashes, widen targets, `derive-migrations`, cure-on-import wiring | the patch response |
| **5. Providers** | Celestial implements 3 traits, packed + loose | **gates phase 6** |
| **6. Raw-by-default storage** | fantome -> raw folder on import; packing becomes export-only | preservation |
| **7. Group A checks** | express as rules, delete Celestial copies | |
| **8. Group B evaluators** | 5 new variants, then express as rules | |
| **9. Heavy checks** | `anm`, `luabin` | |
| **10. Repair switchover** | Quick/Deep Repair call Hematite | |

Phases 1-3 are the spine. Phase 4 is the priority payload and can run in parallel with 5
once the schema lands.

**Hard ordering rule: 5 before 6.** Raw-by-default on top of today's fail-open would
disable validation for the whole library (see 5.2). They can ship together, never in the
other order.

Phase 6 also needs the non-validation mod-WAD readers to be loose-aware before it flips:
SkinLite (4 mount sites), Deep Repair, creator tools, cascade. Those are not gated by the
provider work but they are gated by the same flip.

---

## 8. Test fixtures

Four mods, one per crash class:

| Folder | Mod | Expected |
|---|---|---|
| `anm crash` | Alucard Jhin.fantome | `MissingAnimations` |
| `gearlink crash` | anakin viego.fantome | `DeadGearLink` |
| `map crash` | Spirit Blossom Rift by Moga | `MapgeoOutdated` / `MissingMapTexture` |
| `ui crash` | Emerald Gains V1.0.5 by Vita | `DeadUiLink` / `ReplacedBinCrash` |

Assert each **twice**, packed and extracted loose, demanding identical diagnostics.

The migration work needs its own fixture: a mod with an unmigrated animation path
(expect crash) and one with an unmigrated HUD asset (expect warning), proving per-target
severity resolves correctly.

---

## 9. Open decisions

1. **Dependency direction.** Path dep during the port, git tag for release. Confirm.
2. **Does Celestial keep `ImportValidation`?** Suggest yes at the Tauri boundary
   initially, mapping from `CheckReport`, so the frontend does not change in the same
   step as the engine.
3. **Where does `Reachability` live?** Fourth provider, or part of the check context.
4. **Cure-on-import default.** On for every import, or opt-in for the first release with
   a report-only default? Mutating by default is the stated goal, but it argues strongly
   for 3.6's backup and idempotence guarantees landing at the same time.
