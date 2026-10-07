//! Startup must be independent of other installed applications.
#![cfg(windows)]

use std::{fs, process::Command};

#[test]
fn startup_works_with_or_without_rose() {
    for installation in ["absent", "program-files", "local-app-data"] {
        let sandbox = tempfile::tempdir().unwrap();
        let program_files = sandbox.path().join("ProgramFiles");
        let local_app_data = sandbox.path().join("LocalAppData");
        let app_data = sandbox.path().join("AppData");
        let cache = app_data.join("Hematite/cache");
        fs::create_dir_all(&cache).unwrap();
        // A fresh local manifest makes this check deterministic and offline.
        fs::write(
            cache.join("version.json"),
            serde_json::json!({
                "latest_cli_version": env!("CARGO_PKG_VERSION"),
                "min_cli_version": env!("CARGO_PKG_VERSION"),
                "advisories": []
            })
            .to_string(),
        )
        .unwrap();

        match installation {
            "program-files" => {
                fs::create_dir_all(program_files.join("Rose")).unwrap();
                fs::write(program_files.join("Rose/Rose.exe"), b"fixture").unwrap();
            }
            "local-app-data" => fs::create_dir_all(local_app_data.join("Rose")).unwrap(),
            _ => {}
        }

        let output = Command::new(env!("CARGO_BIN_EXE_hematite-cli"))
            .args(["--check-version", "--no-pause"])
            .env("ProgramFiles", &program_files)
            .env("LOCALAPPDATA", &local_app_data)
            .env("APPDATA", &app_data)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{installation}: {stderr}");
        assert!(stderr.contains("up to date"), "{installation}: {stderr}");
    }
}
