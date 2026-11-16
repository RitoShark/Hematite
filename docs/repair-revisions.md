# Repair revisions and validation

A repair creates a new directory under `Hematite-Fixed/<input-name>.hematite/rNNN/`. The mod retains its original filename. Each directory contains the repaired artifact and `hematite-repair.json`. An unchanged input still receives a copy and a validation record. Check and dry-run modes create no revision. A failed processing run creates no completed record; a written output with validation problems is retained for inspection and reported as an error.

The record keeps the repair revision separate from the author's package version, which is preserved. It records the input and output SHA-256, the Hematite version, the rules version and deterministic digest, the effective selected rules and settings, applied-fix records, and errors. Folder fingerprints include sorted relative paths, lengths, and contents. A parent revision is accepted only when its output fingerprint matches the current input. Keep the record beside the artifact when moving it.

The target League build comes from the installed game's `content-metadata.json`. Without a readable build it is null. An explicit game WAD does not establish a build; its output is not labeled with an unrelated install's version. The source build is unknown unless a verified parent record states the previous repair target. These labels do not claim the original mod's creation patch or in-game compatibility.

## Validation scope

After writing the staged output, Hematite reopens it and checks:

- BIN parsing, preservation through the adapter, and stable serialization.
- File-hash and recognized string-path references, including linked BIN paths, across the package's WADs and the installed game's WAD inventory.
- BNK/WPK container parsing, bank media bounds and IDs, readable hierarchy, and embedded WEM headers.

The JSON lists the checks actually attempted. `passed_automated_checks` applies only to this scope. Target-schema conformance, BIN object-link resolution, layer-combination simulation, runtime asset loading, and in-game appearance are not certified. Multi-layer packages and standalone BINs explicitly report limited coverage. With no complete game inventory, an absent external reference makes the result `incomplete`, not a proven missing-game-file error. Confirmed structural failures or references absent from a complete inventory produce `has_unresolved_issues`. `in_game_tested` is always false.

## VFX repair

Shape migration retains the old shape's fields in `VfxShapeLegacy`, moves the complete birth-translation value to `EmitterPosition`, and preserves modern fields already present. It also repairs birth translations stranded inside an existing legacy spawn shape. Curves and unknown shape fields are retained instead of rebuilding a simplified volume.

## Optional bank recompression

`--compress-audio` adds the `compress_audio` and `compress_audio_wpk` rules to the selected repairs. They are excluded from the default enabled rules. Custom rule files can also enable them explicitly.

The adapter uses `rs_audio` to decode and re-encode Vorbis WEMs at quality 0.4, accepting a result only when smaller and with unchanged sample rate, channel count, and decoded duration. It preserves bank headers, IDs, event hierarchy, unknown bank sections, WPK entry names, and dead slots. PCM and streams with extra RIFF metadata, such as loop/cue chunks, are retained. This is lossy compression, not a Wwise event-bank version upgrade. Failed conversions retain the original bank and are reported as failed repairs. Verified repair ancestry prevents recompressing an already-compressed revision again.

The improvements were evaluated against [ltk-reforge at 7c1cc0c](https://github.com/LeagueToolkit/ltk-reforge/tree/7c1cc0ca6c06b5fcbf0f0421769f674307057fca), using Hematite's existing RitoShark format adapters.
