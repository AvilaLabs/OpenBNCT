// SPDX-License-Identifier: MIT
//! OpenPINT Excel treatment-workbook ingestion.
//!
//! OpenPINT's `PlanConfig.from_excel` encodes a treatment plan as a
//! multi-sheet `.xlsx` workbook:
//!
//! - `ct` — column `path`: one CT NIfTI.
//! - `GTV`, `CTV`, `PTV`, `HOM` — columns `path`, `boron`; each row is a
//!   binary mask NIfTI plus that structure's boron concentration.
//! - `OAR` — columns `path`, `boron`, `max_dose`, `mean_dose`.
//! - `bnct` — component key in the first column (`B10`, `N14`, `n`, `g`)
//!   and column `path` for the component dose NIfTI.
//! - `dose` — optional hadron courses: course name in the first column,
//!   columns `path`, `fractions`.
//!
//! A structure's name is its mask basename with the `.nii`/`.nii.gz`
//! suffix stripped — the convention OpenPINT's own loaders use. Sheets
//! that are absent are simply absent here too; `bnct` is the payload and
//! is required.
//!
//! The parser returns paths verbatim — relative paths resolve against
//! the workbook's directory at the call site, which owns the I/O.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::PathBuf;

use calamine::{Data, Reader, Xlsx, open_workbook_from_rs};

/// Structure-role sheets OpenPINT recognizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenPintRoiType {
    Gtv,
    Ctv,
    Ptv,
    /// Healthy-organ mask (non-limiting normal tissue in OpenPINT).
    Hom,
    /// Organ at risk — carries `max_dose`/`mean_dose` constraints.
    Oar,
}

impl OpenPintRoiType {
    fn from_sheet(sheet: &str) -> Option<Self> {
        match sheet {
            "GTV" => Some(Self::Gtv),
            "CTV" => Some(Self::Ctv),
            "PTV" => Some(Self::Ptv),
            "HOM" => Some(Self::Hom),
            "OAR" => Some(Self::Oar),
            _ => None,
        }
    }

    /// Sheet name as written in the workbook.
    #[must_use]
    pub fn sheet_name(self) -> &'static str {
        match self {
            Self::Gtv => "GTV",
            Self::Ctv => "CTV",
            Self::Ptv => "PTV",
            Self::Hom => "HOM",
            Self::Oar => "OAR",
        }
    }
}

/// One structure row from a role sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenPintStructure {
    /// Mask basename minus its `.nii`/`.nii.gz` suffix.
    pub name: String,
    pub roi_type: OpenPintRoiType,
    pub mask_path: PathBuf,
    /// Declared boron concentration for the structure (the workbook's
    /// `boron` column — OpenPINT does not enforce a unit).
    pub boron_conc: f64,
    /// `OAR` sheet only: limiting maximum dose.
    pub max_dose: Option<f64>,
    /// `OAR` sheet only: limiting mean dose.
    pub mean_dose: Option<f64>,
}

/// One optional hadron course from the `dose` sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenPintHadronCourse {
    pub name: String,
    pub dose_path: PathBuf,
    pub fractions: u32,
}

/// A parsed OpenPINT treatment workbook — paths are verbatim workbook
/// values, unresolvable until joined to the workbook's directory.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenPintPlan {
    /// `ct` sheet, first data row's `path`.
    pub ct_path: Option<PathBuf>,
    pub structures: Vec<OpenPintStructure>,
    /// `bnct` sheet: component key (`B10`, `N14`, `n`, `g`) → dose NIfTI.
    pub bnct_components: BTreeMap<String, PathBuf>,
    /// `dose` sheet: optional hadron courses.
    pub hadron_courses: Vec<OpenPintHadronCourse>,
}

