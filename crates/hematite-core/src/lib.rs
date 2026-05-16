//! # hematite-core
//!
//! The fix engine for Hematite. This crate contains all detection and transformation
//! logic but has **zero** league-toolkit imports. It operates on types from
//! [`hematite_types`] and interacts with LTK through trait abstractions.
//!
//! ## Key rule: NO LTK IMPORTS
//! When `ltk_meta` / `ltk_ritobin` break their APIs, only `hematite-ltk` needs updating.
//! This crate stays untouched.
//!
//! ## Modules
//! - [`traits`] — `BinProvider`, `HashProvider`, `WadProvider` trait definitions
//! - [`context`] — `FixContext` runtime state
//! - [`pipeline`] — Fix orchestration: detect → transform → result
//! - [`detect`] — Issue detection rules
//! - [`transform`] — Fix transform actions
//!
//! ### Shared utilities (LOC reduction)
//! - [`walk`] — `PropertyWalker` visitor pattern (replaces 6 recursive walk impls)
//! - [`filter`] — `ObjectFilter` (replaces 15+ inline iteration loops)
//! - [`factory`] — `ValueFactory` JSON → PropertyValue conversion
//! - [`strings`] — Extension replace, FNV-1a hash, path normalization
//! - [`fallback`] — Asset fallback with Jaro-Winkler similarity

pub mod assets;
pub mod context;
pub mod detect;
pub mod factory;
pub mod fallback;
pub mod filter;
pub mod pipeline;
pub mod repath;
pub mod seeds;
pub mod strings;
pub mod traits;
pub mod transform;
pub mod wad_pipeline;
pub mod walk;
