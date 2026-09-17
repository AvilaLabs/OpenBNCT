// SPDX-License-Identifier: Apache-2.0

//! Strict DICOM import and synthetic benchmark support.

#![forbid(unsafe_code)]

mod benchmark;
mod ct;
mod error;
mod rtdose;
mod rtplan;
mod rtstruct;
pub mod synthetic;

pub use benchmark::{
    BenchmarkReport, RoiReport, VerifiedBenchmarkCase, load_nf_bnct_001, verify_nf_bnct_001,
};
pub use ct::{CtVolume, import_ct_series};
pub use error::{DicomError, Result};
pub use rtdose::{DoseSelection, RtDoseExportOptions, RtDoseExportResult, export_rt_dose};
pub use rtplan::{
    RTPLAN_SUMMARY_SCHEMA, RtPlanBeamSpec, RtPlanBeamSummary, RtPlanControlPoint,
    RtPlanExportOptions, RtPlanFractionGroup, RtPlanSummary, export_rt_plan, summarize_rt_plan,
};
pub use rtstruct::{RoiMask, StructureSet, import_rtstruct};
