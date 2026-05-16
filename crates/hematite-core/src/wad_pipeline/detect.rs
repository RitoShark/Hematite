//! WAD-level detection rules.
//!
//! Checks if a file matches a WadDetectionRule.

use anyhow::{Context, Result};
use hematite_types::config::{BinaryHeaderCheck, Endian, WadDetectionRule};

/// Check if a file matches a WAD detection rule.
pub fn check_file(path: &str, bytes: &[u8], rule: &WadDetectionRule) -> Result<bool> {
    match rule {
        WadDetectionRule::FileExtension {
            extension,
            binary_check,
            exclude_files,
        } => check_extension(path, extension, bytes, binary_check.as_ref(), exclude_files),
        WadDetectionRule::FilePattern {
            pattern,
            binary_check,
        } => check_pattern(path, pattern, bytes, binary_check.as_ref()),
        // `Always` is consumed at the rule level (see wad_pipeline::mod),
        // not via per-file iteration. Returning `false` here keeps the
        // per-file loop a no-op for this variant.
        WadDetectionRule::Always => Ok(false),
    }
}

fn check_extension(
    path: &str,
    extension: &str,
    bytes: &[u8],
    binary_check: Option<&BinaryHeaderCheck>,
    exclude_files: &[String],
) -> Result<bool> {
    let path_lower = path.to_lowercase();

    if !path_lower.ends_with(extension) {
        return Ok(false);
    }

    // Check if filename is in exclusion list (e.g., sfx_events.bnk)
    if let Some(filename) = path_lower.split('/').next_back() {
        if exclude_files
            .iter()
            .any(|excluded| excluded.to_lowercase() == filename)
        {
            return Ok(false);
        }
    }

    if let Some(check) = binary_check {
        check_binary_header(bytes, check)
    } else {
        Ok(true)
    }
}

fn check_pattern(
    path: &str,
    pattern: &str,
    bytes: &[u8],
    binary_check: Option<&BinaryHeaderCheck>,
) -> Result<bool> {
    // Simple glob-style pattern matching (* and ** support)
    let regex_pattern = pattern
        .replace(".", r"\.")
        .replace("**", ".*")
        .replace("*", "[^/]*");

    let regex =
        regex::Regex::new(&format!("^{}$", regex_pattern)).context("Invalid file pattern")?;

    if !regex.is_match(&path.to_lowercase()) {
        return Ok(false);
    }

    if let Some(check) = binary_check {
        check_binary_header(bytes, check)
    } else {
        Ok(true)
    }
}

fn check_binary_header(bytes: &[u8], check: &BinaryHeaderCheck) -> Result<bool> {
    match check {
        BinaryHeaderCheck::VersionAtOffset {
            offset,
            size,
            endian,
            allowed_versions,
        } => check_version_at_offset(bytes, *offset, *size, endian, allowed_versions),
        BinaryHeaderCheck::MagicSignature { signature } => Ok(bytes.starts_with(signature)),
    }
}

fn check_version_at_offset(
    bytes: &[u8],
    offset: usize,
    size: usize,
    endian: &Endian,
    allowed_versions: &[u32],
) -> Result<bool> {
    if offset + size > bytes.len() {
        return Ok(false);
    }

    let version = match size {
        1 => bytes[offset] as u32,
        2 => {
            let slice = &bytes[offset..offset + 2];
            match endian {
                Endian::Little => u16::from_le_bytes([slice[0], slice[1]]) as u32,
                Endian::Big => u16::from_be_bytes([slice[0], slice[1]]) as u32,
            }
        }
        4 => {
            let slice = &bytes[offset..offset + 4];
            match endian {
                Endian::Little => u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]),
                Endian::Big => u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]),
            }
        }
        _ => anyhow::bail!("Invalid version size: {} (must be 1, 2, or 4)", size),
    };

    // Return true if version is NOT in allowed list (needs fixing)
    Ok(!allowed_versions.contains(&version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_extension() {
        let result = check_extension("test.bnk", ".bnk", &[], None, &[]).unwrap();
        assert!(result);

        let result = check_extension("test.bin", ".bnk", &[], None, &[]).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_check_version_at_offset() {
        // Version 100 at offset 8 (4 bytes, little endian)
        let mut bytes = vec![0u8; 16];
        bytes[8..12].copy_from_slice(&100u32.to_le_bytes());

        let result = check_version_at_offset(&bytes, 8, 4, &Endian::Little, &[145, 134]).unwrap();
        assert!(result); // Version 100 NOT in allowed list → needs fixing

        let result = check_version_at_offset(&bytes, 8, 4, &Endian::Little, &[100, 145]).unwrap();
        assert!(!result); // Version 100 in allowed list → OK
    }

    #[test]
    fn test_magic_signature() {
        let bytes = vec![0x42, 0x4B, 0x48, 0x44, 0x01, 0x02]; // "BKHD" header
        let check = BinaryHeaderCheck::MagicSignature {
            signature: vec![0x42, 0x4B, 0x48, 0x44],
        };

        assert!(check_binary_header(&bytes, &check).unwrap());
    }
}
