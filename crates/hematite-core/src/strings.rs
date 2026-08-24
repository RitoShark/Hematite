//! String utilities — extension replace, FNV-1a hash, path normalization.
//!
//! Consolidates string operations that were repeated in 3+ places in the old codebase.

/// Replace file extension in a path string (case-insensitive match).
///
/// Returns `Some(new_path)` if the extension was found and replaced,
/// `None` if the path doesn't end with `from`.
///
/// ## Example
/// ```
/// # use hematite_core::strings::replace_extension;
/// assert_eq!(
///     replace_extension("assets/icon.DDS", ".dds", ".tex"),
///     Some("assets/icon.tex".to_string())
/// );
/// ```
pub fn replace_extension(path: &str, from: &str, to: &str) -> Option<String> {
    let lower = path.to_lowercase();
    let from_lower = from.to_lowercase();
    if lower.ends_with(&from_lower) {
        let stem = &path[..path.len() - from.len()];
        Some(format!("{stem}{to}"))
    } else {
        None
    }
}

/// Check if a field name matches a wildcard pattern.
///
/// Supports: `*foo*` (contains), `foo*` (starts with), `*foo` (ends with), or exact match.
pub fn matches_pattern(name: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let starts_wild = pattern.starts_with('*');
    let ends_wild = pattern.ends_with('*');
    let core = pattern.trim_matches('*');

    match (starts_wild, ends_wild) {
        (true, true) => name.contains(core),
        (true, false) => name.ends_with(core),
        (false, true) => name.starts_with(core),
        (false, false) => name == pattern,
    }
}

/// Compute FNV-1a hash of a string (lowercase normalized).
///
/// This is how League computes field and type hashes.
/// Matches the algorithm in `league-toolkit/ltk_hash`.
pub fn fnv1a_hash(name: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.to_lowercase().bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Resolve a config-supplied class/field/key token to its 32-bit hash.
///
/// Accepts either a readable name or a literal `0x…` hex hash, because some classes are
/// only ever seen as hashes: no dictionary names them, so a config could not reference
/// them at all if names were the only accepted form.
pub fn resolve_hash_token(token: &str) -> u32 {
    token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .unwrap_or_else(|| fnv1a_hash(token))
}

/// Normalize a WAD asset path (lowercase, forward slashes).
pub fn normalize_wad_path(path: &str) -> String {
    path.to_lowercase().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_extension() {
        assert_eq!(
            replace_extension("assets/icon.DDS", ".dds", ".tex"),
            Some("assets/icon.tex".to_string())
        );
        assert_eq!(replace_extension("assets/model.skn", ".dds", ".tex"), None);
    }

    #[test]
    fn test_matches_pattern() {
        assert!(matches_pattern("TextureName", "Texture*"));
        assert!(matches_pattern("TextureName", "*Name"));
        assert!(matches_pattern("TextureName", "*ture*"));
        assert!(matches_pattern("TextureName", "TextureName"));
        assert!(!matches_pattern("TextureName", "Sampler*"));
    }

    #[test]
    fn test_fnv1a_hash() {
        // Known hash values can be verified against LTK
        let hash = fnv1a_hash("UnitHealthBarStyle");
        assert_ne!(hash, 0);
    }

    #[test]
    fn test_normalize_wad_path() {
        assert_eq!(
            normalize_wad_path("Assets\\Characters\\Lux\\Skins\\Skin01.dds"),
            "assets/characters/lux/skins/skin01.dds"
        );
    }
}
