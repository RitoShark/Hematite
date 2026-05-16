//! Fix transform actions.
//!
//! Each [`TransformAction`] variant maps to a handler in its own module.
//! Transforms mutate the BIN tree in-place and return a change count.
//!
//! ## Modules and what they handle
//! | Module | TransformAction | Shared utils |
//! |--------|----------------|--------------|
//! | [`ensure_field`] | `EnsureField`, `EnsureFieldWithContext` | ObjectFilter, ValueFactory |
//! | [`rename_hash`] | `RenameHash` | PropertyWalker (visit_field_hash) |
//! | [`replace_ext`] | `ReplaceStringExtension` | PropertyWalker (visit_string) |
//! | [`change_type`] | `ChangeFieldType` | ObjectFilter, ValueFactory |
//! | [`regex_ops`] | `RegexReplace`, `RegexRenameField` | PropertyWalker (visit_string) |
//! | [`vfx_shape`] | `VfxShapeFix` | ObjectFilter |
//! | [`remove`] | `RemoveFromWad` | (trivial) |
//! | [`shader_fallback`] | `ShaderFallback` | ShaderValidator token matching |
//! | [`remove_unreferenced`] | `RemoveUnreferencedEntries` | Link collection + entry removal |

pub mod change_type;
pub mod ensure_field;
pub mod regex_ops;
pub mod remove;
pub mod remove_unreferenced;
pub mod rename_hash;
pub mod replace_ext;
pub mod shader_fallback;
pub mod split_entries;
pub mod vfx_shape;

use crate::context::FixContext;
use hematite_types::config::TransformAction;

/// Main transform dispatch. Returns number of changes applied.
///
/// The entry_type parameter is used by transforms that need to filter objects
/// (EnsureField, VfxShapeFix). It should come from the detection rule's entry_type.
pub fn apply_transform(
    action: &TransformAction,
    ctx: &mut FixContext<'_>,
    entry_type: Option<&str>,
) -> u32 {
    match action {
        TransformAction::EnsureField {
            field,
            value,
            data_type,
            create_parent,
        } => {
            let Some(entry_type) = entry_type else {
                return 0;
            };
            let data_type_str = data_type.to_lowercase();
            ensure_field::apply(
                ctx,
                entry_type,
                field,
                value,
                &data_type_str,
                create_parent.as_ref(),
            )
        }
        TransformAction::RenameHash { from_hash, to_hash } => {
            rename_hash::apply(ctx, from_hash, to_hash)
        }
        TransformAction::ReplaceStringExtension {
            from,
            to,
            path_prefixes,
            field_filter,
        } => replace_ext::apply(
            ctx,
            from,
            to,
            path_prefixes,
            field_filter.as_deref(),
        ),
        TransformAction::RemoveFromWad => remove::apply(ctx),
        TransformAction::ChangeFieldType {
            from_type,
            to_type,
            append_values,
            ..
        } => change_type::apply(ctx, from_type, to_type, append_values),
        TransformAction::RegexReplace {
            pattern,
            replacement,
            field_filter,
        } => regex_ops::apply_replace(ctx, pattern, replacement, field_filter.as_deref()),
        TransformAction::RegexRenameField {
            pattern,
            replacement,
        } => regex_ops::apply_rename(ctx, pattern, replacement),
        TransformAction::VfxShapeFix => {
            let Some(entry_type) = entry_type else {
                return 0;
            };
            vfx_shape::apply(ctx, entry_type)
        }
        TransformAction::ShaderFallback {
            shader_def_type,
            shader_link_field,
        } => {
            if let Some(validator) = ctx.shader_validator {
                shader_fallback::apply(ctx, shader_def_type, shader_link_field, validator)
            } else {
                tracing::warn!("Shader fallback requested but no shader validator available");
                0
            }
        }
        TransformAction::RemoveUnreferencedEntries {
            main_entry_type,
            targets,
        } => remove_unreferenced::apply(ctx, main_entry_type, targets),
        TransformAction::SplitEntriesByType {
            entry_types,
            output_path_template,
            link_in_source,
        } => split_entries::apply(ctx, entry_types, output_path_template, *link_in_source),
    }
}
