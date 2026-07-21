//! GameProvider impl backed by hematite-live's GameIndex.
//! Interior mutability (Mutex) because core's GameProvider takes &self.

use hematite_core::traits::{BinProvider, GameProvider};
use hematite_live::GameIndex;
use hematite_types::bin::BinTree;
use std::sync::Mutex;

pub struct LiveGameProvider {
    index: Mutex<GameIndex>,
    bin: Box<dyn BinProvider>,
}

impl LiveGameProvider {
    pub fn new(index: GameIndex, bin: Box<dyn BinProvider>) -> Self {
        Self {
            index: Mutex::new(index),
            bin,
        }
    }

    /// Direct access for CLI-side machinery (deep repair, restore-anm,
    /// relocation) that wants hashes/pulls without trait indirection.
    pub fn with_index<R>(&self, f: impl FnOnce(&mut GameIndex) -> R) -> R {
        f(&mut self.index.lock().expect("GameIndex mutex poisoned"))
    }
}

impl GameProvider for LiveGameProvider {
    fn has_path(&self, path: &str) -> bool {
        self.index.lock().expect("poisoned").has_path(path)
    }
    fn pull_raw(&self, path: &str) -> Option<Vec<u8>> {
        self.index.lock().expect("poisoned").pull_path(path)
    }
    fn game_bin(&self, path: &str) -> Option<BinTree> {
        let bytes = self.pull_raw(path)?;
        match self.bin.parse_bytes(&bytes) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::debug!("game_bin parse failed for {}: {}", path, e);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_file::bin_adapter::FileBinProvider;
    use hematite_live::{wad_path_hash, LeagueInstall};

    /// Minimal fixture .wad.client writer, duplicated from
    /// hematite-live's (pub(crate)-only) test helper since it isn't
    /// exported across the crate boundary.
    fn write_fixture_wad(path: &std::path::Path, entries: &[(&str, &[u8])]) {
        let mut payloads: Vec<Vec<u8>> = Vec::new();
        for (_, data) in entries {
            payloads.push(zstd::stream::encode_all(&data[..], 3).unwrap());
        }
        let header_len = 2 + 2 + 256 + 8 + 4 + 32 * entries.len();
        let mut out = Vec::new();
        out.extend_from_slice(b"RW");
        out.push(3);
        out.push(4);
        out.extend_from_slice(&[0u8; 264]);
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        let mut offset = header_len as u32;
        for ((p, data), comp) in entries.iter().zip(&payloads) {
            out.extend_from_slice(&wad_path_hash(p).to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.push(3); // zstd
            out.push(0);
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u64.to_le_bytes());
            offset += comp.len() as u32;
        }
        for comp in &payloads {
            out.extend_from_slice(comp);
        }
        std::fs::write(path, out).unwrap();
    }

    fn fake_install_with_champion_wad(
        champion: &str,
        entries: &[(&str, &[u8])],
    ) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let champs_dir = dir.path().join("Game/DATA/FINAL/Champions");
        std::fs::create_dir_all(&champs_dir).unwrap();
        let wad = champs_dir.join(format!("{champion}.wad.client"));
        write_fixture_wad(&wad, entries);
        std::fs::write(dir.path().join("Game").join("League of Legends.exe"), b"").unwrap();
        dir
    }

    #[test]
    fn has_path_and_pull_raw_round_trip() {
        let dir = fake_install_with_champion_wad(
            "Yone",
            &[("data/characters/yone/skins/skin0.bin", b"PROPdata")],
        );
        let install = LeagueInstall::from_path(dir.path()).unwrap();
        let mut index = GameIndex::new(&install);
        assert!(index.add_champion("Yone"));

        let provider = LiveGameProvider::new(index, Box::new(FileBinProvider::new()));

        assert!(provider.has_path("data/characters/yone/skins/skin0.bin"));
        assert!(!provider.has_path("data/characters/yone/skins/skin1.bin"));

        let bytes = provider
            .pull_raw("data/characters/yone/skins/skin0.bin")
            .unwrap();
        assert_eq!(bytes, b"PROPdata");
    }

    #[test]
    fn game_bin_returns_none_on_garbage_bytes() {
        let dir = fake_install_with_champion_wad(
            "Yone",
            &[("data/characters/yone/skins/skin0.bin", b"not a real bin")],
        );
        let install = LeagueInstall::from_path(dir.path()).unwrap();
        let mut index = GameIndex::new(&install);
        assert!(index.add_champion("Yone"));

        let provider = LiveGameProvider::new(index, Box::new(FileBinProvider::new()));

        assert!(provider
            .game_bin("data/characters/yone/skins/skin0.bin")
            .is_none());
        // Missing path also returns None (short-circuits before parse).
        assert!(provider
            .game_bin("data/characters/yone/skins/skin9.bin")
            .is_none());
    }
}
