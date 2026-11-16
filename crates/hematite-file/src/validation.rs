use crate::{audio, bin_adapter::FileBinProvider, wad_adapter::wad_path_hash};
use hematite_core::{repath, traits::BinProvider};
use hematite_types::result::ValidationReport;
use rs_io::{Parse, Serialize};
use std::collections::BTreeSet;

pub fn validate_files(
    files: &[(u64, String, Vec<u8>)],
    game_has_hash: &dyn Fn(u64) -> bool,
    complete_game_inventory: bool,
) -> ValidationReport {
    let mut report = ValidationReport {
        checks: vec![
            "bin_parse_and_roundtrip".into(),
            "asset_and_linked_bin_references".into(),
            "bank_structure_and_wem_headers".into(),
        ],
        ..Default::default()
    };
    let shipped: BTreeSet<u64> = files.iter().map(|(hash, _, _)| *hash).collect();
    for (_, path, bytes) in files {
        let lower = path.to_lowercase();
        if lower.ends_with(".bnk")
            || lower.ends_with(".wpk")
            || bytes.starts_with(b"BKHD")
            || (bytes.starts_with(b"r3d2") && bytes.get(4..8) == Some(&[1, 0, 0, 0]))
        {
            report.banks_checked += 1;
            if let Err(error) = audio::validate_bank(bytes) {
                report.issues.push(format!("{path}: {error:#}"));
            }
        }
        if !lower.ends_with(".bin") && !repath::looks_like_bin(bytes) {
            continue;
        }
        report.bins_checked += 1;
        let tree = match FileBinProvider.parse_bytes(bytes) {
            Ok(tree) => tree,
            Err(error) => {
                report
                    .issues
                    .push(format!("{path}: unreadable BIN: {error}"));
                continue;
            }
        };
        let roundtrip = FileBinProvider.write_bytes(&tree).and_then(|written| {
            let original = rs_bin::Bin::from_bytes(bytes)?;
            anyhow::ensure!(
                original.to_bytes()? == written,
                "BIN roundtrip changed parsed data"
            );
            let read = FileBinProvider.parse_bytes(&written)?;
            let again = FileBinProvider.write_bytes(&read)?;
            anyhow::ensure!(written == again, "BIN serialization is not stable");
            Ok(())
        });
        if let Err(error) = roundtrip {
            report.issues.push(format!("{path}: {error}"));
        }
        let mut references: BTreeSet<u64> = repath::collect_bin_asset_hashes(&tree)
            .into_iter()
            .filter(|hash| *hash != 0)
            .collect();
        let paths = repath::collect_bin_asset_paths(&tree, false)
            .into_iter()
            .chain(tree.linked.iter().cloned());
        for reference in paths {
            references.insert(wad_path_hash(&reference.to_lowercase().replace('\\', "/")));
        }
        for reference in references {
            report.references_checked += 1;
            if !shipped.contains(&reference) && !game_has_hash(reference) {
                let message = format!("{path}: reference {reference:016x} was not found in this package or the indexed game WADs");
                if complete_game_inventory {
                    report.issues.push(message);
                } else {
                    report.limitations.push(message);
                }
            }
        }
    }
    if report.bins_checked == 0 && report.banks_checked == 0 {
        report
            .limitations
            .push("No BIN or audio bank could be validated".into());
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use hematite_types::{
        bin::{BinObject, BinProperty, BinTree, PropertyValue},
        hash::{FieldHash, PathHash, TypeHash},
    };

    #[test]
    fn checks_cross_wad_hash_references_and_reports_missing_and_corrupt_bins() {
        let tree = BinTree {
            objects: [(
                1,
                BinObject {
                    class_hash: TypeHash(2),
                    path_hash: PathHash(1),
                    properties: [(
                        3,
                        BinProperty {
                            name_hash: FieldHash(3),
                            value: PropertyValue::WadHash(42),
                        },
                    )]
                    .into_iter()
                    .collect(),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let bytes = FileBinProvider.write_bytes(&tree).unwrap();
        let mut files = vec![(1, "skin.bin".into(), bytes)];
        assert_eq!(
            validate_files(&files, &|_| false, false).status(),
            "incomplete"
        );
        assert_eq!(
            validate_files(&files, &|_| false, true).status(),
            "has_unresolved_issues"
        );
        assert_eq!(
            validate_files(&files, &|h| h == 42, false).status(),
            "passed_automated_checks"
        );
        files.push((42, "asset.tex".into(), vec![]));
        assert_eq!(
            validate_files(&files, &|_| false, false).status(),
            "passed_automated_checks"
        );
        files.push((99, "broken.bin".into(), vec![1]));
        assert_eq!(
            validate_files(&files, &|_| false, false).status(),
            "has_unresolved_issues"
        );
    }
    #[test]
    fn does_not_mistake_static_meshes_for_audio_packages() {
        let report = validate_files(
            &[(1, "mesh.scb".into(), b"r3d2Mesh".to_vec())],
            &|_| false,
            false,
        );
        assert_eq!(report.banks_checked, 0);
        assert!(report.issues.is_empty());
    }
}
