use super::*;
use hematite_file::hash_adapter::TxtHashProvider;

#[test]
fn packed_wad_and_fantome_binless_guards_preserve_input() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("Test.wad.client");
    let asset = "assets/characters/azir/skins/skin0/model.skn";
    let files = vec![(wad_path_hash(asset), asset.into(), vec![1, 2, 3])];
    let mut wad = std::io::Cursor::new(Vec::new());
    hematite_file::wad_builder::build_wad(&files, &[], &mut wad).unwrap();
    std::fs::write(&file, wad.get_ref()).unwrap();
    let config: FixConfig =
        toml::from_str(include_str!("../../../config/fix_config.toml")).unwrap();
    let champions = CharacterRelations::default();
    let mut repath = RepathOptions::new("test");
    repath.game_wad = Some(tmp.path().join("must-not-open.wad.client"));
    let hashes: Arc<dyn HashProvider> = Arc::new(TxtHashProvider::new());
    let ctx = ProcessContext {
        config: &config,
        selected_fixes: &[],
        champions: &champions,
        dry_run: false,
        check: false,
        repath_opts: Some(&repath),
        ui: crate::ui::UiReporter::new(crate::ui::Mode::Silent),
        live: None,
        restore_anm: false,
        game_wad: None,
        relocate_combo_bins: false,
        input_root: tmp.path().to_path_buf(),
        out_dir: tmp.path().join("output"),
    };
    let result = process_wad_file(&file, &ctx, &hashes).unwrap();
    assert_eq!(result.fixes_applied, 0);
    assert!(result.repath_reports[0].skip_reason.is_some());
    assert_eq!(std::fs::read(&file).unwrap(), *wad.get_ref());
    assert!(!ctx.out_dir.exists());

    let archive_path = tmp.path().join("test.fantome");
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&archive_path).unwrap());
    archive
        .start_file("WAD/Test.wad.client", zip::write::FileOptions::default())
        .unwrap();
    std::io::Write::write_all(&mut archive, wad.get_ref()).unwrap();
    archive
        .start_file(
            "WAD/Other.wad.client/assets/characters/azir/a.skn",
            zip::write::FileOptions::default(),
        )
        .unwrap();
    std::io::Write::write_all(&mut archive, &[1, 2, 3]).unwrap();
    archive.finish().unwrap();
    let before = std::fs::read(&archive_path).unwrap();
    let result = process_fantome_file(&archive_path, &ctx, &hashes).unwrap();
    assert_eq!(result.repath_reports.len(), 2);
    assert!(result
        .repath_reports
        .iter()
        .all(|r| r.skip_reason.is_some()));
    assert_eq!(std::fs::read(&archive_path).unwrap(), before);
    assert!(!ctx.out_dir.exists());
}
