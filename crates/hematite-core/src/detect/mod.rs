//! Issue detection rules.
//!
//! Each [`DetectionRule`] variant maps to a detection function in [`rules`].
//! Detection is read-only — it examines the BIN tree and returns true/false.

pub mod bnk;
pub mod dead_asset;
pub mod dead_links;
pub mod replaced_bin;
pub mod rules;
pub mod shader;
pub mod shader_link;
pub mod stale_character;
pub mod skin;

pub use rules::detect_issue;
