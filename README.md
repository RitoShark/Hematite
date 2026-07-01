<!--
  Hematite — README
  Animated header banners + typing SVG render on GitHub.com out of the
  box. They degrade to plain text on local viewers / offline rendering.
-->

<p align="center">
  <img src="https://capsule-render.vercel.app/api?type=waving&color=0:1a1a2e,50:721121,100:c1272d&height=220&section=header&text=Hematite&fontSize=92&fontColor=ffffff&animation=fadeIn&fontAlignY=42" alt="Hematite banner" />
</p>

<p align="center">
  <a href="https://github.com/RitoShark/Hematite/releases">
    <img src="https://readme-typing-svg.demolab.com?font=JetBrains+Mono&size=20&duration=3200&pause=900&color=C1272D&center=true&vCenter=true&width=820&lines=Detect+%26+fix+broken+League+of+Legends+skins;Config-driven+rules+%E2%80%94+no+recompile+to+ship+a+fix;1.8M+hashes%2C+loaded+in+%3C1s;Drag.+Drop.+Done." alt="What Hematite does" />
  </a>
</p>

<p align="center">
  <a href="https://github.com/RitoShark/Hematite/releases"><img src="https://img.shields.io/github/v/release/RitoShark/Hematite?style=for-the-badge&label=release&color=c1272d&labelColor=1a1a2e" alt="Release"></a>
  <img src="https://img.shields.io/badge/rust-2021-c1272d?style=for-the-badge&labelColor=1a1a2e&logo=rust" alt="Rust">
  <img src="https://img.shields.io/badge/platform-windows-c1272d?style=for-the-badge&labelColor=1a1a2e&logo=windows" alt="Windows">
  <img src="https://img.shields.io/badge/tests-152_passing-2e7d32?style=for-the-badge&labelColor=1a1a2e" alt="Tests">
  <img src="https://img.shields.io/badge/config--driven-yes-c1272d?style=for-the-badge&labelColor=1a1a2e" alt="Config-driven">
</p>

<p align="center">
  <b>Drop a <code>.fantome</code>, <code>.wad.client</code>, or <code>.bin</code> on Hematite.</b><br/>
  <sub>It detects, repairs, and writes the file back. Auto-updates fix rules from GitHub. No recompile needed.</sub>
</p>

---

## At a glance

<table>
<tr>
<td width="50%" valign="top">

### For users

```bash
# Auto-detect + fix everything
hematite-cli "skin.fantome"

# Preview without writing
hematite-cli "skin.fantome" --dry-run

# Batch a whole folder
hematite-cli "C:/mods/"

# Detect only — show champion + skin
hematite-cli "skin.fantome" --check
```

