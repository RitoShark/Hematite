//! GameIndex — lazy, multi-WAD hash index with on-demand chunk pulls.

use crate::chunk::read_chunk;
use crate::detect::LeagueInstall;
use crate::error::LiveError;
use crate::toc::{read_toc, TocChunk};
use crate::wads::champion_wad;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use xxhash_rust::xxh64::xxh64;

/// League indexes WAD chunks by xxh64 of the lowercased, forward-slashed path.
pub fn wad_path_hash(path: &str) -> u64 {
    xxh64(path.to_lowercase().replace('\\', "/").as_bytes(), 0)
}

struct LoadedWad {
    path: PathBuf,
    chunks: Vec<TocChunk>,
    file: Option<File>, // opened lazily on first pull
}

pub struct GameIndex {
    game_dir: PathBuf,
    wads: Vec<LoadedWad>,
    /// hash → (wad idx, chunk idx). First WAD to define a hash wins.
    by_hash: HashMap<u64, (usize, usize)>,
    loaded_paths: HashSet<PathBuf>,
}

impl GameIndex {
    pub fn new(install: &LeagueInstall) -> Self {
        Self {
            game_dir: install.game_dir.clone(),
            wads: Vec::new(),
            by_hash: HashMap::new(),
            loaded_paths: HashSet::new(),
        }
    }

    pub fn game_dir(&self) -> &Path {
        &self.game_dir
    }

    /// Load a WAD's TOC into the index. Idempotent per path.
    pub fn add_wad(&mut self, path: &Path) -> Result<(), LiveError> {
        let canonical = path.to_path_buf();
        if !self.loaded_paths.insert(canonical.clone()) {
            return Ok(());
        }
        let toc = read_toc(path)?;
        let wad_idx = self.wads.len();
        for (i, c) in toc.chunks.iter().enumerate() {
            self.by_hash.entry(c.path_hash).or_insert((wad_idx, i));
        }
        tracing::debug!(
            "GameIndex: loaded {} chunks from {}",
            toc.chunks.len(),
            path.display()
        );
        self.wads.push(LoadedWad {
            path: canonical,
            chunks: toc.chunks,
            file: None,
        });
        Ok(())
    }

