//! Detect-only mode must report fired fixes without touching disk.
//!
//! Builds a synthetic single-BIN `.wad.client` folder that triggers exactly
//! one BIN-level fix (`champion_bin_remover`, whose `entry_type_exists_any`
//! detection fires on an object whose class resolves to `SpellObject`),
//! snapshots every file's bytes, runs `fix_folder` with `detect_only: true`,
//! and asserts:
//!   1. `ProcessResult.applied_fixes` is non-empty (the fix fired), and
//!   2. the folder's file bytes are byte-identical afterwards.
//!
//! Asset-free: no game WADs, no LMDB. A tiny in-test `HashProvider` resolves
//! the one synthetic class hash to `SpellObject`; the BIN itself is produced
//! by `hematite_file`'s real rs_bin serializer so the pipeline parses it for
//! real.

use hematite_core::traits::{BinProvider, HashProvider};
use hematite_file::bin_adapter::FileBinProvider;
use hematite_orchestrate::{fix_folder, FixOptions, NoopSink};
use hematite_types::champion::CharacterRelations;
use hematite_types::config::FixConfig;
use hematite_types::hash::{FieldHash, GameHash, PathHash, TypeHash};
use indexmap::IndexMap;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Class hash carried by the synthetic BIN object; the stub provider below
/// resolves it to `SpellObject` so `champion_bin_remover` detects it.
const SPELL_OBJECT_CLASS_HASH: u32 = 0xDEAD_BEEF;

/// Minimal `HashProvider` that reports itself loaded and resolves exactly one
/// class hash to `SpellObject`. Everything else is unknown.
struct StubHashes;

impl HashProvider for StubHashes {
    fn resolve_type(&self, hash: TypeHash) -> Option<String> {
        if hash.0 == SPELL_OBJECT_CLASS_HASH {
            Some("SpellObject".to_string())
        } else {
            None
        }
    }
    fn resolve_field(&self, _hash: FieldHash) -> Option<String> {
        None
    }
    fn resolve_entry(&self, _hash: PathHash) -> Option<String> {
        None
    }
    fn resolve_game_path(&self, _hash: GameHash) -> Option<String> {
        None
    }
    fn type_hash(&self, name: &str) -> Option<TypeHash> {
        if name.eq_ignore_ascii_case("SpellObject") {
            Some(TypeHash(SPELL_OBJECT_CLASS_HASH))
        } else {
            None
        }
    }
    fn field_hash(&self, _name: &str) -> Option<FieldHash> {
        None
    }
    fn has_game_path(&self, _path: &str) -> bool {
        false
    }
    fn is_loaded(&self) -> bool {
        true
    }
}

/// Build a real BIN (via rs_bin) containing one object whose class resolves to
/// `SpellObject`, so `champion_bin_remover` fires on it.
fn spell_object_bin() -> Vec<u8> {
    use hematite_types::bin::{BinObject, BinTree};

    let obj = BinObject {
        path_hash: PathHash(0x1234_5678),
        class_hash: TypeHash(SPELL_OBJECT_CLASS_HASH),
        properties: IndexMap::new(),
    };
    let mut objects = IndexMap::new();
    objects.insert(0x1234_5678u32, obj);
    let tree = BinTree {
        objects,
        ..Default::default()
    };

    let provider = FileBinProvider::new();
    provider
        .write_bytes(&tree)
        .expect("serialize synthetic BIN")
}

/// Snapshot every file under `root` as path → bytes.
fn snapshot(root: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    let mut map = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.unwrap();
        if entry.path().is_file() {
            let rel = entry
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            map.insert(rel, std::fs::read(entry.path()).unwrap());
        }
    }
    map
}

