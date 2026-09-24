// SPDX-License-Identifier: MIT

//! Tabular CSV/XLSX interchange for `openbnct.exposure-plan` documents
//! plus deterministic exposure-weight optimization.
//!
//! Spreadsheet tooling is where many research irradiation schedules are
//! authored. This crate round-trips an [`ExposurePlan`] between its JSON
//! contract and two table formats carrying identical semantics:
//!
//! - **CSV**: `# key: value` metadata lines (`format`, `id`, `case_id`,
//!   `covariance`) followed by a header row and one row per exposure.
//! - **XLSX**: a `plan` key/value sheet plus an `exposures` sheet with a
//!   header row and one row per exposure.
//!
//! Columns: `name`, `dose_bundle_path`, `dose_bundle_id`,
//! `dose_bundle_sha256`, `weight`, `weight_basis`, `duration_s`,
//! `boron_assumption`. Blank `dose_bundle_id` cells default to the path
//! stem; blank `dose_bundle_sha256` cells are allowed only when
//! [`TableImportOptions::bundles_dir`] is given, in which case each file is
//! hashed on import. Unknown columns are rejected so spreadsheet typos
//! cannot silently drop data.
//!
//! The [`optimize`] module solves non-negative exposure weights against
//! `openbnct.inverse-plan-objective` dose-volume objectives by
//! projected-gradient descent — a research optimizer qualified
//! `inverse_planning_research_only_not_clinical`, not a commissioned
//! treatment-planning product.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use openbnct_core::{
    BoundFileReference, Exposure, ExposureCovariance, ExposurePlan, ExposurePlanError,
    PhysicalDoseBundle, WeightBasis,
};
use thiserror::Error;

pub mod directions;
pub mod fields;
pub mod openpint;
pub mod optimize;
pub mod robustness;
pub mod scenarios;

/// Format token written into exported tables and required on import when
/// metadata is present.
pub const TABLE_FORMAT: &str = "openbnct.exposure-plan-table/1";

/// Sheet carrying plan-level key/value metadata in XLSX workbooks.
pub const PLAN_SHEET: &str = "plan";
/// Sheet carrying the exposure rows in XLSX workbooks.
pub const EXPOSURES_SHEET: &str = "exposures";

const COLUMNS: [&str; 8] = [
    "name",
    "dose_bundle_path",
    "dose_bundle_id",
    "dose_bundle_sha256",
    "weight",
    "weight_basis",
    "duration_s",
    "boron_assumption",
];

/// Caller-supplied plan identity used when the table does not carry it (or
/// to override it).
#[derive(Debug, Default, Clone)]
pub struct TableImportOptions {
    pub id: Option<String>,
    pub case_id: Option<String>,
    /// Directory used to hash `dose_bundle_path` files whose `sha256` cell
    /// is blank. Without it, blank hashes are a per-row error.
    pub bundles_dir: Option<PathBuf>,
}

#[derive(Debug)]
pub struct RowIssue {
    /// 1-based data-row number (header excluded), matching the spreadsheet
    /// row for sheets whose header occupies row 1.
    pub row: usize,
    pub message: String,
}

impl std::fmt::Display for RowIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "row {}: {}", self.row, self.message)
    }
}

