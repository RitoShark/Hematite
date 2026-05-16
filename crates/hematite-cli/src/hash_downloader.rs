//! Automatic hash database downloader.
//!
//! Downloads pre-built LMDB hash databases from GitHub releases and decompresses them.
//! Location: %APPDATA%\RitoShark\Requirements\Hashes\hashes.lmdb

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

const GITHUB_REPO: &str = "RitoShark/lmdb-hashes";
const ASSET_NAME: &str = "lol-hashes-combined.zst";
const VERSION_FILE: &str = "version.txt";

/// Get the RitoShark hashes directory path.
fn get_hashes_dir() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA").context("APPDATA environment variable not set")?;
    Ok(PathBuf::from(appdata)
        .join("RitoShark")
        .join("Requirements")
        .join("Hashes"))
}

/// Get the LMDB directory path (LMDB databases are directories, not single files).
fn get_lmdb_path() -> Result<PathBuf> {
    Ok(get_hashes_dir()?.join("hashes.lmdb"))
}

/// Get the version tracking file path.
fn get_version_path() -> Result<PathBuf> {
    Ok(get_hashes_dir()?.join(VERSION_FILE))
}

/// GitHub release information.
#[derive(Debug, serde::Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, serde::Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// Fetch the latest release info from GitHub.
fn fetch_latest_release() -> Result<Release> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    tracing::debug!("Fetching latest release from: {}", url);

    let client = reqwest::blocking::Client::builder()
        .user_agent("hematite-cli")
        .build()?;

    let response = client
        .get(&url)
        .send()
        .context("Failed to fetch GitHub release")?;

    if !response.status().is_success() {
        anyhow::bail!("GitHub API returned status: {}", response.status());
    }

    let release: Release = response
        .json()
        .context("Failed to parse GitHub release JSON")?;

    tracing::info!("Latest release: {}", release.tag_name);

    Ok(release)
}

/// Read the currently installed version (if any).
fn read_installed_version() -> Option<String> {
    let version_path = get_version_path().ok()?;
    std::fs::read_to_string(version_path).ok()
}

/// Write the installed version to disk.
fn write_installed_version(version: &str) -> Result<()> {
    let version_path = get_version_path()?;
    std::fs::write(version_path, version).context("Failed to write version file")?;
    Ok(())
}

/// Check if we need to download (missing or outdated).
pub fn should_download() -> Result<bool> {
    let lmdb_dir = get_lmdb_path()?;
    let data_mdb = lmdb_dir.join("data.mdb");

    // If LMDB data.mdb doesn't exist, we need to download
    if !data_mdb.exists() {
        tracing::debug!("LMDB data.mdb not found, download required");
        return Ok(true);
    }

    // Check if version tracking exists
    let installed_version = read_installed_version();
    if installed_version.is_none() {
        tracing::debug!("Version tracking file not found, checking for updates");
    }

    // Fetch latest release to compare
    let release = fetch_latest_release()?;

    match installed_version {
        Some(installed) if installed == release.tag_name => {
            tracing::debug!("Hash database is up to date ({})", installed);
            Ok(false)
        }
        Some(installed) => {
            tracing::info!(
                "Hash database update available: {} -> {}",
                installed,
                release.tag_name
            );
            Ok(true)
        }
        None => {
            tracing::info!("Hash database exists but version unknown, update recommended");
            Ok(true)
        }
    }
}

/// Download and decompress the hash database.
pub fn download_hashes() -> Result<()> {
    tracing::info!("Downloading hash database from {}", GITHUB_REPO);

    // Ensure directory exists
    let hashes_dir = get_hashes_dir()?;
    std::fs::create_dir_all(&hashes_dir).context("Failed to create hashes directory")?;

    // Fetch release info
    let release = fetch_latest_release()?;

    // Find the combined asset
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == ASSET_NAME)
        .context(format!("Asset '{}' not found in release", ASSET_NAME))?;

    tracing::info!(
        "Downloading {} ({:.1} MB)",
        asset.name,
        asset.size as f64 / 1_000_000.0
    );

    // Download compressed file
    let client = reqwest::blocking::Client::builder()
        .user_agent("hematite-cli")
        .build()?;

    let response = client
        .get(&asset.browser_download_url)
        .send()
        .context("Failed to download asset")?;

    if !response.status().is_success() {
        anyhow::bail!("Download failed with status: {}", response.status());
    }

    // Read compressed data
    let compressed_data = response.bytes().context("Failed to read download bytes")?;

    tracing::info!("Decompressing hash database...");

    // Decompress using zstd
    let decompressed =
        zstd::decode_all(compressed_data.as_ref()).context("Failed to decompress zstd data")?;

    tracing::info!(
        "Decompressed size: {:.1} MB",
        decompressed.len() as f64 / 1_000_000.0
    );

    // LMDB databases are directories containing data.mdb
    // Create the directory structure
    let lmdb_dir = get_lmdb_path()?;
    std::fs::create_dir_all(&lmdb_dir).context("Failed to create LMDB directory")?;

    // Write data.mdb inside the directory
    let data_mdb_path = lmdb_dir.join("data.mdb");
    let mut file =
        BufWriter::new(File::create(&data_mdb_path).context("Failed to create data.mdb file")?);

    file.write_all(&decompressed)
        .context("Failed to write data.mdb file")?;

    file.flush().context("Failed to flush data.mdb file")?;

    // Write version tracking
    write_installed_version(&release.tag_name)?;

    tracing::info!(
        "Hash database installed successfully: {}",
        lmdb_dir.display()
    );
    tracing::info!("Version: {}", release.tag_name);

    Ok(())
}

/// Auto-download if missing or prompt user if outdated.
pub fn ensure_hashes_available(force_check: bool) -> Result<()> {
    let lmdb_dir = get_lmdb_path()?;
    let data_mdb = lmdb_dir.join("data.mdb");

    // If LMDB data.mdb exists and we're not forcing a check, skip
    if data_mdb.exists() && !force_check {
        tracing::debug!("LMDB data.mdb exists, skipping update check");
        return Ok(());
    }

    // Check if download is needed
    let needs_download = should_download()?;

    if needs_download {
        download_hashes()?;
    }

    Ok(())
}

/// Best-effort wipe of the on-disk LMDB cache. Called when the
/// adapter can't make sense of either the current or the legacy
/// schema — the next run then redownloads cleanly instead of
/// failing the same way forever. Silently ignores any I/O error
/// because losing the cache is never worse than keeping a broken
/// one.
pub fn invalidate_cache() {
    let Ok(dir) = get_lmdb_path() else { return };
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
        tracing::info!(
            "Invalidated LMDB cache at {} — will redownload on next run.",
            dir.display()
        );
    }
    let _ = get_version_path().map(std::fs::remove_file);
}

/// Force check for updates and download if available.
#[allow(dead_code)]
pub fn update_hashes() -> Result<()> {
    ensure_hashes_available(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Requires network and GitHub API
    fn test_fetch_latest_release() {
        let release = fetch_latest_release().unwrap();
        assert!(!release.tag_name.is_empty());
        assert!(!release.assets.is_empty());
    }

    #[test]
    fn test_get_paths() {
        let hashes_dir = get_hashes_dir().unwrap();
        assert!(hashes_dir.ends_with("RitoShark\\Requirements\\Hashes"));

        let lmdb_path = get_lmdb_path().unwrap();
        assert!(lmdb_path.ends_with("hashes.lmdb"));
    }
}
