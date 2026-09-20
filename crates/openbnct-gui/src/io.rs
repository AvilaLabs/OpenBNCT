// SPDX-License-Identifier: Apache-2.0

//! Target-split artifact IO. Native builds read and write real
//! filesystem paths; the web build cannot touch a filesystem, so
//! path-based entry points refuse with a clear error and every
//! artifact arrives as in-memory bytes via drag-and-drop. Panels
//! therefore always hold bytes, never file handles — the same
//! render path serves both targets.

use std::path::Path;

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

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

/// Native folder picker — only compiled on native; web callers use
/// `pick_files_into_drops` / the drop channel instead.
#[cfg(not(target_arch = "wasm32"))]
pub fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

/// Native file picker with a filter — native only.
#[cfg(not(target_arch = "wasm32"))]
pub fn pick_file(filter_name: &str, extensions: &[&str]) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(filter_name, extensions)
        .pick_file()
}

/// Map a dropped/picked file name to its manifest-relative case path.
/// Flat drops give basenames; archives may nest under a top folder —
/// only the basename decides membership.
pub fn case_member_name(name: &str) -> Option<String> {
    let normalized = name.replace('\\', "/");
    let base = normalized
        .rsplit('/')
        .next()
        .unwrap_or(normalized.as_str())
        .to_ascii_lowercase();
    if base == "case.json" {
        Some("case.json".to_owned())
    } else if base == "rtstruct.dcm" {
        Some("rtstruct.dcm".to_owned())
    } else if base.starts_with("ct-") && base.ends_with(".dcm") {
        Some(format!("ct/{base}"))
    } else {
        None
    }
}

/// True when a dropped name looks like an NF-BNCT-001 case member.
pub fn looks_like_case_member(name: &str) -> bool {
    case_member_name(name).is_some()
}

/// Unpack a `.zip` of the case directory into manifest-relative entries.
/// Works on both targets — `zip` is pure Rust with deflate inflate.
pub fn unzip_case_archive(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|error| format!("invalid zip archive: {error}"))?;
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("zip entry {index}: {error}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_owned();
        let Some(member) = case_member_name(&name) else {
            continue;
        };
        let mut contents = Vec::with_capacity(entry.size() as usize);
        std::io::Read::read_to_end(&mut entry, &mut contents)
            .map_err(|error| format!("zip entry {name}: {error}"))?;
        files.push((member, contents));
    }
    if files.is_empty() {
        return Err(
            "archive contains no NF-BNCT-001 members (expected case.json, rtstruct.dcm, ct/ct-*.dcm)"
                .to_owned(),
        );
    }
    Ok(files)
}
