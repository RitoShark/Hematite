//! Versioned CLI update gate.
//!
//! Hematite is normally config-driven — fix rules, champion list, repath
//! defaults all live in JSON files that get fetched from the remote at
//! runtime, so binaries rarely need to be re-shipped. When something *does*
//! force a binary change (BIN parser breakage, hash-DB schema bump, etc.)
//! we need a way to refuse to run outdated CLIs without users having to
//! discover that on their own.
//!
//! ## How it works
//! The remote ships a tiny `version.json` next to `fix_config.json`:
//!
//! ```json
//! {
//!   "latest_cli_version": "0.4.1",
//!   "min_cli_version": "0.3.0",
//!   "download_url": "https://github.com/RitoShark/Hematite/releases/latest",
//!   "release_notes": "Fixes BIN parser regression on 14.20 mods.",
//!   "advisories": []
//! }
//! ```
//!
//! On startup we:
//! 1. Read `env!("CARGO_PKG_VERSION")` as the running version.
//! 2. Fetch `version.json` (cached for 15 min, falls back to stale cache,
//!    finally to an embedded "permissive" default that approves everything).
//! 3. Compare with `semver`:
//!    * `running < min_cli_version` → **hard block** unless
//!      `--skip-version-check` is passed.
//!    * `running < latest_cli_version` → **soft notice** printed once.
//!    * Otherwise silent.
//!
//! The decision policy lives in the JSON — bumping `min_cli_version` is the
//! only thing required to force every old CLI in the wild to upgrade.
//! There is no hard-coded "this version is too old" logic in Rust.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Where the version manifest lives. Same repo path as the other configs
/// so a single edit can ship a new version policy without a binary release.
const VERSION_MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/RitoShark/Hematite/main/config/version.json";

/// Version manifest is cheap and changes only when we cut a release —
/// 15 minutes is plenty fresh without hammering GitHub.
const CACHE_TTL: Duration = Duration::from_secs(15 * 60);

/// HTTP timeout for the manifest fetch. Short — we don't want to delay
/// startup if GitHub is slow.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Built-in manifest baked in at compile time. Used when the remote is
/// unreachable AND there is no on-disk cache. Permissive on purpose: we
/// must never block startup just because the network is down.
const EMBEDDED_MANIFEST_JSON: &str = r#"{
    "latest_cli_version": "0.0.0",
    "min_cli_version": "0.0.0",
    "download_url": "https://github.com/RitoShark/Hematite/releases/latest",
    "release_notes": "",
    "advisories": []
}"#;

/// One-off advisory shown to users on the version-check banner. Useful for
/// "patch X.Y breaks Z, please upgrade" warnings that don't warrant a hard
/// block but should reach every user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Advisory {
    /// Short headline shown in the banner.
    pub title: String,
    /// Optional longer body.
    #[serde(default)]
    pub body: String,
    /// Severity hint — `info` / `warn` / `error`. Free-form, only affects
    /// the colour of the printed banner.
    #[serde(default = "default_severity")]
    pub severity: String,
}

fn default_severity() -> String {
    "info".into()
}

/// Schema of the remote `version.json` document.
///
/// New fields must default so old CLIs reading a newer manifest don't
/// crash on parse — and so a new manifest field never becomes a forced
/// migration in itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionManifest {
    /// Latest published CLI version. Used for the soft-notice path.
    pub latest_cli_version: String,
    /// CLIs older than this MUST upgrade — execution is refused (unless
    /// the user passes `--skip-version-check`). Bump this only when an
    /// older CLI would produce broken output or crash mid-run.
    pub min_cli_version: String,
    /// Where to point users to download the new release.
    #[serde(default = "default_download_url")]
    pub download_url: String,
    /// Free-form release notes for the latest version. Shown on soft
    /// notice; trimmed if very long.
    #[serde(default)]
    pub release_notes: String,
    /// Out-of-band messages shown on the banner — independent of the
    /// version gate.
    #[serde(default)]
    pub advisories: Vec<Advisory>,
}

fn default_download_url() -> String {
    "https://github.com/RitoShark/Hematite/releases/latest".into()
}

/// Outcome of a single version-check.
#[derive(Debug, Clone)]
pub enum VersionStatus {
    /// Running version is at or above `latest_cli_version`. Nothing to say.
    UpToDate,
    /// Running version is between `min_cli_version` and `latest_cli_version`.
    /// Print a banner but proceed.
    UpdateAvailable {
        running: String,
        latest: String,
        manifest: VersionManifest,
    },
    /// Running version is below `min_cli_version`. Refuse to run unless
    /// `--skip-version-check` was passed.
    Outdated {
        running: String,
        minimum: String,
        manifest: VersionManifest,
    },
    /// Could not determine status (network failure + no cache + no
    /// embedded baseline parse). Always treated as `UpToDate` from a
    /// gating standpoint — we never block on infra failures.
    Unknown,
}