#[derive(Debug, Error)]
pub enum TableError {
    #[error("unsupported table extension for {0:?}; expected .csv or .xlsx")]
    UnsupportedExtension(PathBuf),
    #[error("table I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("CSV parse error: {0}")]
    Csv(#[from] csv::Error),
    #[error("XLSX parse error: {0}")]
    Xlsx(#[from] calamine::XlsxError),
    #[error("XLSX write error: {0}")]
    XlsxWrite(#[from] rust_xlsxwriter::XlsxError),
    #[error("workbook has no {0:?} sheet")]
    MissingSheet(&'static str),
    #[error("unsupported table format token {0:?}; expected {TABLE_FORMAT:?}")]
    FormatToken(String),
    #[error("header is missing column(s): {0}")]
    MissingColumns(String),
    #[error("header carries unknown column(s): {0}")]
    UnknownColumns(String),
    #[error("malformed exposure rows:\n{}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"))]
    Rows(Vec<RowIssue>),
    #[error("metadata: {0}")]
    Metadata(String),
    #[error("imported plan failed validation: {0}")]
    Plan(#[from] ExposurePlanError),
}

/// Serialize `plan` to the CSV table text.
pub fn exposures_to_csv(plan: &ExposurePlan) -> String {
    let mut out = String::new();
    out.push_str(&format!("# format: {TABLE_FORMAT}\n"));
    out.push_str(&format!("# id: {}\n", plan.id));
    out.push_str(&format!("# case_id: {}\n", plan.case_id));
    out.push_str(&format!(
        "# covariance: {}\n",
        covariance_token(plan.covariance)
    ));
    out.push_str(&COLUMNS.join(","));
    out.push('\n');
    for exposure in &plan.exposures {
        let cells = exposure_cells(exposure);
        out.push_str(
            &cells
                .iter()
                .map(|cell| csv_escape(cell))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    out
}

/// Serialize `plan` to an in-memory XLSX workbook.
pub fn exposures_to_xlsx(plan: &ExposurePlan) -> Result<Vec<u8>, TableError> {
    use rust_xlsxwriter::Workbook;
    let mut workbook = Workbook::new();
    {
        let sheet = workbook.add_worksheet().set_name(PLAN_SHEET)?;
        for (row, (key, value)) in [
            ("format", TABLE_FORMAT.to_owned()),
            ("id", plan.id.clone()),
            ("case_id", plan.case_id.clone()),
            ("covariance", covariance_token(plan.covariance)),
        ]
        .iter()
        .enumerate()
        {
            sheet.write_string(row as u32, 0, *key)?;
            sheet.write_string(row as u32, 1, value)?;
        }
    }
    let sheet = workbook.add_worksheet().set_name(EXPOSURES_SHEET)?;
    for (column, name) in COLUMNS.iter().enumerate() {
        sheet.write_string(0, column as u16, *name)?;
    }
    for (row, exposure) in plan.exposures.iter().enumerate() {
        for (column, cell) in exposure_cells(exposure).iter().enumerate() {
            sheet.write_string((row + 1) as u32, column as u16, cell)?;
        }
    }
    Ok(workbook.save_to_buffer()?)
}

/// Parse a CSV table into a validated `ExposurePlan`.
pub fn exposures_from_csv(
    text: &str,
    options: &TableImportOptions,
) -> Result<ExposurePlan, TableError> {
    let mut metadata = BTreeMap::new();
    let mut body = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('#') {
            if let Some((key, value)) = rest.split_once(':') {
                metadata.insert(key.trim().to_owned(), value.trim().to_owned());
            }
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(Cursor::new(body));
    let header: Vec<String> = reader
        .headers()
        .map_err(TableError::Csv)?
        .iter()
        .map(|h| h.to_owned())
        .collect();
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(TableError::Csv)?;
        rows.push(record.iter().map(|c| c.to_owned()).collect::<Vec<_>>());
    }
    grid_to_plan(&metadata, &header, &rows, options)
}

/// Parse an XLSX workbook into a validated `ExposurePlan`.
pub fn exposures_from_xlsx(
    bytes: &[u8],
    options: &TableImportOptions,
) -> Result<ExposurePlan, TableError> {
    use calamine::{Reader, Xlsx, open_workbook_from_rs};
    let mut workbook: Xlsx<Cursor<&[u8]>> = open_workbook_from_rs(Cursor::new(bytes))?;
    let mut metadata = BTreeMap::new();
    if workbook.sheet_names().iter().any(|n| n == PLAN_SHEET) {
        let range = workbook.worksheet_range(PLAN_SHEET)?;
        for row in range.rows() {
            if let [key, value, ..] = row {
                metadata.insert(
                    key.to_string().trim().to_owned(),
                    value.to_string().trim().to_owned(),
                );
            }
        }
    }
    let range = workbook
        .worksheet_range(EXPOSURES_SHEET)
        .map_err(|_| TableError::MissingSheet(EXPOSURES_SHEET))?;
    let mut rows_iter = range.rows();
    let header: Vec<String> = rows_iter
        .next()
        .map(|row| row.iter().map(cell_text).collect())
        .ok_or_else(|| TableError::Metadata("exposures sheet is empty".into()))?;
    let rows: Vec<Vec<String>> = rows_iter
        .map(|row| row.iter().map(cell_text).collect())
        .collect();
    grid_to_plan(&metadata, &header, &rows, options)
}

fn cell_text(cell: &calamine::Data) -> String {
    match cell {
        calamine::Data::Empty => String::new(),
        calamine::Data::String(text) => text.trim().to_owned(),
        other => other.to_string().trim().to_owned(),
    }
}

/// Read a `.csv` or `.xlsx` table into a validated `ExposurePlan`.
pub fn read_table(path: &Path, options: &TableImportOptions) -> Result<ExposurePlan, TableError> {
    match extension(path)?.as_str() {
        "csv" => exposures_from_csv(&fs::read_to_string(path)?, options),
        "xlsx" => exposures_from_xlsx(&fs::read(path)?, options),
        _ => unreachable!(),
    }
}

/// Write `plan` as `.csv` or `.xlsx` selected by `path`'s extension.
/// Refuses to overwrite an existing file.
pub fn write_table(path: &Path, plan: &ExposurePlan) -> Result<(), TableError> {
    if path.exists() {
        return Err(TableError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", path.display()),
        )));
    }
    match extension(path)?.as_str() {
        "csv" => Ok(fs::write(path, exposures_to_csv(plan))?),
        "xlsx" => Ok(fs::write(path, exposures_to_xlsx(plan)?)?),
        _ => unreachable!(),
    }
}

fn extension(path: &Path) -> Result<String, TableError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if ext == "csv" || ext == "xlsx" {
        Ok(ext)
    } else {
        Err(TableError::UnsupportedExtension(path.to_path_buf()))
    }
}

fn exposure_cells(exposure: &Exposure) -> Vec<String> {
    vec![
        exposure.name.clone(),
        exposure.dose_bundle.path.clone(),
        exposure.dose_bundle.id.clone(),
        exposure.dose_bundle.sha256.clone(),
        exposure.weight.to_string(),
        basis_token(exposure.weight_basis),
        exposure
            .duration_s
            .map(|d| d.to_string())
            .unwrap_or_default(),
        exposure.boron_assumption.clone().unwrap_or_default(),
    ]
}

fn csv_escape(cell: &str) -> String {
    if cell.contains([',', '"', '\n']) || cell.starts_with(' ') {
        format!("\"{}\"", cell.replace('"', "\"\""))
    } else {
        cell.to_owned()
    }
}

fn basis_token(basis: WeightBasis) -> String {
    serde_json::to_value(basis)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{basis:?}").to_lowercase())
}

fn covariance_token(covariance: ExposureCovariance) -> String {
    serde_json::to_value(covariance)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{covariance:?}").to_lowercase())
}

fn parse_basis(token: &str) -> Option<WeightBasis> {
    serde_json::from_value(serde_json::Value::String(token.to_owned())).ok()
}

fn parse_covariance(token: &str) -> Option<ExposureCovariance> {
    serde_json::from_value(serde_json::Value::String(token.to_owned())).ok()
}

fn grid_to_plan(
    metadata: &BTreeMap<String, String>,
    header: &[String],
    rows: &[Vec<String>],
    options: &TableImportOptions,
) -> Result<ExposurePlan, TableError> {
    if let Some(format) = metadata.get("format")
        && format != TABLE_FORMAT
    {
        return Err(TableError::FormatToken(format.clone()));
    }

    let header_set: BTreeSet<&str> = header.iter().map(String::as_str).collect();
    let known: BTreeSet<&str> = COLUMNS.iter().copied().collect();
    let missing: Vec<&str> = ["name", "dose_bundle_path", "weight", "weight_basis"]
        .iter()
        .copied()
        .filter(|c| !header_set.contains(c))
        .collect();
    if !missing.is_empty() {
        return Err(TableError::MissingColumns(missing.join(", ")));
    }
    let unknown: Vec<&str> = header_set.difference(&known).copied().collect();
    if !unknown.is_empty() {
        return Err(TableError::UnknownColumns(unknown.join(", ")));
    }
    let index_of = |name: &str| header.iter().position(|h| h == name);

    let mut issues = Vec::new();
    let mut exposures = Vec::new();
    for (offset, row) in rows.iter().enumerate() {
        let row_number = offset + 1;
        if row.iter().all(|cell| cell.trim().is_empty()) {
            continue;
        }
        let cell = |column: &str| -> String {
            index_of(column)
                .and_then(|index| row.get(index))
                .map(|c| c.trim().to_owned())
                .unwrap_or_default()
        };
        let name = cell("name");
        let path = cell("dose_bundle_path");
        if name.is_empty() || path.is_empty() {
            issues.push(RowIssue {
                row: row_number,
                message: "name and dose_bundle_path are required".into(),
            });
            continue;
        }
        let weight = cell("weight");
        let weight = match weight.parse::<f64>() {
            Ok(value) if value.is_finite() && value >= 0.0 => value,
            Ok(_) | Err(_) => {
                issues.push(RowIssue {
                    row: row_number,
                    message: format!("weight {weight:?} is not a finite non-negative number"),
                });
                continue;
            }
        };
        let basis_token = cell("weight_basis");
        let weight_basis = match parse_basis(&basis_token) {
            Some(basis) => basis,
            None => {
                issues.push(RowIssue {
                    row: row_number,
                    message: format!(
                        "weight_basis {basis_token:?} is not one of delivered_fraction, source_strength_scaling, delivered_histories, manual"
                    ),
                });
                continue;
            }
        };
        let duration_s = match cell("duration_s").as_str() {
            "" => None,
            text => match text.parse::<f64>() {
                Ok(value) if value.is_finite() && value > 0.0 => Some(value),
                _ => {
                    issues.push(RowIssue {
                        row: row_number,
                        message: format!("duration_s {text:?} is not a positive number"),
                    });
                    continue;
                }
            },
        };
        let id = match cell("dose_bundle_id").as_str() {
            "" => Path::new(&path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone()),
            id => id.to_owned(),
        };
        let sha256 = match cell("dose_bundle_sha256").as_str() {
            "" => match &options.bundles_dir {
                Some(dir) => match hash_bundle(&dir.join(&path)) {
                    Ok(hash) => hash,
                    Err(message) => {
                        issues.push(RowIssue {
                            row: row_number,
                            message,
                        });
                        continue;
                    }
                },
                None => {
                    issues.push(RowIssue {
                        row: row_number,
                        message: "dose_bundle_sha256 is blank; supply --bundles-dir to hash files on import".into(),
                    });
                    continue;
                }
            },
            hash => hash.to_owned(),
        };
        exposures.push(Exposure {
            name,
            dose_bundle: BoundFileReference { id, sha256, path },
            weight,
            weight_basis,
            duration_s,
            boron_assumption: match cell("boron_assumption").as_str() {
                "" => None,
                text => Some(text.to_owned()),
            },
        });
    }
    if !issues.is_empty() {
        return Err(TableError::Rows(issues));
    }
    if exposures.is_empty() {
        return Err(TableError::Metadata(
            "table contains no exposure rows".into(),
        ));
    }

    let id = options
        .id
        .clone()
        .or_else(|| metadata.get("id").cloned())
        .unwrap_or_default();
    let case_id = options
        .case_id
        .clone()
        .or_else(|| metadata.get("case_id").cloned())
        .unwrap_or_default();
    if id.is_empty() || case_id.is_empty() {
        return Err(TableError::Metadata(
            "plan id/case_id missing; add '# id:'/'# case_id:' metadata or pass --id/--case-id"
                .into(),
        ));
    }
    let covariance = match metadata.get("covariance") {
        Some(token) => parse_covariance(token)
            .ok_or_else(|| TableError::Metadata(format!("covariance {token:?} is unsupported")))?,
        None => ExposureCovariance::IndependentExposures,
    };
    let plan = ExposurePlan {
        schema_version: openbnct_core::EXPOSURE_PLAN_SCHEMA.into(),
        id,
        case_id,
        covariance,
        exposures,
    };
    plan.validate()?;
    Ok(plan)
}

/// Errors from running a saved exposure plan end to end.
#[derive(Debug, Error)]
pub enum PlanRunError {
    #[error("plan I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("plan JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("plan validation: {0}")]
    Plan(#[from] ExposurePlanError),
    #[error(
        "exposure {exposure:?} bundle {} sha256 mismatch (plan {expected}, actual {actual})",
        path.display()
    )]
    HashMismatch {
        exposure: String,
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("exposure {exposure} bundle JSON: {error}")]
    BundleJson {
        exposure: String,
        error: serde_json::Error,
    },
}

/// Run a saved `openbnct.exposure-plan/0.1.0` file end to end: verify every
/// bound dose bundle's recorded SHA-256 against the file on disk (resolved
/// relative to the plan file's directory), then accumulate the weighted
/// exposures into one physical dose bundle.
pub fn accumulate_plan_file(plan_path: &Path) -> Result<PhysicalDoseBundle, PlanRunError> {
    let plan_bytes = fs::read(plan_path)?;
    let plan_sha256 = sha256_hex(&plan_bytes);
    let plan: ExposurePlan = serde_json::from_slice(&plan_bytes)?;
    plan.validate()?;
    let plan_dir = plan_path.parent().unwrap_or(Path::new("."));
    let mut bundles = Vec::with_capacity(plan.exposures.len());
    for exposure in &plan.exposures {
        let path = plan_dir.join(&exposure.dose_bundle.path);
        let bytes = fs::read(&path)?;
        let actual = sha256_hex(&bytes);
        if actual != exposure.dose_bundle.sha256 {
            return Err(PlanRunError::HashMismatch {
                exposure: exposure.name.clone(),
                path,
                expected: exposure.dose_bundle.sha256.clone(),
                actual,
            });
        }
        bundles.push(
            serde_json::from_slice::<PhysicalDoseBundle>(&bytes).map_err(|error| {
                PlanRunError::BundleJson {
                    exposure: exposure.name.clone(),
                    error,
                }
            })?,
        );
    }
    Ok(openbnct_core::accumulate_exposures(
        &plan,
        &plan_sha256,
        &bundles,
    )?)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

fn hash_bundle(path: &Path) -> Result<String, String> {
    use sha2::Digest;
    let bytes =
        fs::read(path).map_err(|e| format!("cannot read bundle {}: {e}", path.display()))?;
    Ok(format!("{:x}", sha2::Sha256::digest(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> ExposurePlan {
        ExposurePlan {
            schema_version: openbnct_core::EXPOSURE_PLAN_SCHEMA.into(),
            id: "openbnct.test.plan.v1".into(),
            case_id: "case-a".into(),
            covariance: ExposureCovariance::IndependentExposures,
            exposures: vec![
                Exposure {
                    name: "field-a".into(),
                    dose_bundle: BoundFileReference {
                        id: "bundle-a".into(),
                        sha256: "a".repeat(64),
                        path: "bundle-a.json".into(),
                    },
                    weight: 1.0,
                    weight_basis: WeightBasis::DeliveredFraction,
                    duration_s: Some(600.0),
                    boron_assumption: Some("10 ppm B-10".into()),
                },
                Exposure {
                    name: "field-b".into(),
                    dose_bundle: BoundFileReference {
                        id: "bundle-b".into(),
                        sha256: "b".repeat(64),
                        path: "nested/bundle-b.json".into(),
                    },
                    weight: 0.5,
                    weight_basis: WeightBasis::Manual,
                    duration_s: None,
                    boron_assumption: None,
                },
            ],
        }
    }

    #[test]
    fn csv_round_trip_preserves_every_field() {
        let text = exposures_to_csv(&plan());
        let parsed = exposures_from_csv(&text, &TableImportOptions::default()).unwrap();
        assert_eq!(parsed, plan());
    }

    #[test]
    fn xlsx_round_trip_preserves_every_field() {
        let bytes = exposures_to_xlsx(&plan()).unwrap();
        let parsed = exposures_from_xlsx(&bytes, &TableImportOptions::default()).unwrap();
        assert_eq!(parsed, plan());
    }

    #[test]
    fn csv_escapes_commas_and_quotes() {
        let mut plan = plan();
        plan.exposures[0].boron_assumption = Some("10 ppm, compound \"A\"".into());
        let text = exposures_to_csv(&plan);
        let parsed = exposures_from_csv(&text, &TableImportOptions::default()).unwrap();
        assert_eq!(
            parsed.exposures[0].boron_assumption.as_deref(),
            Some("10 ppm, compound \"A\"")
        );
    }

    #[test]
    fn reports_all_bad_rows_with_row_numbers() {
        let text = "name,dose_bundle_path,dose_bundle_id,dose_bundle_sha256,weight,weight_basis\n\
            good,a.json,id-a,aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,1.0,delivered_fraction\n\
            badw,b.json,id-b,bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,abc,manual\n\
            badb,c.json,id-c,cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc,1.0,bogus\n";
        let options = TableImportOptions {
            id: Some("p".into()),
            case_id: Some("c".into()),
            ..Default::default()
        };
        match exposures_from_csv(text, &options) {
            Err(TableError::Rows(issues)) => {
                assert_eq!(issues.len(), 2);
                assert_eq!(issues[0].row, 2);
                assert!(issues[0].message.contains("weight"));
                assert_eq!(issues[1].row, 3);
                assert!(issues[1].message.contains("weight_basis"));
            }
            other => panic!("expected row issues, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_and_missing_columns() {
        let options = TableImportOptions {
            id: Some("p".into()),
            case_id: Some("c".into()),
            ..Default::default()
        };
        let missing = "name,weight\na,1.0\n";
        assert!(matches!(
            exposures_from_csv(missing, &options),
            Err(TableError::MissingColumns(_))
        ));
        let unknown = "name,dose_bundle_path,weight,weight_basis,weght\ngood,a.json,1.0,manual,2\n";
        assert!(matches!(
            exposures_from_csv(unknown, &options),
            Err(TableError::UnknownColumns(_))
        ));
    }

    #[test]
    fn blank_sha256_needs_bundles_dir_then_hashes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.json"), b"bundle bytes").unwrap();
        let text = "name,dose_bundle_path,weight,weight_basis\nf-a,a.json,1.0,manual\n";
        let options = TableImportOptions {
            id: Some("p".into()),
            case_id: Some("c".into()),
            ..Default::default()
        };
        assert!(matches!(
            exposures_from_csv(text, &options),
            Err(TableError::Rows(_))
        ));
        let hashed = exposures_from_csv(
            text,
            &TableImportOptions {
                bundles_dir: Some(dir.path().to_path_buf()),
                ..options
            },
        )
        .unwrap();
        let reference = &hashed.exposures[0].dose_bundle;
        assert_eq!(reference.sha256.len(), 64);
        assert_eq!(reference.id, "a"); // stem-derived id
        assert_eq!(reference.path, "a.json");
    }

    #[test]
    fn table_without_metadata_uses_option_fallbacks() {
        let text = "name,dose_bundle_path,dose_bundle_id,dose_bundle_sha256,weight,weight_basis\n\
            f-a,a.json,id-a,aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,1.0,manual\n";
        let options = TableImportOptions {
            id: Some("plan-1".into()),
            case_id: Some("case-9".into()),
            ..Default::default()
        };
        let parsed = exposures_from_csv(text, &options).unwrap();
        assert_eq!(parsed.id, "plan-1");
        assert_eq!(parsed.case_id, "case-9");
        assert!(matches!(
            exposures_from_csv(text, &TableImportOptions::default()),
            Err(TableError::Metadata(_))
        ));
    }
}