Releases ship at **[github.com/RitoShark/Hematite/releases](https://github.com/RitoShark/Hematite/releases)**.

</td>
<td width="50%" valign="top">

### For devs

```bash
git clone https://github.com/RitoShark/Hematite
cd Hematite && git checkout v2
cargo build --release
cargo test --workspace      # 152 tests
```

Full architecture, transform framework, contribution flow → **[DEVELOPER.md](DEVELOPER.md)**.

</td>
</tr>
</table>

---

## What gets fixed

<table>
<tr>
<th align="left">Symptom</th>
<th align="left">What Hematite does</th>
<th align="left">Flag</th>
</tr>
<tr><td>Invisible HP bar</td><td>Adds missing <code>UnitHealthBarStyle</code></td><td><code>--healthbar</code></td></tr>
<tr><td>White / chrome model</td><td>Renames <code>TextureName</code>/<code>SamplerName</code> in materials</td><td><code>--white-model</code></td></tr>
<tr><td>Black or missing icons</td><td>Rewrites <code>.dds</code> → <code>.tex</code> when the WAD lacks the source</td><td><code>--black-icons</code></td></tr>
<tr><td>Broken particle textures</td><td>Recursive <code>.dds</code> → <code>.tex</code> conversion</td><td><code>--particles</code></td></tr>
<tr><td>Outdated champion data</td><td>Removes stale champion BIN entries</td><td><code>--remove-champion-bins</code></td></tr>
<tr><td>Crackling / silent audio</td><td>Drops BNK files with incompatible Wwise versions</td><td><code>--remove-bnk</code></td></tr>
<tr><td>Animations that locked the rig</td><td>Removes problematic <code>.anm</code> files</td><td><code>--remove-anm</code></td></tr>
<tr><td>VFX gone / wrong shape</td><td>Migrates VFX shape data to the 14.1+ layout</td><td><code>--vfx-shape</code></td></tr>
<tr><td>Invisible model from bad shader</td><td>Replaces invalid shader refs with the closest valid match</td><td><code>--fix-shaders</code></td></tr>
<tr><td>Orphan entries bloating the BIN</td><td>Removes unreferenced CAD/AnimGraph/GearSkinUpgrade entries</td><td><code>--validate-entries</code></td></tr>
</table>

<p align="center"><sub>No flags = all of the above. Pass <code>--check</code> to detect only.</sub></p>

---

## What's new in this release

<table>
<tr>
<td>

### Texture lifesavers
- **Mipmap stripping** for the post-2026 League regression that ate mipmapped textures
- **TEX dimension repair** — rounds non-block-aligned `.tex` dimensions down to multiples of 4 (no more DXT-block crashes)

</td>
<td>

### Smarter repathing
- **Modder-root paths** — `reddivinekinggaren/foo.dds` style namespaces now get rewritten properly (closes an 88K-ref gap)
- **Suffix-strip fallback** — handles Riot's `attack1.matcha_x.anm` → `attack1.anm` rename
- **`remove_prefix`** helper for the inverse direction

</td>
</tr>
<tr>
<td>

### Smarter detection
- **Seed discovery** — scans the WAD TOC and surfaces every champion/skin pair (jinx + jinxmine etc.) before fixes run
- **Field-scoped path rewrites** — `replace_string_extension` now takes a regex on the *field name*, so HUD-only rewrites stop touching material textures

</td>
<td>

### Always-up-to-date CLI
- **Force-update gate** — if a critical bug ships, bumping `min_cli_version` in [config/version.json](config/version.json) refuses to run on old CLIs
- **`--check-version`** to query the gate
- **`--skip-version-check`** for CI

</td>
</tr>
<tr>
<td>

### Deep repair
- **Seed-BIN backfill** — asset-only mods (loose textures/meshes, no character BIN) now get their canonical `skin{N}.bin` pulled from the base game as a foundation
- **Transitive dependency closure** — `--game-wad` now follows the *full* dependency chain (referenced assets **and** `.linked` BINs), recursing until the mod is self-contained

</td>
<td>

### We own the stack
- **Migrated off `league-toolkit`** onto the in-house [RitoShark `rs_*` crates](https://github.com/RitoShark/RitoShark-Crates) (`rs_bin`, `rs_wad`, `rs_tex`, `rs_mesh`, `rs_io`) — isolated entirely within `hematite-file`, so the fix engine never noticed

</td>
</tr>
</table>

> Three new transform primitives let configs do more without code: `transform_bytes` (in-place byte ops), `add_files` (inject named assets from the registry), and `split_entries_by_type` (move objects into a sibling BIN). See [DEVELOPER.md](DEVELOPER.md#transform-framework) for the schema.

---

## Quick start

```bash
# 1. Grab the binary
# → https://github.com/RitoShark/Hematite/releases/latest

# 2. Drag a mod onto it, or run from the terminal
hematite-cli "MyAwesomeSkin.fantome"

# 3. Hematite writes the fixed file next to the original:
#    MyAwesomeSkin.fixed.fantome
#
# (Same binary works as a drag-target on Windows — no terminal required.)
```

### Supported inputs

| Format | What happens |
|---|---|
| `.fantome` / `.zip` | Extracts every `.wad.client`, processes each, repacks into `.fixed.fantome` |
| `.wad.client` | Extracts, fixes, rebuilds → `.fixed.wad.client` |
| `.bin` | Parses, fixes, writes → `.fixed.bin` |
| Folder | Recurses + processes every supported file (parallel via rayon) |

### Useful flags

| Flag | What it does |
|---|---|
| `--check` | Detect only — prints champion / skin / issue count, doesn't touch the file |
| `--dry-run` | Show what *would* be fixed |
| `--json` | Emit machine-readable output (skips the "press enter" pause) |
| `--repath` | Rename mod assets with a unique prefix so they can't collide with the base game |
| `--game-wad <PATH>` | **Deep repair** (requires `--repath`): make the mod self-contained by pulling missing dependencies from the base-game champion WAD. Backfills the canonical `skin{N}.bin` for asset-only mods, then transitively resolves every referenced + linked file until the dependency chain is closed |
| `--invis-texture` | Inject invisible placeholders for missing texture refs after repath |
| `-v verbose` | Show every fix as it's applied; `-v trace` for everything |

Run `hematite-cli --help` for the complete flag list with descriptions.

---

## See it in action

<details>
<summary><b>What a clean run looks like</b></summary>

```
$ hematite-cli yone-spiritblossom.fantome

[hematite] Loading hash dictionary (1.8M entries) ............. 712ms
[hematite] Seed discovery: 1 skin across 1 champion
[hematite] WAD has 1834 total entries, 14 BIN file(s)
[hematite] WAD-level fix 'TEX Dimension Fix' affected 3 files
[hematite] WAD-level fix 'DDS → TEX Texture Conversion' affected 8 files
[hematite] data/characters/yone/skins/skin7.bin
[hematite]   ✓ Missing HP Bar (1 changes)
[hematite]   ✓ White Model (TextureName) (4 changes)
[hematite] Repathing assets with prefix ".yone7_" (layout: InFolder)…
[hematite] ✓ Renamed 47 WAD entries, rewrote 213 BIN strings
[hematite] Wrote yone-spiritblossom.fixed.fantome (4.2 MB)

Done — 1 mod processed, 18 fixes applied in 1.9s.
```
</details>

<details>
<summary><b>Force-update banner in the wild</b></summary>

When a critical bug ships, the remote `version.json` bumps `min_cli_version` and every old CLI refuses to run until upgraded:

```
[BLOCKED] Hematite-CLI 0.3.0 is too old — minimum required is 0.4.0.
  Fixes BIN parser regression on patch 14.20 mods.
  Download: https://github.com/RitoShark/Hematite/releases/latest
  Pass --skip-version-check to override at your own risk.

Error: Refusing to run: CLI is older than the published minimum.
```

For non-blocking updates, you get a soft notice instead and the run proceeds.
</details>

<details>
<summary><b>Subcharacter detection</b></summary>

Mods that ship subchampions (Jinx + jinxmine, Annie + Tibbers, Anivia + egg) used to lose those files silently. Now you see them up front:

```
[hematite] Seed discovery: 2 skins across 2 champions
[hematite] WAD contains subchampion forms: jinx, jinxmine
```
</details>

---

## Configure without recompiling

Every fix rule lives in **[config/fix_config.json](config/fix_config.json)**. Add a rule, push to `main`, the next CLI run picks it up (cached for 1 hour, embedded fallback when offline).

A rule has three parts:

```json
"my_new_fix": {
  "name": "Pretty name",
  "enabled": true,
  "severity": "medium",
  "detect": { "type": "...", "...": "..." },
  "apply":  { "type": "...", "...": "..." }
}
```

Detection rules cover field presence, hash existence, file-extension matches, binary header version checks, entry-type lookups, even shader validity. Transforms cover field add/rename, regex replace, file removal, in-place byte transforms, asset injection from the named registry, splitting entries into sibling BINs.

> The full schema and the recipe for adding a brand-new transform action live in **[DEVELOPER.md](DEVELOPER.md)**.

---

## Hash system in one paragraph

League uses 32-bit FNV-1a for class / field / path names and 64-bit xxhash for WAD asset paths. Hematite ships an LMDB containing **1.8M** resolved hashes — loads in under a second, lives under `%APPDATA%\RitoShark\Requirements\Hashes\`, auto-downloads on first run. Falls back to the text files if LMDB is missing.

---

## Why "Hematite"?

Hematite is the primary ore of iron. When iron oxidizes, it becomes *rust*. This tool is built in Rust and cleans up broken skins — the name fits.

<p align="center"><sub>
  Made by <a href="https://github.com/SirDexal">SirDexal</a> · part of the <a href="https://github.com/RitoShark">RitoShark</a> ecosystem<br/>
  Have a question, found a bug, want to contribute? → <a href="DEVELOPER.md">DEVELOPER.md</a>
</sub></p>

<p align="center">
  <img src="https://capsule-render.vercel.app/api?type=waving&color=0:c1272d,50:721121,100:1a1a2e&height=120&section=footer&animation=fadeIn" alt="footer" />
</p>
