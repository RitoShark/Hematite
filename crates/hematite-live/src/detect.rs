//! League of Legends install auto-detection.
//!
//! Ported from Flint's chain (`ltk_mod_core::league_path` + flint-ltk's
//! `league/detector.rs`), in priority order:
//!  1. RiotClientInstalls.json (ProgramData)
//!  2. Running League processes (sysinfo)
//!  3. Common install paths across drives
//!  4. Windows registry (reg query shell-out)
//!
//! All the Riot names/paths below are inherently hardcoded — they are Riot's
//! install layout, and there is nothing we can do about that.

use crate::error::LiveError;
use std::path::{Path, PathBuf};

const GAME_EXE: &str = "League of Legends.exe";
const GAME_DIR: &str = "Game";
const PROCESS_NAMES: &[&str] = &["LeagueClientUx.exe", "LeagueClient.exe", "League of Legends.exe"];
const FALLBACK_DRIVES: &[&str] = &["C:", "D:", "E:", "F:", "G:", "H:"];
const COMMON_SUBPATHS: &[&str] = &[
    "Riot Games\\League of Legends",
    "Program Files\\Riot Games\\League of Legends",
    "Program Files (x86)\\Riot Games\\League of Legends",
];
const REGISTRY_KEY: &str = r"HKLM\SOFTWARE\WOW6432Node\Riot Games, Inc\League of Legends";

#[derive(Debug, Clone)]
pub struct LeagueInstall {
    /// Install root (contains LeagueClient.exe and Game/).
    pub root: PathBuf,
    /// `<root>/Game` — where DATA/FINAL lives.
    pub game_dir: PathBuf,
    pub auto_detected: bool,
}

impl LeagueInstall {
    /// Accept either the install root or the Game/ dir itself; validate that
    /// `Game/League of Legends.exe` exists.
    pub fn from_path(p: &Path) -> Result<Self, LiveError> {
        // Case 1: p is the Game dir (contains the game exe directly).
        if p.join(GAME_EXE).is_file() {
            let root = p.parent().unwrap_or(p).to_path_buf();
            return Ok(Self { root, game_dir: p.to_path_buf(), auto_detected: false });
        }
        // Case 2: p is the install root.
        let game = p.join(GAME_DIR);
        if game.join(GAME_EXE).is_file() {
            return Ok(Self { root: p.to_path_buf(), game_dir: game, auto_detected: false });
        }
        Err(LiveError::InvalidInstall(p.to_path_buf()))
    }

    fn from_root_detected(root: PathBuf) -> Option<Self> {
        let game = root.join(GAME_DIR);
        if game.join(GAME_EXE).is_file() {
            Some(Self { root, game_dir: game, auto_detected: true })
        } else {
            None
        }
    }
}

/// Parse League install roots out of RiotClientInstalls.json content.
/// PBE installs are excluded (folder must be exactly "League of Legends").
pub(crate) fn league_paths_from_riot_installs_json(content: &str) -> Vec<PathBuf> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(content) else {
        return Vec::new();
    };
    let Some(map) = v.get("associated_client").and_then(|c| c.as_object()) else {
        return Vec::new();
    };
    map.keys()
        .filter(|k| {
            let trimmed = k.trim_end_matches(['/', '\\']);
            trimmed.ends_with("League of Legends")
        })
        .map(PathBuf::from)
        .collect()
}

fn detect_from_riot_client_installs() -> Option<LeagueInstall> {
    let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let json_path = PathBuf::from(format!(
        "{}\\ProgramData\\Riot Games\\RiotClientInstalls.json",
        system_drive
    ));
    let content = std::fs::read_to_string(json_path).ok()?;
    league_paths_from_riot_installs_json(&content)
        .into_iter()
        .find_map(LeagueInstall::from_root_detected)
}

fn detect_from_running_process() -> Option<LeagueInstall> {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_processes();
    for wanted in PROCESS_NAMES {
        for proc_ in sys.processes().values() {
            if !proc_.name().eq_ignore_ascii_case(wanted) {
                continue;
            }
            let Some(exe) = proc_.exe() else { continue };
            let Some(dir) = exe.parent() else { continue };
            // Game exe: parent is Game/, root is one above.
            if wanted.eq_ignore_ascii_case(GAME_EXE) {
                if let Some(root) = dir.parent() {
                    if let Some(li) = LeagueInstall::from_root_detected(root.to_path_buf()) {
                        return Some(li);
                    }
                }
            } else if let Some(li) = LeagueInstall::from_root_detected(dir.to_path_buf()) {
                // Client exes live in the install root.
                return Some(li);
            }
        }
    }
    None
}

fn detect_from_common_paths() -> Option<LeagueInstall> {
    for drive in FALLBACK_DRIVES {
        for sub in COMMON_SUBPATHS {
            let root = PathBuf::from(format!("{}\\{}", drive, sub));
            if let Some(li) = LeagueInstall::from_root_detected(root) {
                return Some(li);
            }
        }
    }
    None
}

fn detect_from_registry() -> Option<LeagueInstall> {
    let out = std::process::Command::new("reg")
        .args(["query", REGISTRY_KEY, "/v", "Location"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Line shape: "    Location    REG_SZ    C:\Riot Games\League of Legends"
    let loc = text.lines().find_map(|l| {
        let l = l.trim();
        l.contains("REG_SZ").then(|| l.split("REG_SZ").nth(1).map(str::trim))?
    })?;
    LeagueInstall::from_root_detected(PathBuf::from(loc))
}

/// Try all detection mechanisms in priority order.
pub fn detect_league() -> Option<LeagueInstall> {
    detect_from_riot_client_installs()
        .or_else(detect_from_running_process)
        .or_else(detect_from_common_paths)
        .or_else(detect_from_registry)
        .inspect(|li| tracing::info!("Detected League install at {}", li.root.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_install(root: &std::path::Path) {
        let game = root.join("Game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("League of Legends.exe"), b"").unwrap();
    }

    #[test]
    fn from_path_accepts_install_root() {
        let dir = tempfile::tempdir().unwrap();
        fake_install(dir.path());
        let li = LeagueInstall::from_path(dir.path()).unwrap();
        assert_eq!(li.game_dir, dir.path().join("Game"));
        assert!(!li.auto_detected);
    }

    #[test]
    fn from_path_accepts_game_dir_directly() {
        let dir = tempfile::tempdir().unwrap();
        fake_install(dir.path());
        let li = LeagueInstall::from_path(&dir.path().join("Game")).unwrap();
        assert_eq!(li.game_dir, dir.path().join("Game"));
    }

    #[test]
    fn from_path_rejects_random_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(LeagueInstall::from_path(dir.path()).is_err());
    }

    #[test]
    fn riot_installs_json_parses_associated_client() {
        let json = r#"{
            "associated_client": {
                "C:/Riot Games/League of Legends/": "C:/Riot Games/Riot Client/x.exe",
                "C:/Riot Games/League of Legends (PBE)/": "C:/Riot Games/Riot Client/x.exe"
            }
        }"#;
        let paths = league_paths_from_riot_installs_json(json);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with("League of Legends") || paths[0].ends_with("League of Legends/"));
    }
}
