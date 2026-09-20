// SPDX-License-Identifier: Apache-2.0

//! Target-split artifact IO. Native builds read and write real
//! filesystem paths; the web build cannot touch a filesystem, so
//! path-based entry points refuse with a clear error and every
//! artifact arrives as in-memory bytes via drag-and-drop. Panels
//! therefore always hold bytes, never file handles — the same
//! render path serves both targets.

use std::path::{Path, PathBuf};

/// Read an artifact by path. Web builds have no filesystem — callers
/// must route bytes through `dropped_bytes` instead.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
}

/// Path-based reads are unavailable on the web target; drag-and-drop
/// delivers `DroppedFile::bytes` directly.
#[cfg(target_arch = "wasm32")]
pub fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    Err(format!(
        "{}: filesystem paths are unavailable in the web build — drop the file instead",
        path.display()
    ))
}

/// Write bytes to a path. Web builds refuse: downloads are a browser
/// concern layered on top of artifact bytes, not filesystem writes.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_bytes(path: &Path, contents: &[u8]) -> Result<(), String> {
    std::fs::write(path, contents).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(target_arch = "wasm32")]
pub fn write_bytes(path: &Path, _contents: &[u8]) -> Result<(), String> {
    Err(format!(
        "{}: filesystem writes are unavailable in the web build",
        path.display()
    ))
}

/// Native folder picker. Web builds return `None` — the button that
/// calls this is hidden on the web target.
#[cfg(not(target_arch = "wasm32"))]
pub fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

#[cfg(target_arch = "wasm32")]
pub fn pick_folder() -> Option<PathBuf> {
    None
}

/// Native file picker with a filter. Web builds return `None`.
#[cfg(not(target_arch = "wasm32"))]
pub fn pick_file(filter_name: &str, extensions: &[&str]) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(filter_name, extensions)
        .pick_file()
}

#[cfg(target_arch = "wasm32")]
pub fn pick_file(_filter_name: &str, _extensions: &[&str]) -> Option<PathBuf> {
    None
}