/// Errors reading an OpenPINT workbook.
#[derive(Debug, thiserror::Error)]
pub enum OpenPintError {
    #[error(transparent)]
    Xlsx(#[from] calamine::XlsxError),
    #[error("workbook has no 'bnct' sheet — the component-dose table is the payload")]
    MissingBnctSheet,
    #[error("sheet {sheet:?} has no {column:?} column")]
    MissingColumn {
        sheet: &'static str,
        column: &'static str,
    },
    #[error("sheet {sheet:?} row {row}: empty path")]
    EmptyPath { sheet: &'static str, row: usize },
    #[error("sheet {sheet:?} row {row}: non-numeric {column:?} value {value:?}")]
    NonNumeric {
        sheet: &'static str,
        row: usize,
        column: &'static str,
        value: String,
    },
    #[error("sheet {sheet:?} row {row}: empty component/course key")]
    EmptyKey { sheet: &'static str, row: usize },
    #[error("sheet {sheet:?} row {row}: duplicate key {key:?}")]
    DuplicateKey {
        sheet: &'static str,
        row: usize,
        key: String,
    },
}

fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(text) => text.trim().to_owned(),
        other => other.to_string().trim().to_owned(),
    }
}

fn cell_number(
    cell: Option<&Data>,
    sheet: &'static str,
    row: usize,
    column: &'static str,
) -> Result<Option<f64>, OpenPintError> {
    let Some(cell) = cell else { return Ok(None) };
    match cell {
        Data::Empty => Ok(None),
        Data::Float(v) => Ok(Some(*v)),
        Data::Int(v) => Ok(Some(*v as f64)),
        Data::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            trimmed
                .parse::<f64>()
                .map(Some)
                .map_err(|_| OpenPintError::NonNumeric {
                    sheet,
                    row,
                    column,
                    value: trimmed.to_owned(),
                })
        }
        other => Err(OpenPintError::NonNumeric {
            sheet,
            row,
            column,
            value: other.to_string(),
        }),
    }
}

fn header_columns(range: &calamine::Range<Data>) -> (Vec<String>, Vec<Vec<Data>>) {
    let mut rows = range.rows();
    let header = rows
        .next()
        .map(|row| row.iter().map(cell_text).collect())
        .unwrap_or_default();
    (header, rows.map(|row| row.to_vec()).collect())
}

fn column_index(
    header: &[String],
    sheet: &'static str,
    column: &'static str,
) -> Result<Option<usize>, OpenPintError> {
    match header.iter().position(|h| h == column) {
        Some(index) => Ok(Some(index)),
        None if column == "path" => Err(OpenPintError::MissingColumn { sheet, column }),
        None => Ok(None),
    }
}

fn structure_name(path: &str) -> String {
    let base = PathBuf::from(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned());
    base.strip_suffix(".nii.gz")
        .or_else(|| base.strip_suffix(".nii"))
        .unwrap_or(&base)
        .to_owned()
}

