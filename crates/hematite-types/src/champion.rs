//! Champion list and character-relation lookups.
//!
//! Loaded from `champion_list.json`. Provides:
//! - List of all champion names
//! - Subchamp mappings (e.g. Annie → Tibbers, Anivia → AniviaEgg)
//! - Healthbar values for non-champion entities (turrets, monsters, etc.)
//! - Reverse lookups: subchamp → primary champion

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Raw champion list data from champion_list.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChampionList {
    pub version: String,
    pub champions: Vec<String>,
    pub subchamps: HashMap<String, Vec<String>>,
    pub healthbar_values: HashMap<String, u8>,
    /// Characters that should be skipped during processing (crashes/breaks skins)
    #[serde(default)]
    pub blacklist: Vec<String>,
    /// Special blacklists for specific champions (e.g., Lux elementals)
    #[serde(default)]
    pub special_blacklists: HashMap<String, Vec<String>>,
}

/// Pre-computed character relationship lookups.
///
/// Built from [`ChampionList`] at startup. All lookups are case-insensitive
/// (keys stored lowercase).
#[derive(Debug, Clone, Default)]
pub struct CharacterRelations {
    /// champion (lowercase) → list of subchamps
    pub champion_to_subchamps: HashMap<String, Vec<String>>,
    /// subchamp (lowercase) → primary champion
    pub subchamp_to_champion: HashMap<String, String>,
    /// Every playable champion (lowercase).
    ///
    /// Membership is what separates a champion from a summoned form: a trap, an egg or
    /// a turret has its own character folder and its own skin BINs, so it is
    /// structurally indistinguishable from a champion without this list.
    pub champions: std::collections::HashSet<String>,
    /// entity name (lowercase) → healthbar value
    pub healthbar_values: HashMap<String, u8>,
    /// Global character blacklist (e.g., viegowraith)
    pub blacklist: Vec<String>,
    /// Special blacklists per champion (e.g., lux → elementals)
    pub special_blacklists: HashMap<String, Vec<String>>,
}

impl CharacterRelations {
    /// Build from a raw champion list, pre-computing reverse maps.
    pub fn from_champion_list(list: &ChampionList) -> Self {
        let mut relations = Self::default();

        for (champion, subchamps) in &list.subchamps {
            let champion_lower = champion.to_lowercase();
            relations
                .champion_to_subchamps
                .insert(champion_lower.clone(), subchamps.clone());
            for sub in subchamps {
                relations
                    .subchamp_to_champion
                    .insert(sub.to_lowercase(), champion.clone());
            }
        }

        relations.champions = list.champions.iter().map(|c| c.to_lowercase()).collect();

        for (name, value) in &list.healthbar_values {
            relations
                .healthbar_values
                .insert(name.to_lowercase(), *value);
        }

        // Normalize blacklists to lowercase
        relations.blacklist = list.blacklist.iter().map(|s| s.to_lowercase()).collect();

        for (champion, blacklist) in &list.special_blacklists {
            relations.special_blacklists.insert(
                champion.to_lowercase(),
                blacklist.iter().map(|s| s.to_lowercase()).collect(),
            );
        }

        relations
    }

    /// Get related subchamps for a champion (case-insensitive).
    /// Whether this character is a playable champion rather than a summoned form.
    pub fn is_champion(&self, character: &str) -> bool {
        self.champions.contains(&character.to_lowercase())
    }

    pub fn get_subchamps(&self, champion: &str) -> Option<&[String]> {
        self.champion_to_subchamps
            .get(&champion.to_lowercase())
            .map(|v| v.as_slice())
    }

    /// Get the primary champion for a subchamp (case-insensitive).
    pub fn get_primary_champion(&self, subchamp: &str) -> Option<&str> {
        self.subchamp_to_champion
            .get(&subchamp.to_lowercase())
            .map(|s| s.as_str())
    }

    /// Check if a character is globally blacklisted (case-insensitive).
    pub fn is_blacklisted(&self, character: &str) -> bool {
        let char_lower = character.to_lowercase();
        self.blacklist.contains(&char_lower)
    }

    /// Check if a character is in a champion's special blacklist (case-insensitive).
    pub fn is_in_special_blacklist(&self, champion: &str, character: &str) -> bool {
        let champ_lower = champion.to_lowercase();
        let char_lower = character.to_lowercase();
        self.special_blacklists
            .get(&champ_lower)
            .map(|list| list.contains(&char_lower))
            .unwrap_or(false)
    }

    /// Check if a character should be skipped (either global or special blacklist).
    pub fn should_skip_character(&self, champion: Option<&str>, character: &str) -> bool {
        if self.is_blacklisted(character) {
            return true;
        }
        if let Some(champ) = champion {
            if self.is_in_special_blacklist(champ, character) {
                return true;
            }
        }
        false
    }
}
