//! GameProvider impl backed by hematite-live's GameIndex.
//! Interior mutability (Mutex) because core's GameProvider takes &self.

use hematite_core::traits::{BinProvider, GameProvider};
use hematite_live::GameIndex;
use hematite_types::bin::BinTree;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

pub struct LiveGameProvider {
    index: Mutex<GameIndex>,
    bin: Box<dyn BinProvider>,
    /// Lazily built set of shader entry keys this install ships.
    ///
    /// Built at most once per provider: it means decompressing the shader WAD, and the
    /// contents only change when the game patches. The inner `Option` keeps "could not
    /// read" distinct from "no shaders", which callers must never conflate.
    shader_defs: OnceLock<Option<Arc<HashSet<u32>>>>,
    /// Parsed game BINs, keyed by path.
    ///
    /// Resolving a dead link walks up to 64 game BINs, and that walk repeats for every
    /// BIN in the mod: on one fixture, 49 mod BINs against the same champion meant the
    /// same handful of game files were decompressed and parsed hundreds of times. The
    /// answer depends only on the install, which does not change while the provider
    /// lives, so it is memoised here rather than by any individual caller.
    ///
    /// `None` is cached too: a path the game does not ship is just as worth remembering,
    /// and re-asking costs a WAD lookup every time.
    bin_cache: Mutex<std::collections::HashMap<String, Option<Arc<BinTree>>>>,
}

impl LiveGameProvider {
    pub fn new(index: GameIndex, bin: Box<dyn BinProvider>) -> Self {
        Self {
            index: Mutex::new(index),
            bin,
            shader_defs: OnceLock::new(),
            bin_cache: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Collect every shader definition's entry key from the install's shader WAD.
    ///
    /// A material's `shader` property links to one of these keys, so this is exactly the
    /// set of links that resolve at load time. Derived from the installed game rather
    /// than from a shipped list, because the valid set changes every patch: a stale list
    /// invents crashes that do not exist, and an absent one silently disables the check.
    fn build_shader_defs(&self) -> Option<Arc<HashSet<u32>>> {
        let _t = hematite_core::timing::span("shader defs (build)");
        let mut idx = self.index.lock().expect("poisoned");
        let wad_path = idx
            .game_dir()
            .join("DATA")
            .join("FINAL")
            .join("Shaders")
            .join("Shaders.wad.client");

        if !wad_path.exists() {
            tracing::warn!(
                "shader validation unavailable: no shader WAD at {}",
                wad_path.display()
            );
            return None;
        }

        let toc = match hematite_live::read_toc(&wad_path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("shader validation unavailable: {} ({e})", wad_path.display());
                return None;
            }
        };
        if let Err(e) = idx.add_wad(&wad_path) {
            tracing::warn!("shader validation unavailable: {} ({e})", wad_path.display());
            return None;
        }

        let mut out = HashSet::new();
        for chunk in &toc.chunks {
            let Some(bytes) = idx.pull_hash(chunk.path_hash) else {
                continue;
            };
            // Only PROP BINs hold shader definitions. The WAD is mostly compiled shader
            // blobs per graphics backend, and parsing those would be wasted work.
            if bytes.len() < 4 || &bytes[0..4] != b"PROP" {
                continue;
            }
            if let Ok(tree) = self.bin.parse_bytes(&bytes) {
                out.extend(tree.objects.values().map(|o| o.path_hash.0));
            }
        }

        if out.is_empty() {
            tracing::warn!("shader validation unavailable: shader WAD yielded no definitions");
            return None;
        }
        tracing::info!("shader validation: {} definitions read from the install", out.len());
        Some(Arc::new(out))
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
    fn game_bin(&self, path: &str) -> Option<Arc<BinTree>> {
        if let Ok(cache) = self.bin_cache.lock() {
            if let Some(hit) = cache.get(path) {
                return hit.clone();
            }
        }

        let parsed = self
            .pull_raw(path)
            .and_then(|bytes| match self.bin.parse_bytes(&bytes) {
                Ok(tree) => Some(Arc::new(tree)),
                Err(e) => {
                    tracing::debug!("game_bin parse failed for {}: {}", path, e);
                    None
                }
            });

        if let Ok(mut cache) = self.bin_cache.lock() {
            cache.insert(path.to_string(), parsed.clone());
        }
        parsed
    }
    fn shader_defs(&self) -> Option<Arc<HashSet<u32>>> {
        self.shader_defs
            .get_or_init(|| self.build_shader_defs())
            .clone()
    }
    fn wads_touched(&self, hashes: &HashSet<u64>) -> Option<usize> {
        // Every archive, not the primed subset: a shared-asset collision reaches archives
        // no champion name would ever point at, which is the whole signal.
        Some(
            self.index
                .lock()
                .expect("poisoned")
                .count_wads_containing(hashes),
        )
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
