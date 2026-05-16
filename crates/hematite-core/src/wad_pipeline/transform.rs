//! WAD-level transform actions.
//!
//! Applies transformations to files at the WAD level.

use anyhow::Result;
use hematite_types::config::WadTransformAction;

/// Result of applying a WAD transform action.
#[derive(Debug, Clone)]
pub enum ActionResult {
    RemoveFile,
    ConvertFile {
        from_ext: String,
        to_ext: String,
        converter: String,
    },
    /// In-place byte transform — same path, mutated contents.
    TransformInPlace {
        converter: String,
    },
    RenameFile {
        new_path: String,
    },
    NoOp,
}

/// Apply a WAD transform action to a file.
pub fn apply_action(
    path: &str,
    _bytes: &[u8],
    action: &WadTransformAction,
) -> Result<ActionResult> {
    match action {
        WadTransformAction::RemoveFile => Ok(ActionResult::RemoveFile),

        WadTransformAction::ConvertFormat {
            from_ext,
            to_ext,
            converter,
        } => Ok(ActionResult::ConvertFile {
            from_ext: from_ext.clone(),
            to_ext: to_ext.clone(),
            converter: converter.clone(),
        }),

        WadTransformAction::TransformBytes { converter } => Ok(ActionResult::TransformInPlace {
            converter: converter.clone(),
        }),

        WadTransformAction::RenameFile {
            pattern,
            replacement,
        } => {
            let regex = regex::Regex::new(pattern)?;

            if let Some(new_path) = regex
                .replace(path, replacement.as_str())
                .into_owned()
                .into()
            {
                if new_path != path {
                    return Ok(ActionResult::RenameFile { new_path });
                }
            }

            Ok(ActionResult::NoOp)
        }

        // `AddFiles` doesn't operate on a single matched file; it's
        // processed at the rule level by `apply_single_fix`.
        WadTransformAction::AddFiles { .. } => Ok(ActionResult::NoOp),
    }
}
