// SPDX-License-Identifier: MIT

//! Hash-bound external arrays ("sidecars") for large numeric artifacts.
//!
//! Volume-scale arrays (multigroup flux, dose values and uncertainties) are
//! pure JSON by default. Above a size threshold a writer may instead emit
//! the array as a raw little-endian `f64` file next to the JSON document and
//! reference it from the document:
//!
//! ```json
//! {"external": {"path": "case.flux.f64le", "sha256": "<hex>", "len": 1234,
//!               "dtype": "f64le"}}
//! ```
//!
//! The document's own content hash covers the sidecar's SHA-256, so
//! provenance stays hash-bound end to end. The inline JSON-array encoding is
//! unchanged and remains the default for small artifacts, so existing
//! artifacts (and their hashes) are byte-for-byte unaffected.
//!
//! Reading is two-phase. The wire type [`F64Array`] (or [`F64Rows`] for a
//! nested `[row][col]` array) deserializes from either encoding without
//! touching the disk; [`F64Array::resolve`] then loads and verifies the
//! sidecar (length and SHA-256) relative to the document's directory. The
//! `serde(with = ...)` modules in this file perform both phases during
//! deserialization using the base directory installed by [`from_slice_in`],
//! [`load_json`] or a `load_*` helper; an external array met without a base
//! directory is a clear error, never a silent empty array.
//!
//! Writing is opt-in through [`with_write_context`] (used by the CLI's
//! artifact writers); outside a write context every array serializes inline.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::de::{DeserializeOwned, Error as _, MapAccess, SeqAccess, Visitor};
use serde::ser::{Error as _, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// The only supported sidecar element type: IEEE-754 binary64, little endian.
pub const DTYPE_F64LE: &str = "f64le";
/// Environment variable overriding the sidecar threshold: an array is
/// written as a sidecar when it holds more than this many values. `0` means
/// always (any non-empty array), a negative value means never.
pub const SIDECAR_MIN_VALUES_ENV: &str = "OPENBNCT_SIDECAR_MIN_VALUES";
/// Default sidecar threshold in values.
pub const DEFAULT_SIDECAR_MIN_VALUES: i64 = 1_000_000;

#[derive(Debug, Error)]
pub enum SidecarError {
    #[error("sidecar {path}: cannot read: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("sidecar {path}: sha256 mismatch: document declares {expected}, file has {observed}")]
    HashMismatch {
        path: PathBuf,
        expected: String,
        observed: String,
    },
    #[error(
        "sidecar {path}: length mismatch: document declares {expected} f64 values, file holds {observed_bytes} bytes"
    )]
    LengthMismatch {
        path: PathBuf,
        expected: usize,
        observed_bytes: u64,
    },
    #[error("invalid external array reference: {0}")]
    InvalidReference(String),
    #[error(
        "external array {0:?} cannot be resolved without the document's directory; load the artifact through the load_* helpers"
    )]
    NoBaseDirectory(String),
    #[error("invalid {SIDECAR_MIN_VALUES_ENV} value {0:?}: expected an integer")]
    InvalidThreshold(String),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

/// Reference to a raw little-endian `f64` file relative to the JSON document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalArrayRef {
    /// File name (relative to the JSON document's directory).
    pub path: String,
    /// Lowercase hex SHA-256 of the sidecar file's bytes.
    pub sha256: String,
    /// Number of `f64` values in the file.
    pub len: usize,
    /// Always [`DTYPE_F64LE`].
    pub dtype: String,
    /// Row width when the sidecar stores a `[row][col]` array flattened in
    /// row-major order (the multigroup flux, `[voxel][group]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_len: Option<usize>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExternalEnvelope {
    external: ExternalArrayRef,
}