/// Result returned by [`check_version`].
pub struct CheckOutcome {
    pub status: VersionStatus,
    /// `true` when the manifest had any advisories worth printing,
    /// regardless of the version gate. Exposed so future callers (e.g.
    /// JSON output, a status sidecar) can short-circuit on "nothing to
    /// say" — not consumed today by the banner path.
    #[allow(dead_code)]
    pub has_advisories: bool,
}

// ---------------------------------------------------------------------------
// Cache plumbing — mirrors `remote.rs` so behaviour is consistent.
// ---------------------------------------------------------------------------

fn cache_dir() -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").context("APPDATA environment variable not set")?;
        Ok(PathBuf::from(appdata).join("Hematite").join("cache"))
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").context("HOME environment variable not set")?;
        Ok(PathBuf::from(home).join("Library/Application Support/Hematite/cache"))
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME").context("HOME environment variable not set")?;
        Ok(PathBuf::from(home).join(".config/hematite/cache"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Ok(PathBuf::from(".hematite/cache"))
    }
}

fn cache_file() -> Result<PathBuf> {
    Ok(cache_dir()?.join("version.json"))
}

fn is_cache_fresh(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|d| d < CACHE_TTL)
        .unwrap_or(false)
}

fn fetch_manifest() -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(format!("Hematite-CLI/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("Failed to build HTTP client")?;
    let resp = client
        .get(VERSION_MANIFEST_URL)
        .send()
        .context("Failed to fetch version manifest")?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} from {}", resp.status(), VERSION_MANIFEST_URL);
    }
    resp.text().context("Failed to read manifest body")
}

fn load_manifest() -> VersionManifest {
    // 1. Fresh cache.
    if let Ok(path) = cache_file() {
        if is_cache_fresh(&path) {
            if let Ok(body) = fs::read_to_string(&path) {
                if let Ok(m) = serde_json::from_str::<VersionManifest>(&body) {
                    tracing::debug!(
                        "Using cached version manifest (latest={})",
                        m.latest_cli_version
                    );
                    return m;
                }
            }
        }
    }

    // 2. Remote.
    match fetch_manifest() {
        Ok(body) => match serde_json::from_str::<VersionManifest>(&body) {
            Ok(m) => {
                if let Ok(path) = cache_file() {
                    if let Some(parent) = path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::write(&path, &body);
                }
                tracing::debug!(
                    "Fetched version manifest (latest={}, min={})",
                    m.latest_cli_version,
                    m.min_cli_version
                );
                return m;
            }
            Err(e) => tracing::warn!("Failed to parse remote version manifest: {e}"),
        },
        Err(e) => tracing::debug!("Failed to fetch version manifest: {e}"),
    }

    // 3. Stale cache.
    if let Ok(path) = cache_file() {
        if let Ok(body) = fs::read_to_string(&path) {
            if let Ok(m) = serde_json::from_str::<VersionManifest>(&body) {
                tracing::debug!(
                    "Using stale cached version manifest (latest={})",
                    m.latest_cli_version
                );
                return m;
            }
        }
    }

    // 4. Embedded permissive default.
    serde_json::from_str(EMBEDDED_MANIFEST_JSON)
        .expect("Embedded version manifest is invalid - this is a build error")
}

/// Compare a `running` semver against a `bound`. Returns `true` when
/// running is strictly older than bound. Non-semver inputs are treated
/// permissively (returns `false` so the gate doesn't fire on parse error).
fn is_older(running: &str, bound: &str) -> bool {
    let Ok(r) = semver::Version::parse(running) else {
        return false;
    };
    let Ok(b) = semver::Version::parse(bound) else {
        return false;
    };
    r < b
}

/// Check the running CLI against the remote manifest.
pub fn check_version() -> CheckOutcome {
    let manifest = load_manifest();
    let running = env!("CARGO_PKG_VERSION").to_string();
    let has_advisories = !manifest.advisories.is_empty();

    let status = if is_older(&running, &manifest.min_cli_version) {
        VersionStatus::Outdated {
            running,
            minimum: manifest.min_cli_version.clone(),
            manifest,
        }
    } else if is_older(&running, &manifest.latest_cli_version) {
        VersionStatus::UpdateAvailable {
            running,
            latest: manifest.latest_cli_version.clone(),
            manifest,
        }
    } else if manifest.latest_cli_version == "0.0.0" {
        // Embedded permissive default — nothing to compare against.
        VersionStatus::Unknown
    } else {
        VersionStatus::UpToDate
    };

    CheckOutcome {
        status,
        has_advisories,
    }
}

