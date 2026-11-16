# Configurable BIN edits

Use `hematite-cli input.bin --config rules.json --dry-run --no-pause` to preview your own rules, then remove `--dry-run` to write the normal fixed output. TOML works too. An explicit config replaces the default rule file; invalid files produce an error, never a fallback to unrelated fixes. See [edit_example.json](../config/edit_example.json) for a complete standalone config. Edit operations also work through the library and the WAD/folder pipelines.

A rule uses `apply.type = "edit"` and an ordered `operations` array. Use `detect.type = "always"` to let the program determine whether anything changes. A no-op is not reported as a failure or an issue. Existing detection predicates may also gate the program. The editor previews or executes a complete rule on a copy and commits only if every step succeeds. Errors identify the operation number. This transaction covers the BIN tree and recorded file paths, not the entire batch of files or other rules.

## Selection

Every field operation has a `select` object:

- `classes`: names or hex hashes of owning objects, including nested pointer/embed objects. Omit to match all classes.
- `entries`: optional top-level entry names/path hashes that restrict traversal.
- `path`: an array of field names/hex hashes relative to each matching object. `*` visits all fields in an object or all values in a list/option/map. For example `["SamplerValues", "*", "TextureName"]` selects only the sampler fields.
- `targets`: an alternative to `path`, containing `{ "class": "...", "field": "..." }` pairs. Useful for large migration tables; each pair matches the immediate owner class.

Names are hashed by rs_bin; no installed hash dictionary is needed for edits. Selectors must contain `path` or `targets`, never both. Unknown selector/operation keys are errors. Map wildcard traversal visits values, not keys. Select a whole container field for conversion or string replacement of its payloads. Use a typed `set` to replace a complete container or change map keys.

## Operations

| `op` | Additional fields | Behavior |
| --- | --- | --- |
| `set` | `value` | Add/replace the selected field; unchanged values are no-ops. |
| `ensure` | `value` | Add only if absent; preserve existing values. |
| `remove` | none | Remove the selected field. |
| `rename` | `to` | Rename in place; a conflicting destination is an error. |
| `convert` | `from`, `to` | Convert matching values using rs_bin's value parser. |
| `replace_string` | `pattern`, `replacement` | Regex replacement in selected strings or container payloads. |
| `set_class` | `class` | Change the class of selected pointer/embed values, preserving their fields. |
| `create_entry` | `path`, `class`, `fields` | Create an entry; identical existing entry is a no-op, conflicting entry is an error. No selector. |
| `remove_entry` | `paths` | Remove named/hashed entries. No selector. |

Conversion accepts canonical scalar BIN types (`u8`, `u32`, `vec3`, `string`, `file`, `embed`, `pointer`, etc.). It retains list/list2/option/map wrappers, updates their declared value tags even when empty, and preserves map key types. Unsupported conversions fail instead of silently truncating values. Numeric widening and pointer/embed conversion are examples; changing vector length requires setting the desired vector explicitly. `file` values use rs_bin's XXH64 path handling. Hematite has no class-specific hashing implementation in this edit path.

Put conversions that discard asset strings in `phase = "post_repath"`, so repathing runs while readable paths are still available. New file hash/path pairs are retained in memory for repair lookup; the editor does not append a new trailer to output BINs. Existing trailing bytes remain intact.

## Typed values and creation from an empty tree

Values use `{ "type": "...", "value": ... }`. Scalars use JSON scalar values, vectors/colors use numeric arrays, and `mtx44` uses a flat 16-number array. Integer/hash values outside their supported range are rejected by rs_bin. Hashes accept names, hex strings, or unsigned JSON integers.

Containers use canonical type labels: `list[file]`, `list2[embed]`, `option[string]`, or `map[hash,embed]`. Lists use arrays, options use `[]` or `[value]`, maps use arrays of `[key, value]` pairs. Empty containers retain the declared tags. Struct values contain `class` and a `fields` object with recursively typed values; a null pointer uses JSON `null`.

This operation builds new nested content without reading classes from the game:

```json
{
  "op": "create_entry",
  "path": "MyCustomEntry",
  "class": "MyCustomClass",
  "fields": {
    "texture": { "type": "file", "value": "assets/custom.tex" },
    "children": {
      "type": "list[embed]",
      "value": [
        { "class": "MyChild", "fields": {
          "enabled": { "type": "bool", "value": true }
        } }
      ]
    },
    "optionalTexture": { "type": "option[file]", "value": [] }
  }
}
```

Library callers can apply it to `BinTree::default()` using `hematite_core::transform::edit::apply` and serialize with `FileBinProvider`. The CLI still takes an existing BIN or mod as input. For deep insertion into existing entries, use ordered `ensure` steps to create missing parents, then `set` the final field; parent creation is explicit so existing siblings are preserved.

## Compatibility and boundaries

The default asset-reference, health-bar, material renaming, and HUD icon rules use the generic editor. The old `retype_string_to_file` action remains a compatibility wrapper around the generic conversion. Other existing parameterized operations remain supported for older configs.

Repathing, VFX structural migration, linked-BIN merging/splitting/relocation, live-game recovery, shader matching, and binary texture/mesh operations remain Rust algorithms. They are not a general scripting language. New combinations of the edit operations above need only config changes; a fundamentally new algorithm still needs code.

The generic editor operates on native `rs_bin::BinValue`. Legacy repair/repath code continues to use Hematite's tree through the shared bridge. The bridge now preserves document headers/patches and native empty container tags rather than defaulting them to `u32`.
