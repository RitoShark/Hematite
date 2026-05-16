//! Embedded asset registry — named blobs that fix rules can inject into
//! a WAD via `WadTransformAction::AddFiles`.
//!
//! The registry exists so config-driven asset injection doesn't need a
//! recompile every time we want to ship a new fallback texture: the JSON
//! references an asset by name (`"invis_tex"`, `"toonshading_tex"`, …)
//! and this module resolves the name to bytes at runtime. Callers can
//! **register their own assets at startup** before the pipeline runs —
//! useful for shipping fallbacks bundled with a downstream binary or
//! tests.
//!
//! ## Built-ins
//! Only the truly universal placeholder ships with the core crate:
//!
//! | Name        | Description                                |
//! |-------------|--------------------------------------------|
//! | `invis_tex` | 1×1 fully transparent `.tex` (existing).   |
//!
//! Heavier validated fallbacks (toonshading, outlinetonemap, particle
//! whites) are registered by the CLI binary so the library doesn't carry
//! blob bytes it doesn't strictly need.

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::RwLock;

/// Bytes of an invisible 1×1 TEX texture used as a placeholder. Mirrors
/// `repath::INVIS_TEX` — kept here too so the asset registry is self-
/// contained for `WadTransformAction::AddFiles` callers.
pub const INVIS_TEX: &[u8] = include_bytes!("assets/invis.tex");

static REGISTRY: Lazy<RwLock<HashMap<&'static str, &'static [u8]>>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("invis_tex", INVIS_TEX);
    RwLock::new(m)
});

/// Look up an asset's bytes by name. Returns `None` if no entry exists.
pub fn get(name: &str) -> Option<&'static [u8]> {
    REGISTRY.read().ok()?.get(name).copied()
}

/// Register an asset under a static name. Idempotent — a second call with
/// the same name replaces the previous bytes (lets callers ship different
/// flavours of the same placeholder if they need to).
///
/// `bytes` is `&'static` because the assets we ship are always either
/// `include_bytes!` outputs or `Box::leak`'d at startup; the registry
/// holds plain `&[u8]` to keep lookup pointer-cheap.
pub fn register(name: &'static str, bytes: &'static [u8]) {
    if let Ok(mut guard) = REGISTRY.write() {
        guard.insert(name, bytes);
    }
}

/// Names currently registered. Order is unspecified.
pub fn registered_names() -> Vec<&'static str> {
    REGISTRY
        .read()
        .map(|g| g.keys().copied().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invis_tex_resolves() {
        let bytes = get("invis_tex").expect("invis_tex must be registered by default");
        assert_eq!(bytes, INVIS_TEX);
    }

    #[test]
    fn unknown_asset_returns_none() {
        assert!(get("__definitely_not_registered__").is_none());
    }

    #[test]
    fn register_then_get_round_trips() {
        static BYTES: &[u8] = b"hello-world";
        register("__test_round_trip__", BYTES);
        assert_eq!(get("__test_round_trip__"), Some(BYTES));
    }
}