/// Render the result to stderr. Returns `true` when execution should be
/// blocked (caller is responsible for bailing).
///
/// `skip_check` short-circuits the hard-block path — the banner is still
/// printed so users notice they're running an outdated CLI even when
/// they've opted out of the gate.
pub fn report(outcome: &CheckOutcome, skip_check: bool) -> bool {
    use colored::Colorize;

    // Advisories first — independent of the version gate.
    if let Some(m) = manifest_of(outcome) {
        for adv in &m.advisories {
            let tag = match adv.severity.as_str() {
                "error" => "ADVISORY".red().bold(),
                "warn" => "ADVISORY".yellow().bold(),
                _ => "ADVISORY".cyan().bold(),
            };
            eprintln!("\n[{}] {}", tag, adv.title);
            if !adv.body.is_empty() {
                eprintln!("  {}", adv.body);
            }
        }
    }

    match &outcome.status {
        VersionStatus::UpToDate | VersionStatus::Unknown => false,

        VersionStatus::UpdateAvailable {
            running,
            latest,
            manifest,
        } => {
            eprintln!(
                "\n[{}] Hematite-CLI {} is available (you are on {}).",
                "update".cyan().bold(),
                latest,
                running
            );
            if !manifest.release_notes.is_empty() {
                let notes = trim_notes(&manifest.release_notes);
                eprintln!("  {}", notes);
            }
            eprintln!("  Download: {}", manifest.download_url);
            false
        }

        VersionStatus::Outdated {
            running,
            minimum,
            manifest,
        } => {
            eprintln!(
                "\n[{}] Hematite-CLI {} is too old — minimum required is {}.",
                "BLOCKED".red().bold(),
                running,
                minimum
            );
            if !manifest.release_notes.is_empty() {
                let notes = trim_notes(&manifest.release_notes);
                eprintln!("  {}", notes);
            }
            eprintln!("  Download: {}", manifest.download_url);
            if skip_check {
                eprintln!(
                    "  {} continuing because --skip-version-check was passed.",
                    "Warning:".yellow()
                );
                false
            } else {
                eprintln!("  Pass --skip-version-check to override at your own risk.");
                true
            }
        }
    }
}

fn manifest_of(outcome: &CheckOutcome) -> Option<&VersionManifest> {
    match &outcome.status {
        VersionStatus::UpdateAvailable { manifest, .. } => Some(manifest),
        VersionStatus::Outdated { manifest, .. } => Some(manifest),
        _ => None,
    }
}

fn trim_notes(notes: &str) -> String {
    const MAX: usize = 280;
    if notes.len() <= MAX {
        notes.to_string()
    } else {
        let mut s: String = notes.chars().take(MAX).collect();
        s.push('…');
        s
    }
}

/// Clear the on-disk version manifest cache. Sibling of
/// `remote::clear_cache`; useful for `--refresh-cache` style flags.
#[allow(dead_code)]
pub fn clear_cache() -> Result<()> {
    let path = cache_file()?;
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("Failed to remove version cache: {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_manifest_parses() {
        let m: VersionManifest = serde_json::from_str(EMBEDDED_MANIFEST_JSON).unwrap();
        assert_eq!(m.latest_cli_version, "0.0.0");
        assert_eq!(m.min_cli_version, "0.0.0");
    }

    #[test]
    fn is_older_basic_semver() {
        assert!(is_older("0.1.0", "0.2.0"));
        assert!(!is_older("0.2.0", "0.2.0"));
        assert!(!is_older("0.3.0", "0.2.0"));
    }

    #[test]
    fn is_older_permissive_on_garbage() {
        // Unparseable inputs must not fire the gate — we treat them as
        // "could not determine", which means "do not block".
        assert!(!is_older("not-a-version", "0.2.0"));
        assert!(!is_older("0.2.0", "also-not-a-version"));
    }

    #[test]
    fn outdated_status_when_below_minimum() {
        let manifest = VersionManifest {
            latest_cli_version: "1.0.0".into(),
            min_cli_version: "1.0.0".into(),
            download_url: default_download_url(),
            release_notes: String::new(),
            advisories: vec![],
        };
        // Simulate the policy comparison done in `check_version`.
        let running = "0.5.0";
        assert!(is_older(running, &manifest.min_cli_version));
        assert!(is_older(running, &manifest.latest_cli_version));
    }

    #[test]
    fn permissive_default_yields_unknown() {
        // The baked-in "everything is fine" manifest must never trigger
        // a notice.
        let m: VersionManifest = serde_json::from_str(EMBEDDED_MANIFEST_JSON).unwrap();
        let running = env!("CARGO_PKG_VERSION");
        assert!(!is_older(running, &m.min_cli_version));
        assert!(!is_older(running, &m.latest_cli_version));
    }
}
