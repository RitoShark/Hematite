//! RenameHash transform.
//!
//! Renames a field hash across the BIN tree.
//!
//! ## Used by
//! - `staticmat_texturepath`: TextureName → TexturePath
//! - `staticmat_samplername`: SamplerName → TextureName
//!
//! ## Why one of them looks at the value
//! A shader sampler carries two different things under similar names: the sampler's own
//! name (`Diffuse_Texture`, `ToonShadingTex`) and the path of the texture bound to it. The
//! pair of renames above moves each into its modern field, and renaming blindly gets that
//! wrong in two ways.
//!
//! A sampler that only ever had a NAME and no path had that name promoted into
//! `texturePath`, so the material pointed at "Diffuse_Texture", which is not a file. The
//! migration pass then hashed it as though it were one. Where there had been a real path, it
//! was gone.
//!
//! It was also not repeatable. The second rename leaves a sampler name sitting in
//! `TextureName`, which is exactly what the first rename looks for, so repairing a mod twice
//! corrupted materials that survived the first pass.
//!
//! Both go away by asking what the value is: a path moves, a name stays. That makes the
//! result the same however many times it runs, and whichever order the two rules fire in.

use crate::context::FixContext;
use hematite_types::bin::{BinProperty, PropertyValue};
use hematite_types::hash::FieldHash;

/// Extensions that make a bare, slash-free string an asset path.
const ASSET_EXTENSIONS: &[&str] = &[".dds", ".tex", ".png", ".jpg", ".tga", ".dat"];

/// Whether this value names a file rather than a sampler.
///
/// A path has directory separators, or at least an asset extension. A sampler name has
/// neither: `Diffuse_Texture`, `ToonShadingRimTex`, `ScreenSpaceTexture`.
///
/// An already-migrated reference is a hash rather than a string. It can only have come from
/// a path, so it is one.
pub fn is_asset_path(value: &PropertyValue) -> bool {
    match value {
        PropertyValue::WadHash(_) => true,
        PropertyValue::String(s) => {
            if s.contains('/') || s.contains('\\') {
                return true;
            }
            let lower = s.to_ascii_lowercase();
            ASSET_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
        }
        _ => false,
    }
}

/// Rename every `from` field to `to`.
///
/// With `only_asset_paths`, a field whose value is not a path is left alone. See the module
/// docs for why that matters on shader samplers.
pub fn apply(ctx: &mut FixContext, from_name: &str, to_name: &str, only_asset_paths: bool) -> u32 {
    let Some(from_hash) = ctx.hashes.field_hash(from_name) else {
        return 0;
    };
    let Some(to_hash) = ctx.hashes.field_hash(to_name) else {
        return 0;
    };
    if from_hash.0 == to_hash.0 {
        return 0;
    }

    let mut renamed = 0;
    for obj in ctx.tree.objects.values_mut() {
        renamed += rename_in_properties(
            &mut obj.properties,
            from_hash.0,
            to_hash.0,
            only_asset_paths,
        );
    }
    renamed
}

fn rename_in_properties(
    properties: &mut indexmap::IndexMap<u32, BinProperty>,
    from: u32,
    to: u32,
    only_asset_paths: bool,
) -> u32 {
    // Descend first, so nested structs are handled whether or not this level renames.
    let mut renamed: u32 = properties
        .values_mut()
        .map(|prop| rename_in_value(&mut prop.value, from, to, only_asset_paths))
        .sum();

    let qualifies = properties
        .get(&from)
        .map(|prop| !only_asset_paths || is_asset_path(&prop.value))
        .unwrap_or(false);
    if !qualifies {
        return renamed;
    }
    // Never overwrite a field that is already there. The two hold different things and one
    // of them would vanish without a trace.
    if properties.contains_key(&to) {
        tracing::debug!("not renaming {from:08x} to {to:08x}: the target field already exists");
        return renamed;
    }

    if let Some(mut prop) = properties.swap_remove(&from) {
        prop.name_hash = FieldHash(to);
        properties.insert(to, prop);
        renamed += 1;
    }
    renamed
}

fn rename_in_value(value: &mut PropertyValue, from: u32, to: u32, only_asset_paths: bool) -> u32 {
    match value {
        PropertyValue::Struct(s) | PropertyValue::Embedded(s) => {
            rename_in_properties(&mut s.properties, from, to, only_asset_paths)
        }
        PropertyValue::Container(items) | PropertyValue::UnorderedContainer(items) => items
            .iter_mut()
            .map(|item| rename_in_value(item, from, to, only_asset_paths))
            .sum(),
        PropertyValue::Optional(inner) => match inner.as_mut() {
            Some(v) => rename_in_value(v, from, to, only_asset_paths),
            None => 0,
        },
        PropertyValue::Map(entries) => entries
            .iter_mut()
            .map(|(_, v)| rename_in_value(v, from, to, only_asset_paths))
            .sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_a_path() {
        assert!(is_asset_path(&PropertyValue::String(
            "ASSETS/Characters/Jhin/x.dds".into()
        )));
        assert!(is_asset_path(&PropertyValue::String(
            r"ASSETS\Characters\Jhin\x.tex".into()
        )));
        // No directory, but an extension still makes it a file.
        assert!(is_asset_path(&PropertyValue::String("x.dds".into())));
        // Already migrated: a hash can only have come from a path.
        assert!(is_asset_path(&PropertyValue::WadHash(0x1234)));
    }

    /// The exact values that were being promoted into `texturePath`. None is a file.
    #[test]
    fn a_sampler_name_is_not_a_path() {
        for name in [
            "Diffuse_Texture",
            "ToonShadingTex",
            "ToonShadingOutlineTex",
            "ToonShadingRimTex",
            "ScreenSpaceTexture",
        ] {
            assert!(
                !is_asset_path(&PropertyValue::String(name.into())),
                "{name} was treated as a path"
            );
        }
    }

    #[test]
    fn a_non_string_value_is_not_a_path() {
        assert!(!is_asset_path(&PropertyValue::U32(1)));
        assert!(!is_asset_path(&PropertyValue::String(String::new())));
    }
}