impl ExternalArrayRef {
    fn check_shape(&self) -> Result<(), SidecarError> {
        if self.dtype != DTYPE_F64LE {
            return Err(SidecarError::InvalidReference(format!(
                "{:?}: unsupported dtype {:?} (expected {DTYPE_F64LE:?})",
                self.path, self.dtype
            )));
        }
        let rel = Path::new(&self.path);
        if self.path.is_empty()
            || rel.is_absolute()
            || rel.components().any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(SidecarError::InvalidReference(format!(
                "path {:?} must be a relative path without '..' components",
                self.path
            )));
        }
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(SidecarError::InvalidReference(format!(
                "{:?}: sha256 must be 64 lowercase hex characters",
                self.path
            )));
        }
        Ok(())
    }

    /// Location of the sidecar given the document directory.
    #[must_use]
    pub fn locate(&self, base_dir: &Path) -> PathBuf {
        base_dir.join(&self.path)
    }

    /// Verify the sidecar file's length and SHA-256 without decoding it.
    pub fn verify(&self, base_dir: &Path) -> Result<(), SidecarError> {
        self.check_shape()?;
        let path = self.locate(base_dir);
        let io_err = |source| SidecarError::Io {
            path: path.clone(),
            source,
        };
        let mut file = fs::File::open(&path).map_err(io_err)?;
        let bytes = file.metadata().map_err(io_err)?.len();
        if bytes != (self.len as u64).saturating_mul(8) {
            return Err(SidecarError::LengthMismatch {
                path,
                expected: self.len,
                observed_bytes: bytes,
            });
        }
        let mut digest = Sha256::new();
        let mut buffer = vec![0_u8; 1 << 20];
        loop {
            let read = file.read(&mut buffer).map_err(io_err)?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        let observed = format!("{:x}", digest.finalize());
        if observed != self.sha256 {
            return Err(SidecarError::HashMismatch {
                path,
                expected: self.sha256.clone(),
                observed,
            });
        }
        Ok(())
    }

    /// Load and verify the sidecar, returning the flat value vector.
    pub fn load(&self, base_dir: &Path) -> Result<Vec<f64>, SidecarError> {
        self.check_shape()?;
        let path = self.locate(base_dir);
        let bytes = fs::read(&path).map_err(|source| SidecarError::Io {
            path: path.clone(),
            source,
        })?;
        if bytes.len() as u64 != (self.len as u64).saturating_mul(8) {
            return Err(SidecarError::LengthMismatch {
                path,
                expected: self.len,
                observed_bytes: bytes.len() as u64,
            });
        }
        let observed = format!("{:x}", Sha256::digest(&bytes));
        if observed != self.sha256 {
            return Err(SidecarError::HashMismatch {
                path,
                expected: self.sha256.clone(),
                observed,
            });
        }
        Ok(bytes
            .chunks_exact(8)
            .map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
            .collect())
    }
}

/// A flat numeric array in either encoding.
#[derive(Debug, Clone, PartialEq)]
pub enum F64Array {
    Inline(Vec<f64>),
    External(ExternalArrayRef),
}

impl F64Array {
    /// Phase two: load and verify an external array relative to `base_dir`.
    pub fn resolve(self, base_dir: &Path) -> Result<Vec<f64>, SidecarError> {
        match self {
            Self::Inline(values) => Ok(values),
            Self::External(reference) => reference.load(base_dir),
        }
    }
}

/// A `[row][col]` numeric array in either encoding. The external form is a
/// flat row-major file with `row_len` set.
#[derive(Debug, Clone, PartialEq)]
pub enum F64Rows {
    Inline(Vec<Vec<f64>>),
    External(ExternalArrayRef),
}

impl F64Rows {
    pub fn resolve(self, base_dir: &Path) -> Result<Vec<Vec<f64>>, SidecarError> {
        match self {
            Self::Inline(rows) => Ok(rows),
            Self::External(reference) => {
                let row_len = reference.row_len.filter(|n| *n > 0).ok_or_else(|| {
                    SidecarError::InvalidReference(format!(
                        "{:?}: a row array needs a positive row_len",
                        reference.path
                    ))
                })?;
                if reference.len % row_len != 0 {
                    return Err(SidecarError::InvalidReference(format!(
                        "{:?}: len {} is not a multiple of row_len {row_len}",
                        reference.path, reference.len
                    )));
                }
                let flat = reference.load(base_dir)?;
                Ok(flat.chunks_exact(row_len).map(<[f64]>::to_vec).collect())
            }
        }
    }
}

struct ArrayVisitor;

impl<'de> Visitor<'de> for ArrayVisitor {
    type Value = F64Array;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array of numbers or an {\"external\": {...}} sidecar reference")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<F64Array, A::Error> {
        let mut values = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(1 << 24));
        while let Some(v) = seq.next_element::<f64>()? {
            values.push(v);
        }
        Ok(F64Array::Inline(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<F64Array, A::Error> {
        let envelope =
            ExternalEnvelope::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
        Ok(F64Array::External(envelope.external))
    }
}

impl<'de> Deserialize<'de> for F64Array {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(ArrayVisitor)
    }
}

impl Serialize for F64Array {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Inline(values) => s.collect_seq(values),
            Self::External(reference) => ExternalEnvelope {
                external: reference.clone(),
            }
            .serialize(s),
        }
    }
}

struct RowsVisitor;

