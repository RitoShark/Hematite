# Repath checks and LTK comparison

Reviewed 2026-09-08 against LeagueToolkit/ltk-manager commit
[`018fe6ec906b1a28c98817996f52a4b36fafced9`](https://github.com/LeagueToolkit/ltk-manager/tree/018fe6ec906b1a28c98817996f52a4b36fafced9).

## Repath reporting

`hematite-core::repath_check::check_repath` adapts the sibling `repath-check`
classifier to Hematite's BIN model. Folder and packed-WAD processing run it
before repairs. Fantome and modpkg processing inherit those checks per WAD.
`ProcessResult.repath_reports` retains every WAD's report during aggregation;
CLI `--check --json` and normal `--json` expose the same list. These reports
describe the **input**, not a post-repair health verdict.

Each report includes `source`, `status`, `percent`, `canonical`, `prefixed`,
`total`, `bins_scanned`, `bins_failed`, `unresolved_hashes`, `needs_repath`, and
`skip_reason`. Status thresholds match repath-check: 99% repathed, 60% partially
repathed (`halfly_repathed`), otherwise not repathed. No eligible references or
an incomplete scan produces `unknown`. `needs_repath` means at least one
canonical reference to a shipped asset was found, even at the 99% threshold.
False does not prove safety when status is unknown.

Canonical references only count when their asset is shipped in that WAD.
References to game-only assets do not reduce the score. Custom champion
namespaces and ambiguous modder-root paths are excluded; champion names and
known subunits come from `champion_list.json`. With an empty champion list the
checker conservatively treats character paths as canonical. BIN references,
soundbanks, and voiceover paths are excluded because Hematite does not repath
them. Both plaintext and `file` hashes are inspected, including nested
containers and map keys/values. Duplicate paths count once per BIN, including
a path referenced through both string and hash forms. Hash names resolve from
shipped paths, BIN strings, embedded path records, then the hash dictionary.

An input without a BIN cannot enter repathing, prefix deduplication, placeholder
injection, or repath's game-file pull, even with an explicit game WAD. An
unreadable input BIN blocks that WAD's repath too: its references cannot safely
follow renamed files. The pipeline also checks that a readable BIN remains
after standard fixes. Other selected fixes can still run. Standalone BIN input
continues to have no asset-repath stage; there is no WAD inventory to score.

## Configuration

`enabled_fixes` now selects **and orders** default fixes. New remote rules no
longer need an addition to the CLI's compiled selection list. Gear/CAC pulls
precede entry validation in the shipped list. Legacy configs without
`enabled_fixes` retain the historical order for existing rules and append new
enabled rules in deterministic order.

`[repath] layout` accepts `nested` or `in_folder`. Explicit CLI prefix/layout
flags override config; a configured prefix is used literally. Only an empty
prefix requests derivation from the input name. The shipped config uses
`prefix = "hematite"` and `layout = "nested"` consistently in CLI and interactive
runs. The interactive no-repath action sets `no_repath` explicitly.

Path rewriting, hashing, BIN/soundbank exclusions, the no-BIN guard, and the
verdict algorithm remain Rust code. Field migration targets, fix enablement,
execution order, placeholder patterns, and repath defaults remain configuration.

## LTK coverage comparison

The current [rule registry](https://github.com/LeagueToolkit/ltk-manager/blob/018fe6ec906b1a28c98817996f52a4b36fafced9/crates/ltk-manager-core/src/problems/rules/mod.rs)
contains five rule families. Its older gap-analysis document describes an
earlier, one-rule implementation; this comparison uses current code.

| Area | Hematite after this change | Remaining difference |
|---|---|---|
| String-to-file migration | Config expanded from 7 to all 385 supported field pairs in LTK's 16.17.8087655 table; existing transform handles scalar, list, populated option, and map-value strings | Hematite does not gate this table against an installed game build. It also cannot preserve/retype empty container declarations with its current value-only model. |
| Other type migrations | Existing general type transforms remain available | The table's 7 hash-to-file rows, 1 map-key rehash, and 2 structural rows are not added as string rules. They require name resolution or different transforms. |
| TEX alignment | Existing configurable `fix_tex_dimensions` converter | No new alignment algorithm introduced. |
| Audio bank version | Existing configurable version check and reference-aware removal retained | LTK walks bank chunks, accepts supported legacy media-only banks, and checks live fallback availability before removing a referenced incompatible bank. Hematite's version-only rule can still delete a valid orphan media bank or retain an incompatible referenced bank. |
| Unset soundbank ID | No equivalent check | LTK can derive an ID from a bank-unit's intended name. A hex chunk filename is insufficient evidence. |
| ResourceResolver key loss | Existing gear/CAC pulls and entry validation do not cover this | LTK compares resolver keys with the live game and reports informational loss; it deliberately offers no archive repair. Baking game resolver data into a mod would tie it to one patch. |

The added field-pair inventory is derived from LTK's published
[migration table](https://github.com/LeagueToolkit/ltk-manager/blob/018fe6ec906b1a28c98817996f52a4b36fafced9/crates/ltk-manager-core/src/problems/tables/binfile_migration_16.17.8087655.jsonl).
It contains 395 rows: 385 `hash_value`, 7 `rehash`, 1 `hash_key`, and 2 `none`.
Only `hash_value` rows were added to the existing `file_ref_migration` rule.
No LTK dependency or implementation was added.

Audio comparison sources:
[bank version](https://github.com/LeagueToolkit/ltk-manager/blob/018fe6ec906b1a28c98817996f52a4b36fafced9/crates/ltk-manager-core/src/problems/rules/audio_bank_version/mod.rs),
[bank ID](https://github.com/LeagueToolkit/ltk-manager/blob/018fe6ec906b1a28c98817996f52a4b36fafced9/crates/ltk-manager-core/src/problems/rules/audio_bank_id/mod.rs).
Resolver source:
[key-loss rule](https://github.com/LeagueToolkit/ltk-manager/blob/018fe6ec906b1a28c98817996f52a4b36fafced9/crates/ltk-manager-core/src/problems/rules/bin_resolver_key_loss/mod.rs).

## Validation

Synthetic regression coverage exercises BIN-less packed WADs and mixed fantome
archives, loose WAD folders, unreadable BINs, hash-named BIN discovery, embedded
path names, unresolved hashes, multi-WAD report aggregation, detect-only disk
preservation, repath idempotency, config-selected new rules, menu opt-out, and
new migration targets. No copyrighted game fixtures are required. This verifies
processing behavior; in-game validation of the expanded migration table remains
outside these tests.
