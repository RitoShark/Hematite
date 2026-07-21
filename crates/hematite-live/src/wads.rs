//! Base-game WAD enumeration under <Game>/DATA/FINAL.

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const DATA_FINAL: &str = "DATA/FINAL";
const CHAMPIONS_DIR: &str = "Champions";
const MAX_DEPTH: usize = 5;

#[derive(Debug, Clone)]
pub struct GameWadInfo {
    pub path: PathBuf,
    pub name: String,
    pub category: String,
}

fn is_wad_name(name: &str) -> bool {
    let n = name.to_lowercase();
    n.ends_with(".wad.client") || n.ends_with(".wad")
}

pub fn enumerate_wads(game_dir: &Path) -> Vec<GameWadInfo> {
    let root = game_dir.join(DATA_FINAL);
    let mut out = Vec::new();
    for entry in WalkDir::new(&root)
        .max_depth(MAX_DEPTH)
        .into_iter()
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_wad_name(&name) {
            continue;
        }
        let category = entry
            .path()
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        out.push(GameWadInfo {
            path: entry.path().to_path_buf(),
            name,
            category,
        });
    }
    out
}

/// Find `<game_dir>/DATA/FINAL/Champions/<Champion>.wad.client`, matching the
/// champion name case-insensitively against the real directory listing.
pub fn champion_wad(game_dir: &Path, champion: &str) -> Option<PathBuf> {
    let dir = game_dir.join(DATA_FINAL).join(CHAMPIONS_DIR);
    let wanted = format!("{}.wad.client", champion.to_lowercase());
    let entries = std::fs::read_dir(&dir).ok()?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.to_lowercase() == wanted {
            return Some(e.path());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_game_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let champs = dir.path().join("DATA/FINAL/Champions");
        let maps = dir.path().join("DATA/FINAL/Maps/Shipping");
        std::fs::create_dir_all(&champs).unwrap();
        std::fs::create_dir_all(&maps).unwrap();
        std::fs::write(champs.join("Aatrox.wad.client"), b"").unwrap();
        std::fs::write(maps.join("Map11.wad.client"), b"").unwrap();
        dir
    }

    #[test]
    fn enumerate_wads_finds_all_wads_with_categories() {
        let dir = setup_game_dir();
        let mut wads = enumerate_wads(dir.path());
        wads.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(wads.len(), 2);
        assert_eq!(wads[0].name, "Aatrox.wad.client");
        assert_eq!(wads[0].category, "Champions");
        assert_eq!(wads[1].name, "Map11.wad.client");
        assert_eq!(wads[1].category, "Shipping");
    }

    #[test]
    fn champion_wad_matches_case_insensitively() {
        let dir = setup_game_dir();
        let found = champion_wad(dir.path(), "aatrox").unwrap();
        assert_eq!(
            found,
            dir.path().join("DATA/FINAL/Champions/Aatrox.wad.client")
        );
    }

    #[test]
    fn champion_wad_returns_none_for_missing_champion() {
        let dir = setup_game_dir();
        assert!(champion_wad(dir.path(), "nonexistent").is_none());
    }
}
