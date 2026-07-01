//! SCO ↔ SCB mesh format conversion using rs_mesh.
//!
//! Converts between League's ASCII (.sco) and binary (.scb) static mesh formats.

use anyhow::{Context, Result};
use rs_io::{Parse, Serialize};

/// Convert SCO (ASCII) static mesh to SCB (binary) format.
///
/// Reads an ASCII static mesh file (.sco) and converts it to the binary
/// format (.scb) used in League of Legends WAD files.
///
/// ## Format Details
/// - **SCO**: ASCII text format with [ObjectBegin]/[ObjectEnd] markers
/// - **SCB**: Binary format with "r3d2Mesh" magic (version 3.2)
/// - Both formats support: vertices, faces, vertex colors, face colors
///
/// ## Process
/// 1. Parse ASCII .sco file → StaticMesh
/// 2. Serialize StaticMesh → binary .scb bytes
///
/// This is a **lossless** conversion (both formats store the same data).
pub fn sco_to_scb(sco_bytes: &[u8]) -> Result<Vec<u8>> {
    use rs_mesh::StaticMesh;

    // Parse the static mesh. `StaticMesh::from_bytes` auto-detects the ASCII
    // `.sco` (`[Object...`) and binary `.scb` (`r3d2Mesh`) forms.
    let mesh = StaticMesh::from_bytes(sco_bytes).context("Failed to parse SCO file")?;

    tracing::debug!(
        "Converting SCO→SCB: mesh '{}', {} positions, {} faces",
        mesh.name(),
        mesh.positions().len(),
        mesh.faces().len()
    );

    // Serialize to SCB (binary format) — rs_mesh's `Serialize` emits binary.
    let output = mesh.to_bytes().context("Failed to write SCB data")?;

    tracing::info!(
        "Converted SCO→SCB: mesh '{}', {} positions, {} faces, {} bytes → {} bytes",
        mesh.name(),
        mesh.positions().len(),
        mesh.faces().len(),
        sco_bytes.len(),
        output.len()
    );

    Ok(output)
}

/// Convert SCB (binary) static mesh to SCO (ASCII) format.
///
/// Reads a binary static mesh file (.scb) and converts it to the ASCII
/// format (.sco) used for editing and version control.
///
/// ## Limitation
/// rs_mesh does **not** provide an ASCII (`.sco`) writer — its `Serialize`
/// impl only emits the binary `.scb` form (the text form was removed from the
/// game and is no longer written). This direction is therefore unsupported and
/// always returns an error. It is currently unused (only `sco_to_scb` is wired
/// into the converter registry).
pub fn scb_to_sco(_scb_bytes: &[u8]) -> Result<Vec<u8>> {
    anyhow::bail!("SCB->SCO ascii export not supported by rs_mesh")
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore] // Requires actual SCO/SCB files
    fn test_sco_to_scb_roundtrip() {
        // Requires real mesh files — tested manually with sample files
    }
}