    /// Add the champion's base WAD, if it exists. Returns whether it was found.
    pub fn add_champion(&mut self, champion: &str) -> bool {
        match champion_wad(&self.game_dir, champion) {
            Some(p) => match self.add_wad(&p) {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!("GameIndex: failed to load {}: {}", p.display(), e);
                    false
                }
            },
            None => {
                tracing::debug!("GameIndex: no champion WAD for '{}'", champion);
                false
            }
        }
    }

    /// Add the always-shared `UI` and `Global` WADs. Returns how many loaded.
    ///
    /// Interface and HUD BINs do not live in champion WADs, and a mod that replaces one
    /// can ship it under any WAD name: a mod packaged as `Global.wad.client` can carry a
    /// BIN the game keeps in `UI.wad.client`. An index primed only with champion WADs
    /// therefore cannot see the vanilla copy, and such a mod reads as having no
    /// counterpart when it plainly has one.
    ///
    /// Only TOCs are read, so this costs two header reads and decompresses nothing.
    pub fn add_shared_wads(&mut self) -> usize {
        let final_dir = self.game_dir.join("DATA").join("FINAL");
        let mut loaded = 0;
        for name in ["UI.wad.client", "Global.wad.client"] {
            let path = final_dir.join(name);
            if !path.exists() {
                continue;
            }
            match self.add_wad(&path) {
                Ok(()) => loaded += 1,
                Err(e) => tracing::warn!("GameIndex: failed to load {}: {}", path.display(), e),
            }
        }
        loaded
    }

    /// Add every shipped map WAD. Returns how many loaded.
    ///
    /// Map geometry, scenery and the minion/turret/structure art all live under
    /// `Maps/Shipping`, in WADs named after the map rather than after any character. An
    /// index primed only from character names cannot see any of it, so a map mod's
    /// references all read as missing: on one fixture that was 87% of everything it named.
    ///
    /// Locale variants are skipped. They duplicate the base WAD's assets under the same
    /// hashes and only add TOC-reading work.
    ///
    /// Only TOCs are read, so this costs a header read per file and decompresses nothing.
    pub fn add_map_wads(&mut self) -> usize {
        let shipping = self.game_dir.join("DATA").join("FINAL").join("Maps").join("Shipping");
        let Ok(entries) = std::fs::read_dir(&shipping) else {
            return 0;
        };

        let mut loaded = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_lowercase();
            if !name.ends_with(".wad.client") || name.contains(".en_") {
                continue;
            }
            match self.add_wad(&path) {
                Ok(()) => loaded += 1,
                Err(e) => tracing::debug!("GameIndex: skipped {}: {}", path.display(), e),
            }
        }
        loaded
    }

    pub fn has_hash(&self, h: u64) -> bool {
        self.by_hash.contains_key(&h)
    }

    pub fn has_path(&self, p: &str) -> bool {
        self.has_hash(wad_path_hash(p))
    }

    /// Snapshot of every indexed hash (for suffix-strip resolution helpers).
    pub fn hash_set(&self) -> HashSet<u64> {
        self.by_hash.keys().copied().collect()
    }

    pub fn pull_hash(&mut self, h: u64) -> Option<Vec<u8>> {
        let &(wi, ci) = self.by_hash.get(&h)?;
        let wad = &mut self.wads[wi];
        if wad.file.is_none() {
            match File::open(&wad.path) {
                Ok(f) => wad.file = Some(f),
                Err(e) => {
                    tracing::warn!("GameIndex: open {} failed: {}", wad.path.display(), e);
                    return None;
                }
            }
        }
        let chunk = wad.chunks[ci];
        match read_chunk(wad.file.as_mut().unwrap(), &chunk) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                tracing::warn!("GameIndex: chunk {:016x} read failed: {}", h, e);
                None
            }
        }
    }

    pub fn pull_path(&mut self, p: &str) -> Option<Vec<u8>> {
        self.pull_hash(wad_path_hash(p))
    }
}

#[cfg(test)]
pub(crate) fn write_fixture_wad(path: &std::path::Path, entries: &[(&str, &[u8])]) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_pulls_by_path_and_hash() {
        let dir = tempfile::tempdir().unwrap();
        let champs = dir.path().join("Game/DATA/FINAL/Champions");
        std::fs::create_dir_all(&champs).unwrap();
        let wad = champs.join("Yone.wad.client");
        write_fixture_wad(
            &wad,
            &[("data/characters/yone/skins/skin0.bin", b"PROPdata")],
        );

        std::fs::write(dir.path().join("Game").join("League of Legends.exe"), b"").unwrap();
        let install = crate::detect::LeagueInstall::from_path(dir.path()).unwrap();
        let mut idx = GameIndex::new(&install);
        assert!(idx.add_champion("yone"));
        assert!(idx.has_path("data/characters/yone/skins/skin0.bin"));
        assert!(!idx.has_path("data/characters/yone/skins/skin1.bin"));
        assert_eq!(
            idx.pull_path("data/characters/yone/skins/skin0.bin")
                .unwrap(),
            b"PROPdata"
        );
        // add_champion for a champion with no WAD is a no-op returning false
        assert!(!idx.add_champion("nonexistent_champ"));
    }

    #[test]
    fn add_wad_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let wad = dir.path().join("Test.wad.client");
        write_fixture_wad(&wad, &[("data/foo.bin", b"hello")]);

        std::fs::create_dir_all(dir.path().join("Game")).unwrap();
        std::fs::write(dir.path().join("Game").join("League of Legends.exe"), b"").unwrap();
        let install = crate::detect::LeagueInstall::from_path(dir.path()).unwrap();
        let mut idx = GameIndex::new(&install);
        idx.add_wad(&wad).unwrap();
        idx.add_wad(&wad).unwrap();
        assert_eq!(idx.wads.len(), 1);
        assert_eq!(idx.hash_set().len(), 1);
    }
}
