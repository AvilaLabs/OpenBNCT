// SPDX-License-Identifier: Apache-2.0

//! Strict DICOM import and synthetic benchmark support.

#![forbid(unsafe_code)]

mod benchmark;
mod ct;
mod error;
mod mr;
mod pet;
mod rtdose;
mod rtplan;
mod rtstruct;
mod study;
pub mod synthetic;

pub use benchmark::{
    BenchmarkReport, RoiReport, VerifiedBenchmarkCase, load_nf_bnct_001,
    load_nf_bnct_001_from_files, verify_nf_bnct_001,
};
pub use ct::{CtVolume, import_ct_series, import_ct_series_from_bytes};
pub use error::{DicomError, Result};
pub use mr::{MrVolume, import_mr_series, import_mr_series_from_bytes};
pub use pet::{PetVolume, import_pet_series, import_pet_series_from_bytes};
pub use rtdose::{DoseSelection, RtDoseExportOptions, RtDoseExportResult, export_rt_dose};
pub use rtplan::{
    RTPLAN_SUMMARY_SCHEMA, RtPlanBeamSpec, RtPlanBeamSummary, RtPlanControlPoint,
    RtPlanExportOptions, RtPlanFractionGroup, RtPlanSummary, export_rt_plan, summarize_rt_plan,
};
pub use rtstruct::{RoiMask, StructureSet, import_rtstruct, import_rtstruct_bytes};
pub use study::{
    ImportedStudy, collect_study_paths, import_study_from_files, import_study_from_paths,
};
