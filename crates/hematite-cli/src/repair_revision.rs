use super::*;
use hematite_types::result::{RepairRecord, ValidationReport};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;

const RECORD: &str = "hematite-repair.json";
const MAX_FILE: u64 = 512 * 1024 * 1024;
const MAX_TOTAL: usize = 2 * 1024 * 1024 * 1024;

type Files = Vec<(u64, String, Vec<u8>)>;

pub(super) fn process(
    input: &Path,
    ctx: &ProcessContext<'_>,
    hashes: &Arc<dyn HashProvider>,
) -> Result<ProcessResult> {
    if ctx.dry_run || ctx.check {
        let mut readonly = ctx.clone();
        readonly.dry_run = true;
        return dispatch(input, &readonly, hashes);
    }
    let name = input.file_name().context("Input has no filename")?;
    let base = ctx
        .output_for(input)
        .context("Input is outside the input root")?;
    let revisions = base.with_file_name(format!("{}.hematite", name.to_string_lossy()));
    fs::create_dir_all(&revisions)?;
    let staging = tempfile::tempdir_in(&revisions)?;
    let mut staged_ctx = ctx.clone();
    staged_ctx.input_root = input.parent().unwrap_or(Path::new(".")).to_path_buf();
    staged_ctx.out_dir = staging.path().to_path_buf();
    let input_sha256 = fingerprint(input)?;
    let parent = input
        .parent()
        .and_then(|dir| {
            let mut bytes = Vec::new();
            File::open(dir.join(RECORD))
                .ok()?
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            Some(bytes)
        })
        .filter(|bytes| bytes.len() <= 4 * 1024 * 1024)
        .and_then(|bytes| serde_json::from_slice::<RepairRecord>(&bytes).ok())
        .filter(|record| record.format_version == 1 && record.output_sha256 == input_sha256);

    let already_compressed = parent.as_ref().is_some_and(|record| {
        record
            .options
            .get("audio_previously_compressed")
            .and_then(|v| v.as_bool())
            == Some(true)
            || record
                .applied_fixes
                .iter()
                .any(|fix| fix.fix_id == "compress_audio" || fix.fix_id == "compress_audio_wpk")
    });
    let effective_fixes: Vec<String> = ctx
        .selected_fixes
        .iter()
        .filter(|id| {
            !already_compressed
                || (id.as_str() != "compress_audio" && id.as_str() != "compress_audio_wpk")
        })
        .cloned()
        .collect();
    staged_ctx.selected_fixes = &effective_fixes;
    let mut result = dispatch(input, &staged_ctx, hashes)?;
    anyhow::ensure!(
        fingerprint(input)? == input_sha256,
        "Input changed during repair"
    );
    let artifact = staging.path().join(name);
    if !artifact.exists() {
        copy_input(input, &artifact)?;
    }
    let mut validation = inspect(&artifact, ctx, hashes);
    if result.fixes_failed > 0 || !result.errors.is_empty() {
        validation.issues.push(format!(
            "Repair reported {} failed fixes and {} errors",
            result.fixes_failed,
            result.errors.len()
        ));
    }
    if ctx.game_wad.is_some() {
        validation.limitations.push(
            "Explicit game-WAD target build is unknown; install metadata is not used to label it"
                .into(),
        );
    }
    let first_revision = parent
        .as_ref()
        .map(|record| record.revision.saturating_add(1))
        .unwrap_or(1);
    let (revision, destination) = reserve(&revisions, first_revision)?;
    let output = destination.join(name);
    let mut rules: serde_json::Value = serde_json::to_value(ctx.config)?;
    canonicalize(&mut rules);
    let record = RepairRecord {
        format_version: 1,
        revision,
        parent_revision: parent.as_ref().map(|record| record.revision),
        input_sha256,
        output_sha256: fingerprint(&artifact)?,
        output: output.to_string_lossy().into_owned(),
        target_game_build: if ctx.game_wad.is_none() {
            target_build(ctx.live)
        } else {
            None
        },
        source_game_build: parent
            .as_ref()
            .and_then(|record| record.target_game_build.clone()),
        hematite_version: env!("CARGO_PKG_VERSION").into(),
        rules_version: ctx.config.version.clone(),
        rules_sha256: format!("{:x}", Sha256::digest(serde_json::to_vec(&rules)?)),
        selected_fixes: effective_fixes,
        options: serde_json::json!({"restore_anm": ctx.restore_anm, "relocate_combo_bins": ctx.relocate_combo_bins,
            "repath": ctx.repath_opts.map(|options| serde_json::json!({
                "prefix": options.prefix, "layout": options.layout, "invis_texture": options.invis_texture,
                "skip_vo": options.skip_vo, "placeholder_rules": options.placeholder_rules,
            })),
            "explicit_game_wad": ctx.game_wad.is_some(), "audio_previously_compressed": already_compressed}),
        applied_fixes: result.applied_fixes.clone(),
        errors: result.errors.clone(),
        validation_status: validation.status().into(),
        validation,
        in_game_tested: false,
    };
    fs::write(
        staging.path().join(RECORD),
        serde_json::to_vec_pretty(&record)?,
    )?;
    fs::rename(&artifact, &output)?;
    fs::rename(staging.path().join(RECORD), destination.join(RECORD))?;
    ctx.ui.note(&format!(
        "Repair r{revision:03}: {} ({})",
        output.display(),
        record.validation_status
    ));
    if !record.validation.issues.is_empty() {
        result.errors.push(format!(
            "Output r{revision:03} has {} validation issue(s); see {}",
            record.validation.issues.len(),
            destination.join(RECORD).display()
        ));
    }
    result.repairs.push(record);
    Ok(result)
}

