//! Library-neutral boundary over live base-game file access.
//!
//! ## Boundary decision
//!
//! The lifted recovery code (deep-repair, restore-anm, combo-bin relocation)
//! reaches the live game in two distinct ways:
//!
//! 1. As `&dyn hematite_core::traits::GameProvider` — the trait the BIN fix
//!    engine reads through `FixContext.game` (`has_path`/`pull_raw`/`game_bin`).
//! 2. Via the concrete [`crate::live_provider::LiveGameProvider`]'s inherent
//!    `with_index(...)` method, which hands the recovery passes a
//!    `&mut GameIndex` to snapshot hashes / pull raw chunks in bulk.
//!
//! Because (2) needs an inherent method that `GameProvider` does not (and
//! cannot object-safely) expose, `LiveGameProvider` moves into this crate
//! alongside the recovery code (see spec §"What moves"), rather than being
//! hidden behind a new trait. The remaining boundary — the FixContext one —
//! is already `GameProvider`, so `GameFileAccess` is a re-export of it. No new
//! trait is invented; embedders that only need the FixContext surface can
//! implement `GameProvider` directly.

pub use hematite_core::traits::GameProvider as GameFileAccess;
