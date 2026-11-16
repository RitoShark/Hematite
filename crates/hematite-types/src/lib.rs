//! # hematite-types
//!
//! Config schemas, legacy repair types, and the shared rs_bin conversion bridge.
//! Native document metadata and empty container tags survive legacy transforms.
//!
//! ## Modules
//! - [`hash`] — Newtype wrappers for League's 4 hash kinds
//! - [`bin`] — BIN tree types (our abstraction over LTK's BinTree)
//! - [`wad`] — WAD chunk metadata and modification tracking
//! - [`config`] — Fix rule schema (deserialised from `fix_config.json`)
//! - [`result`] — Processing and fix result types
//! - [`champion`] — Champion list and character-relation lookups

pub mod bin;
pub mod champion;
pub mod config;
pub mod convert;
pub mod edit;
pub mod hash;
pub mod repath;
pub mod result;
pub mod wad;