fn canonicalize(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            object.sort_keys();
            for value in object.values_mut() {
                canonicalize(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                canonicalize(value);
            }
        }
        _ => {}
    }
}

fn dispatch(
    input: &Path,
    ctx: &ProcessContext<'_>,
    hashes: &Arc<dyn HashProvider>,
) -> Result<ProcessResult> {
    if is_wad_folder(input) {
        process_wad_folder(input, ctx, hashes)
    } else {
        process_file_with_hashes(input, ctx, hashes)
    }
}

fn reserve(root: &Path, first: u32) -> Result<(u32, PathBuf)> {
    for revision in first..=u32::MAX {
        let path = root.join(format!("r{revision:03}"));
        match fs::create_dir(&path) {
            Ok(()) => return Ok((revision, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("No repair revision is available")
}

fn disk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        anyhow::ensure!(
            !entry.file_type().is_symlink(),
            "Symbolic links are not repair inputs"
        );
        if entry.file_type().is_file() {
            paths.push(entry.path().to_path_buf());
        }
    }
    paths.sort();
    Ok(paths)
}

fn fingerprint(path: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    for file in disk_files(path)? {
        if path.is_dir() {
            let relative = file
                .strip_prefix(path)?
                .to_string_lossy()
                .replace('\\', "/");
            digest.update((relative.len() as u64).to_le_bytes());
            digest.update(relative.as_bytes());
            digest.update(fs::metadata(&file)?.len().to_le_bytes());
        }
        let mut reader = File::open(file)?;
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
    }
    Ok(format!("{digest:x}", digest = digest.finalize()))
}

fn copy_input(input: &Path, output: &Path) -> Result<()> {
    if input.is_dir() {
        fs::create_dir_all(output)?;
    }
    for file in disk_files(input)? {
        let target = if input.is_dir() {
            output.join(file.strip_prefix(input)?)
        } else {
            output.to_path_buf()
        };
        ensure_parent(&target)?;
        fs::copy(file, target)?;
    }
    Ok(())
}

fn target_build(live: Option<&LiveGameProvider>) -> Option<String> {
    let game = live?.with_index(|index| index.game_dir().to_path_buf());
    let bytes = fs::read(game.join("content-metadata.json")).ok()?;
    if bytes.len() > 1024 * 1024 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let build = value.get("version")?.as_str()?;
    let parts: Vec<_> = build.split('.').collect();
    (parts.len() >= 3
        && parts.len() <= 4
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())))
    .then(|| build.to_owned())
}

