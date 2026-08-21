//! Enumerate every fix defined in a [`FixConfig`] as flat metadata.
//!
//! Drives Flint's fix picker: id, name, description, severity, and whether the
//! fix came from the BIN-level `fixes` map or the WAD-level `wad_fixes` map.

use hematite_types::config::FixConfig;

/// Flat metadata for a single configured fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixInfo {
    /// Config key / fix ID (e.g. `"healthbar_fix"`).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// What the fix does and why.
    pub description: String,
    /// `low` | `medium` | `high` | `critical`.
    pub severity: String,
    /// Whether the fix is enabled by default in the config.
    pub enabled: bool,
    /// `true` when the fix came from `wad_fixes` (WAD-level), `false` when it
    /// came from `fixes` (BIN-level).
    pub wad_level: bool,
}

/// List every fix in `config` — BIN-level (`fixes`) then WAD-level
/// (`wad_fixes`) — as flat [`FixInfo`] records. Disabled fixes are included
/// too (an embedder may want to surface them greyed-out).
///
/// Iteration order within each map is not guaranteed (the config uses a
/// `HashMap`); callers that need a stable order should sort by `id`.
pub fn list_fixes(config: &FixConfig) -> Vec<FixInfo> {
    let mut infos = Vec::with_capacity(config.fixes.len() + config.wad_fixes.len());

    for (id, rule) in &config.fixes {
        infos.push(FixInfo {
            id: id.clone(),
            name: rule.name.clone(),
            description: rule.description.clone(),
            severity: rule.severity.clone(),
            enabled: config.is_fix_enabled(id),
            wad_level: false,
        });
    }

    for (id, rule) in &config.wad_fixes {
        infos.push(FixInfo {
            id: id.clone(),
            name: rule.name.clone(),
            description: rule.description.clone(),
            severity: rule.severity.clone(),
            enabled: config.is_fix_enabled(id),
            wad_level: true,
        });
    }

    infos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_every_fix_with_name_and_description() {
        // Load the repo's embedded config the same way the CLI test does.
        let raw = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/fix_config.toml"
        ))
        .unwrap();
        let config: hematite_types::config::FixConfig = toml::from_str(&raw).unwrap();
        let infos = list_fixes(&config);
        assert!(!infos.is_empty());
        for i in &infos {
            assert!(!i.name.trim().is_empty(), "fix {} has empty name", i.id);
            assert!(
                !i.description.trim().is_empty(),
                "fix {} has empty description",
                i.id
            );
        }
        // BIN-level + WAD-level counts add up to the config's two maps.
        assert_eq!(
            infos.len(),
            config.fixes.len() + config.wad_fixes.len(),
            "list_fixes must cover both fixes and wad_fixes"
        );
    }
}