impl<'de> Visitor<'de> for RowsVisitor {
    type Value = F64Rows;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array of number arrays or an {\"external\": {...}} sidecar reference")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<F64Rows, A::Error> {
        let mut rows = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(1 << 24));
        while let Some(row) = seq.next_element::<Vec<f64>>()? {
            rows.push(row);
        }
        Ok(F64Rows::Inline(rows))
    }
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<F64Rows, A::Error> {
        let envelope =
            ExternalEnvelope::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
        Ok(F64Rows::External(envelope.external))
    }
}

impl<'de> Deserialize<'de> for F64Rows {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(RowsVisitor)
    }
}

impl Serialize for F64Rows {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Inline(rows) => s.collect_seq(rows),
            Self::External(reference) => ExternalEnvelope {
                external: reference.clone(),
            }
            .serialize(s),
        }
    }
}

// ---------------------------------------------------------------------------
// Read context
// ---------------------------------------------------------------------------

thread_local! {
    static READ_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static WRITE_CTX: RefCell<Option<WriteContext>> = const { RefCell::new(None) };
}

struct ReadGuard(Option<PathBuf>);

impl ReadGuard {
    fn enter(dir: &Path) -> Self {
        Self(READ_DIR.with(|c| c.borrow_mut().replace(dir.to_path_buf())))
    }
}

impl Drop for ReadGuard {
    fn drop(&mut self) {
        READ_DIR.with(|c| *c.borrow_mut() = self.0.take());
    }
}

fn read_dir_or_err<E: serde::de::Error>(reference: &ExternalArrayRef) -> Result<PathBuf, E> {
    READ_DIR
        .with(|c| c.borrow().clone())
        .ok_or_else(|| E::custom(SidecarError::NoBaseDirectory(reference.path.clone())))
}