fn inspect(
    path: &Path,
    ctx: &ProcessContext<'_>,
    hashes: &Arc<dyn HashProvider>,
) -> ValidationReport {
    let mut limitations = Vec::new();
    let mut files = Vec::new();
    if let Err(error) = load_artifact(path, hashes, &mut files, &mut limitations) {
        return ValidationReport {
            issues: vec![format!("Cannot read repaired output: {error:#}")],
            ..Default::default()
        };
    }
    let (game_hashes, complete) = ctx
        .live
        .map(|live| {
            live.with_index(|index| {
                let root = index.game_dir().join("DATA/FINAL");
                let mut complete = root.is_dir();
                let mut found = 0;
                for entry in WalkDir::new(root) {
                    match entry {
                        Ok(entry)
                            if entry.file_type().is_file()
                                && entry
                                    .file_name()
                                    .to_string_lossy()
                                    .to_lowercase()
                                    .ends_with(".wad.client") =>
                        {
                            found += 1;
                            if index.add_wad(entry.path()).is_err() {
                                complete = false;
                            }
                        }
                        Err(_) => complete = false,
                        _ => {}
                    }
                }
                (index.hash_set(), complete && found > 0)
            })
        })
        .unwrap_or_default();
    let mut report = hematite_file::validation::validate_files(
        &files,
        &|hash| game_hashes.contains(&hash),
        complete,
    );
    report.limitations.extend(limitations);
    report
}

fn push(files: &mut Files, hash: u64, path: String, bytes: Vec<u8>) -> Result<()> {
    anyhow::ensure!(
        bytes.len() <= MAX_FILE as usize,
        "Validation file exceeds size limit"
    );
    let total: usize = files.iter().map(|(_, _, bytes)| bytes.len()).sum();
    anyhow::ensure!(
        bytes.len() <= MAX_TOTAL.saturating_sub(total),
        "Validation exceeds size limit"
    );
    files.push((hash, path, bytes));
    Ok(())
}

fn read_limited(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_FILE,
        "Validation file exceeds size limit"
    );
    Ok(bytes)
}

fn path_hash(path: &str) -> u64 {
    hematite_file::wad_folder::hex_chunk_hash(path).unwrap_or_else(|| wad_path_hash(path))
}

