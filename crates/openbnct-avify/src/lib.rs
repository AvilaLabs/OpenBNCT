// SPDX-License-Identifier: MIT

//! Connector surface for the separately licensed Avify Dose engine
//! (R12, see `docs/R12_SCOPE_REVIEW.md`).
//!
//! This crate contains only claim-free mechanical work: it exports an
//! OpenBNCT case as the engine's voxel-plan format (npz + meta JSON),
//! assembles the plan JSON with the declared uncertainty set passed
//! verbatim, runs the engine as a bounded child process, and parses the
//! returned certificate for display. Extremal-map construction, envelope
//! arithmetic, applicability checks, and certificate derivation all live
//! inside the engine and are never reimplemented here — that boundary is
//! what `docs/IP_BOUNDARY.md` protects.

pub mod certificate;
pub mod error;
pub mod npz;
pub mod plan;
pub mod receipt;
pub mod run;
pub mod spec;
pub mod voxel;

pub use certificate::{AvifyCertificate, RoiAction, RoiBracket};
pub use error::AvifyError;
pub use npz::{ArraysNpz, read_arrays_npz};
pub use plan::{AvifyPlan, BeamSpec, DeclaredSet, Normalisation};
pub use receipt::{
    AvifyRunReceipt, BoundInput, EngineRecord, InputState, check_staleness, is_stale,
};
pub use run::{EngineInvocation, VerifyOutcome};
pub use spec::{AvifySpec, EngineClass};
pub use voxel::{VoxelPlanExport, export_voxel_plan};