/// Directory used to resolve sidecars of a document stored at `document`.
#[must_use]
pub fn document_dir(document: &Path) -> PathBuf {
    match document.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Run `f` with `base_dir` installed as the directory external arrays
/// resolve against. For callers that keep their own error type around
/// `serde_json` deserialization; prefer [`from_slice_in`].
pub fn with_read_dir<R>(base_dir: &Path, f: impl FnOnce() -> R) -> R {
    let _guard = ReadGuard::enter(base_dir);
    f()
}

/// Deserialize `bytes` as `T`, resolving any external arrays relative to
/// `base_dir`.
pub fn from_slice_in<T: DeserializeOwned>(
    bytes: &[u8],
    base_dir: &Path,
) -> Result<T, SidecarError> {
    let _guard = ReadGuard::enter(base_dir);
    Ok(serde_json::from_slice(bytes)?)
}

/// Like [`from_slice_in`], with the base directory taken from the path the
/// bytes were read from.
pub fn from_slice_at<T: DeserializeOwned>(
    bytes: &[u8],
    document: &Path,
) -> Result<T, SidecarError> {
    from_slice_in(bytes, &document_dir(document))
}

/// Read `path`, deserialize it and resolve its sidecars.
pub fn load_json<T: DeserializeOwned>(path: &Path) -> Result<T, SidecarError> {
    let bytes = fs::read(path).map_err(|source| SidecarError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    from_slice_at(&bytes, path)
}

/// Every external array referenced anywhere in the JSON document `bytes`.
/// Documents that never mention `"external"` are not parsed.
pub fn external_refs(bytes: &[u8]) -> Result<Vec<ExternalArrayRef>, SidecarError> {
    const NEEDLE: &[u8] = b"\"external\"";
    if !bytes.windows(NEEDLE.len()).any(|w| w == NEEDLE) {
        return Ok(Vec::new());
    }
    fn walk(value: &serde_json::Value, out: &mut Vec<ExternalArrayRef>) {
        match value {
            serde_json::Value::Object(map) => {
                if map.len() == 1
                    && let Some(inner) = map.get("external")
                    && let Ok(reference) = serde_json::from_value::<ExternalArrayRef>(inner.clone())
                {
                    out.push(reference);
                    return;
                }
                map.values().for_each(|v| walk(v, out));
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let mut out = Vec::new();
    walk(&value, &mut out);
    Ok(out)
}

/// Verify (length and SHA-256) every sidecar referenced by the JSON file at
/// `document`; returns the references checked.
pub fn verify_document_sidecars(document: &Path) -> Result<Vec<ExternalArrayRef>, SidecarError> {
    let bytes = fs::read(document).map_err(|source| SidecarError::Io {
        path: document.to_path_buf(),
        source,
    })?;
    let refs = external_refs(&bytes)?;
    let dir = document_dir(document);
    for reference in &refs {
        reference.verify(&dir)?;
    }
    Ok(refs)
}

// ---------------------------------------------------------------------------
// Write context
// ---------------------------------------------------------------------------

struct WriteContext {
    dir: PathBuf,
    stem: String,
    overwrite: bool,
    min_values: i64,
    used: BTreeMap<&'static str, usize>,
}

fn threshold_from_env() -> Result<i64, SidecarError> {
    match std::env::var(SIDECAR_MIN_VALUES_ENV) {
        Ok(raw) => raw
            .trim()
            .parse::<i64>()
            .map_err(|_| SidecarError::InvalidThreshold(raw)),
        Err(_) => Ok(DEFAULT_SIDECAR_MIN_VALUES),
    }
}

struct WriteGuard(Option<WriteContext>);

impl Drop for WriteGuard {
    fn drop(&mut self) {
        WRITE_CTX.with(|c| *c.borrow_mut() = self.0.take());
    }
}

/// Run `f` (which serializes a document destined for `output`) with sidecar
/// writing enabled: arrays above the [`SIDECAR_MIN_VALUES_ENV`] threshold are
/// written to `<output-stem>.<field>.f64le` beside `output` and referenced
/// from the document. With `overwrite == false` an existing sidecar is an
/// error. Small arrays (every array of a small artifact) stay inline, so the
/// output is byte-identical to serializing without a context.
pub fn with_write_context<R>(
    output: &Path,
    overwrite: bool,
    f: impl FnOnce() -> R,
) -> Result<R, SidecarError> {
    with_write_context_min(output, overwrite, threshold_from_env()?, f)
}

/// [`with_write_context`] with an explicit threshold (`0` = always, negative
/// = never) instead of the environment's.
pub fn with_write_context_min<R>(
    output: &Path,
    overwrite: bool,
    min_values: i64,
    f: impl FnOnce() -> R,
) -> Result<R, SidecarError> {
    let stem = output.file_stem().map_or_else(
        || "artifact".to_string(),
        |s| s.to_string_lossy().into_owned(),
    );
    let ctx = WriteContext {
        dir: document_dir(output),
        stem,
        overwrite,
        min_values,
        used: BTreeMap::new(),
    };
    let previous = WRITE_CTX.with(|c| c.borrow_mut().replace(ctx));
    let _guard = WriteGuard(previous);
    Ok(f())
}

/// Serialize `value` as pretty JSON for `output`, writing any sidecars, and
/// return the document bytes (no trailing newline, like
/// `serde_json::to_vec_pretty`).
pub fn to_vec_pretty_for<T: Serialize>(
    value: &T,
    output: &Path,
    overwrite: bool,
) -> Result<Vec<u8>, SidecarError> {
    with_write_context(output, overwrite, || serde_json::to_vec_pretty(value))?
        .map_err(SidecarError::from)
}

fn should_externalize(min_values: i64, len: usize) -> bool {
    match min_values {
        m if m < 0 => false,
        0 => len > 0,
        m => len as u64 > m as u64,
    }
}

/// Write `chunks` (total `len` values) as a sidecar for `field` if the
/// active write context calls for it.
fn maybe_externalize<'a, I>(
    field: &'static str,
    len: usize,
    row_len: Option<usize>,
    chunks: I,
) -> Result<Option<ExternalArrayRef>, io::Error>
where
    I: Iterator<Item = &'a [f64]>,
{
    WRITE_CTX.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(ctx) = slot.as_mut() else {
            return Ok(None);
        };
        if !should_externalize(ctx.min_values, len) {
            return Ok(None);
        }
        let n = ctx.used.entry(field).or_insert(0);
        *n += 1;
        let name = if *n == 1 {
            format!("{}.{field}.f64le", ctx.stem)
        } else {
            format!("{}.{field}.{n}.f64le", ctx.stem)
        };
        let path = ctx.dir.join(&name);
        let mut options = fs::OpenOptions::new();
        options.write(true);
        if ctx.overwrite {
            options.create(true).truncate(true);
        } else {
            options.create_new(true);
        }
        let file = options
            .open(&path)
            .map_err(|e| io::Error::new(e.kind(), format!("sidecar {}: {e}", path.display())))?;
        let mut out = io::BufWriter::with_capacity(1 << 20, file);
        let mut digest = Sha256::new();
        let mut buffer = Vec::with_capacity((1 << 20) + 8);
        for chunk in chunks {
            for v in chunk {
                buffer.extend_from_slice(&v.to_le_bytes());
                if buffer.len() >= (1 << 20) {
                    digest.update(&buffer);
                    out.write_all(&buffer)?;
                    buffer.clear();
                }
            }
        }
        digest.update(&buffer);
        out.write_all(&buffer)?;
        out.flush()?;
        out.into_inner()
            .map_err(io::IntoInnerError::into_error)?
            .sync_all()?;
        Ok(Some(ExternalArrayRef {
            path: name,
            sha256: format!("{:x}", digest.finalize()),
            len,
            dtype: DTYPE_F64LE.into(),
            row_len,
        }))
    })
}

fn ser_flat<S: Serializer>(field: &'static str, values: &[f64], s: S) -> Result<S::Ok, S::Error> {
    match maybe_externalize(field, values.len(), None, std::iter::once(values))
        .map_err(S::Error::custom)?
    {
        Some(reference) => ExternalEnvelope {
            external: reference,
        }
        .serialize(s),
        None => {
            let mut seq = s.serialize_seq(Some(values.len()))?;
            for v in values {
                seq.serialize_element(v)?;
            }
            seq.end()
        }
    }
}

fn ser_rows<S: Serializer>(
    field: &'static str,
    rows: &[Vec<f64>],
    s: S,
) -> Result<S::Ok, S::Error> {
    let row_len = rows.first().map_or(0, Vec::len);
    let uniform = row_len > 0 && rows.iter().all(|r| r.len() == row_len);
    let external = if uniform {
        maybe_externalize(
            field,
            rows.len() * row_len,
            Some(row_len),
            rows.iter().map(Vec::as_slice),
        )
        .map_err(S::Error::custom)?
    } else {
        None
    };
    match external {
        Some(reference) => ExternalEnvelope {
            external: reference,
        }
        .serialize(s),
        None => {
            let mut seq = s.serialize_seq(Some(rows.len()))?;
            for row in rows {
                seq.serialize_element(row)?;
            }
            seq.end()
        }
    }
}

fn de_flat<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
    match F64Array::deserialize(d)? {
        F64Array::Inline(values) => Ok(values),
        F64Array::External(reference) => {
            let dir = read_dir_or_err::<D::Error>(&reference)?;
            reference.load(&dir).map_err(D::Error::custom)
        }
    }
}

fn de_rows<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<f64>>, D::Error> {
    match F64Rows::deserialize(d)? {
        F64Rows::Inline(rows) => Ok(rows),
        F64Rows::External(reference) => {
            let dir = read_dir_or_err::<D::Error>(&reference)?;
            F64Rows::External(reference)
                .resolve(&dir)
                .map_err(D::Error::custom)
        }
    }
}

/// `#[serde(with = "openbnct_core::sidecar::values")]` for a `Vec<f64>`
/// field named `values`.
pub mod values {
    use super::{Deserializer, Serializer, de_flat, ser_flat};
    pub fn serialize<S: Serializer>(v: &[f64], s: S) -> Result<S::Ok, S::Error> {
        ser_flat("values", v, s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
        de_flat(d)
    }
}

/// For a `Vec<f64>` field named `absolute_standard_uncertainty`.
pub mod uncertainty {
    use super::{Deserializer, Serializer, de_flat, ser_flat};
    pub fn serialize<S: Serializer>(v: &[f64], s: S) -> Result<S::Ok, S::Error> {
        ser_flat("absolute_standard_uncertainty", v, s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
        de_flat(d)
    }
}

/// For an `Option<Vec<f64>>` field named `absolute_standard_uncertainty`
/// (pair with `#[serde(default)]`).
pub mod opt_uncertainty {
    use super::{Deserializer, Serialize, Serializer, Visitor, de_flat, fmt, ser_flat};

    struct Flat<'a>(&'a [f64]);
    impl Serialize for Flat<'_> {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            ser_flat("absolute_standard_uncertainty", self.0, s)
        }
    }

    pub fn serialize<S: Serializer>(v: &Option<Vec<f64>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(values) => s.serialize_some(&Flat(values)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<f64>>, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Option<Vec<f64>>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("null, an array of numbers or a sidecar reference")
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
                de_flat(d).map(Some)
            }
        }
        d.deserialize_option(V)
    }
}

/// For the `Vec<Vec<f64>>` multigroup flux field named `flux`.
pub mod flux_rows {
    use super::{Deserializer, Serializer, de_rows, ser_rows};
    pub fn serialize<S: Serializer>(v: &[Vec<f64>], s: S) -> Result<S::Ok, S::Error> {
        ser_rows("flux", v, s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<f64>>, D::Error> {
        de_rows(d)
    }
}