fn load_artifact(
    path: &Path,
    hashes: &Arc<dyn HashProvider>,
    files: &mut Files,
    limitations: &mut Vec<String>,
) -> Result<()> {
    if path.is_dir() {
        for file in disk_files(path)? {
            let relative = file
                .strip_prefix(path)?
                .to_string_lossy()
                .replace('\\', "/");
            push(
                files,
                path_hash(&relative),
                relative,
                read_limited(File::open(file)?)?,
            )?;
        }
    } else if path
        .to_string_lossy()
        .to_lowercase()
        .ends_with(".wad.client")
    {
        let mut wad = hematite_file::wad_adapter::WadFile::open(path)?;
        for hash in wad.chunk_hash_set() {
            let bytes = wad
                .extract_chunk_by_hash(hash)?
                .context("WAD chunk could not be read")?;
            let name = hashes
                .resolve_game_path(hematite_types::hash::GameHash(hash))
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{hash:016x}"));
            push(files, hash, name, bytes)?;
        }
    } else if path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("modpkg"))
    {
        let mut package = ltk_modpkg::Modpkg::mount_from_reader(File::open(path)?)?;
        if package.layer_indices.len() > 1 {
            limitations.push("Layer combinations have not been simulated".into());
        }
        let chunks: Vec<_> = package
            .chunks
            .iter()
            .map(|(key, chunk)| (*key, chunk.path_hash, chunk.path_index, chunk.layer_index))
            .collect();
        for (key, hash, index, layer) in chunks {
            if layer == u32::MAX {
                continue;
            }
            let name = package
                .chunk_path_indices
                .get(index as usize)
                .and_then(|key| package.chunk_paths.get(key))
                .cloned()
                .unwrap_or_else(|| format!("{hash:016x}"));
            let bytes = package.load_chunk_decompressed_by_hash(key.0, key.1)?;
            push(files, hash, name, bytes.to_vec())?;
        }
    } else if path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("bin"))
    {
        push(
            files,
            0,
            path.file_name().unwrap().to_string_lossy().into_owned(),
            read_limited(File::open(path)?)?,
        )?;
        limitations.push("Standalone BIN: no mod asset inventory".into());
    } else {
        let mut archive = zip::ZipArchive::new(File::open(path)?)?;
        for index in 0..archive.len() {
            let entry = archive.by_index(index)?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().replace('\\', "/");
            if name.to_lowercase().starts_with("meta/") {
                continue;
            }
            let bytes = read_limited(entry)?;
            if name.to_lowercase().ends_with(".wad.client") {
                let temporary = tempfile::tempdir()?;
                let packed = temporary.path().join("content.wad.client");
                fs::write(&packed, bytes)?;
                load_artifact(&packed, hashes, files, limitations)?;
            } else {
                let relative = name
                    .to_lowercase()
                    .find(".wad.client/")
                    .map(|index| name[index + 12..].to_owned())
                    .unwrap_or(name);
                push(files, path_hash(&relative), relative, bytes)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisions_preserve_original_and_prior_outputs_and_verify_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("skin.bin");
        let bytes = FileBinProvider
            .write_bytes(&hematite_types::bin::BinTree::default())
            .unwrap();
        fs::write(&input, &bytes).unwrap();
        let config: FixConfig =
            toml::from_str(include_str!("../../../config/fix_config.toml")).unwrap();
        let champions = CharacterRelations::default();
        let hashes: Arc<dyn HashProvider> = Arc::new(TxtHashProvider::new());
        let mut ctx = ProcessContext {
            config: &config,
            selected_fixes: &[],
            champions: &champions,
            dry_run: false,
            check: false,
            repath_opts: None,
            ui: crate::ui::UiReporter::new(crate::ui::Mode::Silent),
            live: None,
            restore_anm: false,
            game_wad: None,
            relocate_combo_bins: false,
            input_root: tmp.path().to_path_buf(),
            out_dir: tmp.path().join("output"),
        };
        ctx.check = true;
        assert!(process(&input, &ctx, &hashes).unwrap().repairs.is_empty());
        assert!(!ctx.out_dir.exists());
        ctx.check = false;
        let first = process(&input, &ctx, &hashes).unwrap().repairs.remove(0);
        let second = process(&input, &ctx, &hashes).unwrap().repairs.remove(0);
        assert_eq!((first.revision, second.revision), (1, 2));
        assert_eq!(fs::read(&input).unwrap(), bytes);
        assert_eq!(fs::read(&first.output).unwrap(), bytes);
        assert_eq!(fs::read(&second.output).unwrap(), bytes);
        assert_eq!(first.target_game_build, None);
        assert_eq!(first.validation_status, "incomplete");
        assert!(!first.in_game_tested);
        let child = process(Path::new(&first.output), &ctx, &hashes)
            .unwrap()
            .repairs
            .remove(0);
        assert_eq!(child.parent_revision, Some(1));
        assert_eq!(child.revision, 2);
        fs::write(&first.output, b"different").unwrap();
        assert_ne!(
            fingerprint(Path::new(&first.output)).unwrap(),
            first.output_sha256
        );
    }

    #[test]
    fn archive_reference_paths_keep_their_original_hashes() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("skin.fantome");
        let mut zip = zip::ZipWriter::new(File::create(&input).unwrap());
        zip.start_file(
            "WAD/A.wad.client/assets/test.tex",
            zip::write::FileOptions::default(),
        )
        .unwrap();
        zip.write_all(&[1, 2, 3]).unwrap();
        zip.finish().unwrap();
        let hashes: Arc<dyn HashProvider> = Arc::new(TxtHashProvider::new());
        let mut files = Vec::new();
        load_artifact(&input, &hashes, &mut files, &mut Vec::new()).unwrap();
        assert_eq!(files[0].0, wad_path_hash("assets/test.tex"));
        assert_eq!(files[0].1, "assets/test.tex");
    }
    #[test]
    fn reads_build_from_install_metadata_and_rejects_invalid_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("Game");
        fs::create_dir_all(game.join("DATA/FINAL")).unwrap();
        let install = hematite_live::LeagueInstall {
            root: tmp.path().to_path_buf(),
            game_dir: game.clone(),
            auto_detected: false,
        };
        let live = LiveGameProvider::new(
            hematite_live::GameIndex::new(&install),
            Box::new(FileBinProvider),
        );
        assert_eq!(target_build(Some(&live)), None);
        fs::write(
            game.join("content-metadata.json"),
            br#"{"version":"16.22.123.456"}"#,
        )
        .unwrap();
        assert_eq!(target_build(Some(&live)).as_deref(), Some("16.22.123.456"));
        fs::write(
            game.join("content-metadata.json"),
            br#"{"version":"latest"}"#,
        )
        .unwrap();
        assert_eq!(target_build(Some(&live)), None);
    }

    #[test]
    fn rule_digest_is_independent_of_object_insertion_order() {
        let mut first = serde_json::json!({"z": {"b": 1, "a": 2}, "a": 4});
        let mut second = serde_json::json!({"a": 4, "z": {"a": 2, "b": 1}});
        canonicalize(&mut first);
        canonicalize(&mut second);
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
    }
    #[test]
    fn vfx_repair_is_validated_from_the_written_revision() {
        use hematite_core::strings::fnv1a_hash as hash;
        use hematite_types::bin::{BinObject, BinProperty, BinTree, PropertyValue, StructValue};
        use hematite_types::hash::{FieldHash, PathHash, TypeHash};
        let property = |name: &str, value| {
            (
                hash(name),
                BinProperty {
                    name_hash: FieldHash(hash(name)),
                    value,
                },
            )
        };
        let shape = StructValue {
            class_hash: TypeHash(hash("VfxShape")),
            properties: [
                property("emitOffset", PropertyValue::Vector3([1.0, 2.0, 3.0])),
                property(
                    "birthTranslation",
                    PropertyValue::Embedded(StructValue {
                        class_hash: TypeHash(hash("ValueVector3")),
                        properties: [
                            property("constantValue", PropertyValue::Vector3([4.0, 5.0, 6.0])),
                            property(
                                "dynamics",
                                PropertyValue::Container(vec![PropertyValue::F32(2.0)]),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    }),
                ),
            ]
            .into_iter()
            .collect(),
        };
        let emitter = StructValue {
            class_hash: TypeHash(hash("VfxEmitterDefinitionData")),
            properties: [property("shape", PropertyValue::Embedded(shape))]
                .into_iter()
                .collect(),
        };
        let tree = BinTree {
            objects: [(
                1,
                BinObject {
                    class_hash: TypeHash(hash("VfxSystemDefinitionData")),
                    path_hash: PathHash(1),
                    properties: [property(
                        "emitters",
                        PropertyValue::Container(vec![PropertyValue::Embedded(emitter)]),
                    )]
                    .into_iter()
                    .collect(),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("Test.wad.client");
        fs::create_dir(&input).unwrap();
        let original = FileBinProvider.write_bytes(&tree).unwrap();
        fs::write(input.join("effect.bin"), &original).unwrap();
        let config: FixConfig =
            toml::from_str(include_str!("../../../config/fix_config.toml")).unwrap();
        let champions = CharacterRelations::default();
        let hashes: Arc<dyn HashProvider> = Arc::new(TxtHashProvider::new());
        let selected = vec!["vfx_shape_fix".into()];
        let ctx = ProcessContext {
            config: &config,
            selected_fixes: &selected,
            champions: &champions,
            dry_run: false,
            check: false,
            repath_opts: None,
            ui: crate::ui::UiReporter::new(crate::ui::Mode::Silent),
            live: None,
            restore_anm: false,
            game_wad: None,
            relocate_combo_bins: false,
            input_root: tmp.path().to_path_buf(),
            out_dir: tmp.path().join("output"),
        };
        let record = process(&input, &ctx, &hashes).unwrap().repairs.remove(0);
        assert_eq!(record.validation_status, "passed_automated_checks");
        assert_eq!(record.validation.bins_checked, 1);
        assert!(record
            .applied_fixes
            .iter()
            .any(|fix| fix.fix_id == "vfx_shape_fix"));
        assert_eq!(fs::read(input.join("effect.bin")).unwrap(), original);
        let output = fs::read(Path::new(&record.output).join("effect.bin")).unwrap();
        assert_ne!(output, original);
        let repaired = FileBinProvider.parse_bytes(&output).unwrap();
        let PropertyValue::Container(emitters) =
            &repaired.objects[&1].properties[&hash("emitters")].value
        else {
            panic!()
        };
        let PropertyValue::Embedded(emitter) = &emitters[0] else {
            panic!()
        };
        let PropertyValue::Embedded(position) = &emitter.properties[&hash("EmitterPosition")].value
        else {
            panic!()
        };
        assert!(position.properties.contains_key(&hash("dynamics")));
        assert!(emitter.properties.contains_key(&hash("SpawnShape")));
    }
}