/// Parse an OpenPINT `.xlsx` workbook from its raw bytes.
pub fn parse_openpint_workbook(bytes: &[u8]) -> Result<OpenPintPlan, OpenPintError> {
    let mut workbook: Xlsx<Cursor<&[u8]>> = open_workbook_from_rs(Cursor::new(bytes))?;
    let sheets: Vec<String> = workbook.sheet_names().to_vec();

    let mut plan = OpenPintPlan {
        ct_path: None,
        structures: Vec::new(),
        bnct_components: BTreeMap::new(),
        hadron_courses: Vec::new(),
    };

    if sheets.iter().any(|s| s == "ct") {
        let range = workbook.worksheet_range("ct")?;
        let (header, rows) = header_columns(&range);
        // `ct` is optional metadata — a missing `path` column or an empty
        // sheet yields `ct_path = None` rather than a hard failure.
        if let Some(path_col) = header.iter().position(|h| h == "path")
            && let Some(row) = rows.first()
        {
            let value = row.get(path_col).map(cell_text).unwrap_or_default();
            if !value.is_empty() {
                plan.ct_path = Some(PathBuf::from(value));
            }
        }
    }

    for sheet in ["GTV", "CTV", "PTV", "HOM", "OAR"] {
        let roi_type = OpenPintRoiType::from_sheet(sheet).unwrap();
        if !sheets.iter().any(|s| s == sheet) {
            continue;
        }
        let range = workbook.worksheet_range(sheet)?;
        let (header, rows) = header_columns(&range);
        let path_col =
            column_index(&header, sheet, "path")?.ok_or(OpenPintError::MissingColumn {
                sheet,
                column: "path",
            })?;
        let boron_col = column_index(&header, sheet, "boron")?;
        let max_col = column_index(&header, sheet, "max_dose")?;
        let mean_col = column_index(&header, sheet, "mean_dose")?;
        for (index, row) in rows.iter().enumerate() {
            let path = row.get(path_col).map(cell_text).unwrap_or_default();
            if path.is_empty() {
                continue;
            }
            let data_row = index + 2;
            let boron = cell_number(
                row.get(boron_col.unwrap_or(usize::MAX)),
                sheet,
                data_row,
                "boron",
            )?
            .unwrap_or(0.0);
            let max_dose = cell_number(
                row.get(max_col.unwrap_or(usize::MAX)),
                sheet,
                data_row,
                "max_dose",
            )?;
            let mean_dose = cell_number(
                row.get(mean_col.unwrap_or(usize::MAX)),
                sheet,
                data_row,
                "mean_dose",
            )?;
            plan.structures.push(OpenPintStructure {
                name: structure_name(&path),
                roi_type,
                mask_path: PathBuf::from(path),
                boron_conc: boron,
                max_dose,
                mean_dose,
            });
        }
    }

    if !sheets.iter().any(|s| s == "bnct") {
        return Err(OpenPintError::MissingBnctSheet);
    }
    let range = workbook.worksheet_range("bnct")?;
    let (header, rows) = header_columns(&range);
    let path_col = column_index(&header, "bnct", "path")?.ok_or(OpenPintError::MissingColumn {
        sheet: "bnct",
        column: "path",
    })?;
    for (index, row) in rows.iter().enumerate() {
        let data_row = index + 2;
        let key = row.first().map(cell_text).unwrap_or_default();
        if key.is_empty() && row.iter().all(|c| cell_text(c).is_empty()) {
            continue;
        }
        if key.is_empty() {
            return Err(OpenPintError::EmptyKey {
                sheet: "bnct",
                row: data_row,
            });
        }
        let path = row.get(path_col).map(cell_text).unwrap_or_default();
        if path.is_empty() {
            return Err(OpenPintError::EmptyPath {
                sheet: "bnct",
                row: data_row,
            });
        }
        if plan
            .bnct_components
            .insert(key.clone(), PathBuf::from(path))
            .is_some()
        {
            return Err(OpenPintError::DuplicateKey {
                sheet: "bnct",
                row: data_row,
                key,
            });
        }
    }

    if sheets.iter().any(|s| s == "dose") {
        let range = workbook.worksheet_range("dose")?;
        let (header, rows) = header_columns(&range);
        let path_col =
            column_index(&header, "dose", "path")?.ok_or(OpenPintError::MissingColumn {
                sheet: "dose",
                column: "path",
            })?;
        let frac_col = column_index(&header, "dose", "fractions")?;
        for (index, row) in rows.iter().enumerate() {
            let data_row = index + 2;
            let name = row.first().map(cell_text).unwrap_or_default();
            if name.is_empty() && row.iter().all(|c| cell_text(c).is_empty()) {
                continue;
            }
            let path = row.get(path_col).map(cell_text).unwrap_or_default();
            if path.is_empty() {
                return Err(OpenPintError::EmptyPath {
                    sheet: "dose",
                    row: data_row,
                });
            }
            let fractions = cell_number(
                row.get(frac_col.unwrap_or(usize::MAX)),
                "dose",
                data_row,
                "fractions",
            )?
            .map(|f| f.max(0.0).round() as u32)
            .unwrap_or(1);
            plan.hadron_courses.push(OpenPintHadronCourse {
                name,
                dose_path: PathBuf::from(path),
                fractions,
            });
        }
    }

    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_xlsxwriter::Workbook;

    fn workbook_bytes(build: impl Fn(&mut Workbook)) -> Vec<u8> {
        let mut workbook = Workbook::new();
        build(&mut workbook);
        workbook.save_to_buffer().unwrap()
    }

    fn fixture() -> Vec<u8> {
        workbook_bytes(|wb| {
            let ct = wb.add_worksheet().set_name("ct").unwrap();
            ct.write_string(0, 0, "path").unwrap();
            ct.write_string(1, 0, "data/case/ct.nii.gz").unwrap();

            let gtv = wb.add_worksheet().set_name("GTV").unwrap();
            gtv.write_string(0, 0, "path").unwrap();
            gtv.write_string(0, 1, "boron").unwrap();
            gtv.write_string(1, 0, "data/case/seg_GTV.nii.gz").unwrap();
            gtv.write_number(1, 1, 30.0).unwrap();

            let oar = wb.add_worksheet().set_name("OAR").unwrap();
            oar.write_string(0, 0, "path").unwrap();
            oar.write_string(0, 1, "boron").unwrap();
            oar.write_string(0, 2, "max_dose").unwrap();
            oar.write_string(0, 3, "mean_dose").unwrap();
            oar.write_string(1, 0, "data/case/seg_brain.nii.gz")
                .unwrap();
            oar.write_number(1, 1, 8.0).unwrap();
            oar.write_number(1, 2, 12.0).unwrap();
            oar.write_number(1, 3, 6.0).unwrap();

            let bnct = wb.add_worksheet().set_name("bnct").unwrap();
            bnct.write_string(0, 1, "path").unwrap();
            for (row, (key, path)) in [
                ("B10", "dose_B10.nii.gz"),
                ("N14", "dose_N14.nii.gz"),
                ("n", "dose_n.nii.gz"),
                ("g", "dose_g.nii.gz"),
            ]
            .into_iter()
            .enumerate()
            {
                bnct.write_string(row as u32 + 1, 0, key).unwrap();
                bnct.write_string(row as u32 + 1, 1, path).unwrap();
            }
        })
    }

    #[test]
    fn parses_full_workbook() {
        let plan = parse_openpint_workbook(&fixture()).unwrap();
        assert_eq!(plan.ct_path, Some(PathBuf::from("data/case/ct.nii.gz")));
        assert_eq!(plan.structures.len(), 2);

        let gtv = &plan.structures[0];
        assert_eq!(gtv.name, "seg_GTV");
        assert_eq!(gtv.roi_type, OpenPintRoiType::Gtv);
        assert_eq!(gtv.boron_conc, 30.0);
        assert_eq!(gtv.max_dose, None);

        let oar = &plan.structures[1];
        assert_eq!(oar.roi_type, OpenPintRoiType::Oar);
        assert_eq!(oar.max_dose, Some(12.0));
        assert_eq!(oar.mean_dose, Some(6.0));

        assert_eq!(plan.bnct_components.len(), 4);
        assert_eq!(
            plan.bnct_components["B10"],
            PathBuf::from("dose_B10.nii.gz")
        );
        assert!(plan.hadron_courses.is_empty());
    }

    #[test]
    fn missing_bnct_sheet_is_an_error() {
        let bytes = workbook_bytes(|wb| {
            wb.add_worksheet().set_name("ct").unwrap();
        });
        assert!(matches!(
            parse_openpint_workbook(&bytes),
            Err(OpenPintError::MissingBnctSheet)
        ));
    }

    #[test]
    fn absent_optional_sheets_are_tolerated() {
        let bytes = workbook_bytes(|wb| {
            let bnct = wb.add_worksheet().set_name("bnct").unwrap();
            bnct.write_string(0, 1, "path").unwrap();
            bnct.write_string(1, 0, "B10").unwrap();
            bnct.write_string(1, 1, "b10.nii").unwrap();
        });
        let plan = parse_openpint_workbook(&bytes).unwrap();
        assert_eq!(plan.bnct_components.len(), 1);
        assert!(plan.structures.is_empty());
        assert_eq!(plan.ct_path, None);
    }

    #[test]
    fn duplicate_component_key_rejected() {
        let bytes = workbook_bytes(|wb| {
            let bnct = wb.add_worksheet().set_name("bnct").unwrap();
            bnct.write_string(0, 1, "path").unwrap();
            bnct.write_string(1, 0, "B10").unwrap();
            bnct.write_string(1, 1, "a.nii").unwrap();
            bnct.write_string(2, 0, "B10").unwrap();
            bnct.write_string(2, 1, "b.nii").unwrap();
        });
        assert!(matches!(
            parse_openpint_workbook(&bytes),
            Err(OpenPintError::DuplicateKey { .. })
        ));
    }

    #[test]
    fn empty_rows_and_blank_cells_are_skipped() {
        let bytes = workbook_bytes(|wb| {
            let bnct = wb.add_worksheet().set_name("bnct").unwrap();
            bnct.write_string(0, 1, "path").unwrap();
            bnct.write_string(1, 0, "B10").unwrap();
            bnct.write_string(1, 1, "b10.nii").unwrap();
            // row 3 entirely empty — skipped
            let gtv = wb.add_worksheet().set_name("GTV").unwrap();
            gtv.write_string(0, 0, "path").unwrap();
            gtv.write_string(0, 1, "boron").unwrap();
            gtv.write_string(1, 0, "mask.nii.gz").unwrap();
            // boron blank → defaults to 0.0
        });
        let plan = parse_openpint_workbook(&bytes).unwrap();
        assert_eq!(plan.structures[0].boron_conc, 0.0);
        assert_eq!(plan.structures[0].name, "mask");
    }
}
