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
pub mod live_provider;
pub mod options;
pub mod progress;

// Re-exports are wired up as each module is filled in (Tasks 2–6).
// pub use fix_folder::fix_folder;
pub use game_access::GameFileAccess;
// pub use list_fixes::{list_fixes, FixInfo};
// pub use live_provider::LiveGameProvider;
// pub use options::FixOptions;
pub use progress::{NoopSink, ProgressSink};
