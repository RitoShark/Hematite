//! # hematite-types
//!
//! Pure data types for the Hematite skin fixer.
//! This crate has **zero** league-toolkit dependencies — it defines Hematite's
//! own domain model that the rest of the workspace operates on.
//!
//! ## Modules
//! - [`hash`] — Newtype wrappers for League's 4 hash kinds
//! - [`bin`] — BIN tree types (our abstraction over LTK's BinTree)
//! - [`wad`] — WAD chunk metadata and modification tracking
//! - [`config`] — Fix rule schema (deserialised from `fix_config.json`)
//! - [`diagnostic`] — What the engine reports: severity, reason catalog, check reports
//! - [`result`] — Processing and fix result types
//! - [`champion`] — Champion list and character-relation lookups

pub mod bin;
pub mod champion;
pub mod config;
pub mod diagnostic;
pub mod hash;
pub mod repath;
pub mod result;
pub mod wad;
