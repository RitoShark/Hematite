//! Golden-path parity scaffold.
//!
//! `fix_folder` is the lift of the CLI's old `process_wad_folder`; this test
//! exists to assert the two produce the same `ProcessResult` (same applied-fix
//! ids + counts) over a real WAD folder. It is `#[ignore]`d because it needs a
//! real `.wad.client` fixture with known issues plus a real hash dictionary,
//! neither of which can ship in the repo (game assets are never committed).
//!
//! To run it: drop a real fixture folder under `tests/fixtures/` (gitignored),
//! point `FIXTURE` at it, and run:
//!
//! ```text
//! cargo test -p hematite-orchestrate -- --ignored
//! ```
//!
//! The full non-ignored coverage of the disk-neutral contract lives in
//! `detect_only.rs`, which runs asset-free in CI.

/// Relative path (under this crate's `tests/` dir) to a real WAD-folder
/// fixture. Left blank on purpose — fill it in locally to run the parity test.
const FIXTURE: &str = "fixtures/REPLACE_WITH_REAL.wad.client";

#[test]
#[ignore = "needs a real .wad.client fixture + hash dictionary (see module docs)"]
fn golden_path_parity() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(FIXTURE);
    assert!(
        fixture.is_dir(),
        "parity fixture not found at {} — drop a real WAD folder there first",
        fixture.display()
    );
    // A real run would: load the hash dictionary, build FixOptions, call
    // `hematite_orchestrate::fix_folder`, and compare the returned
    // `applied_fixes` (ids + counts) against a recorded baseline captured from
    // the pre-lift `process_wad_folder`. Kept as a scaffold so the wiring is in
    // place the moment a shareable fixture exists.
}