#[test]
fn detect_only_reports_fixes_and_writes_nothing() {
    // Load the repo's embedded config so the real `champion_bin_remover` rule
    // drives detection.
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/fix_config.toml"
    ))
    .unwrap();
    let config: FixConfig = toml::from_str(&raw).unwrap();

    // Build a synthetic .wad.client folder containing one triggering BIN.
    let tmp = tempfile::tempdir().unwrap();
    let wad_folder = tmp.path().join("Test.wad.client");
    let bin_path = wad_folder.join("data/characters/test/skins/skin0.bin");
    std::fs::create_dir_all(bin_path.parent().unwrap()).unwrap();
    std::fs::write(&bin_path, spell_object_bin()).unwrap();

    // Sanity: the BIN we wrote is parseable and carries our object.
    let parsed = FileBinProvider::new()
        .parse_bytes(&std::fs::read(&bin_path).unwrap())
        .unwrap();
    assert_eq!(parsed.objects.len(), 1);

    let before = snapshot(&wad_folder);

    let champions = CharacterRelations::default();
    let hash_provider: Arc<dyn HashProvider> = Arc::new(StubHashes);
    let selected = vec!["champion_bin_remover".to_string()];
    let opts = FixOptions {
        dry_run: false,
        detect_only: true,
        repath: None,
        restore_anm: false,
        relocate_combo_bins: false,
        game_wad: None,
        live: None,
        in_place: false,
    };

    let result = fix_folder(
        &wad_folder,
        &config,
        &selected,
        &champions,
        &hash_provider,
        &opts,
        &NoopSink,
    )
    .expect("fix_folder detect-only run");

    // 1. The fix fired and is recorded.
    assert!(
        !result.applied_fixes.is_empty(),
        "detect_only must record the fired fix in applied_fixes"
    );
    assert!(
        result
            .applied_fixes
            .iter()
            .any(|f| f.fix_id == "champion_bin_remover"),
        "champion_bin_remover should be among the detected fixes; got {:?}",
        result
            .applied_fixes
            .iter()
            .map(|f| &f.fix_id)
            .collect::<Vec<_>>()
    );
    // CheckInfo is populated in detect-only mode (mirrors --check).
    assert!(
        result.check_info.is_some(),
        "detect_only must populate check_info"
    );

    // 2. Nothing on disk changed: same files, same bytes, and no
    //    `.fixed.wad.client` sibling was produced.
    let after = snapshot(&wad_folder);
    assert_eq!(before, after, "detect_only must not modify any file bytes");
    assert!(
        !tmp.path().join("Test.fixed.wad.client").exists(),
        "detect_only must not write a fixed output folder"
    );
}

/// A real `in_place` run writes back INTO the source folder and produces NO
/// `.fixed.wad.client` sibling (the copy the CLI makes). Uses
/// `champion_bin_remover`, which removes the skin BIN.
#[test]
fn in_place_run_writes_to_source_no_fixed_copy() {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/fix_config.toml"
    ))
    .unwrap();
    let config: FixConfig = toml::from_str(&raw).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let wad_folder = tmp.path().join("Test.wad.client");
    let bin_rel = "data/characters/test/skins/skin0.bin";
    let bin_path = wad_folder.join(bin_rel);
    std::fs::create_dir_all(bin_path.parent().unwrap()).unwrap();
    std::fs::write(&bin_path, spell_object_bin()).unwrap();
    // A second, untouched file that must survive in place.
    let keep_path = wad_folder.join("data/keep.txt");
    std::fs::write(&keep_path, b"keep me").unwrap();

    let champions = CharacterRelations::default();
    let hash_provider: Arc<dyn HashProvider> = Arc::new(StubHashes);
    let selected = vec!["champion_bin_remover".to_string()];
    let opts = FixOptions {
        dry_run: false,
        detect_only: false,
        repath: None,
        restore_anm: false,
        relocate_combo_bins: false,
        game_wad: None,
        live: None,
        in_place: true,
    };

    let result = fix_folder(
        &wad_folder,
        &config,
        &selected,
        &champions,
        &hash_provider,
        &opts,
        &NoopSink,
    )
    .expect("fix_folder in-place run");

    assert!(result.fixes_applied > 0, "the run should apply the fix");
    // No sibling copy — we wrote in place.
    assert!(
        !tmp.path().join("Test.fixed.wad.client").exists(),
        "in_place must NOT produce a .fixed.wad.client sibling"
    );
    // The source folder still exists and the untouched file survived.
    assert!(wad_folder.is_dir(), "source folder must remain");
    assert_eq!(std::fs::read(&keep_path).unwrap(), b"keep me");
}
