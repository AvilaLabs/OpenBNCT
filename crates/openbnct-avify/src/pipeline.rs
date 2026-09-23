// SPDX-License-Identifier: MIT

//! The verify orchestration shared by the CLI and the Python surface:
//! export the voxel plan, write the engine plan JSON, run the bounded
//! engine, bind everything into a run receipt, and load the returned
//! certificate. One implementation — every caller renders from the
//! same artifacts.

use std::path::{Path, PathBuf};

use openbnct_transport::{MaterialAssignment, TransportCase};

use crate::error::AvifyError;
use crate::plan::AvifyPlan;
use crate::receipt::{
    AvifyRunReceipt, BoundInput, EngineRecord, bind_inputs, hex_sha256, receipt_for_run,
};
use crate::run::{EngineInvocation, VerifyOutcome};
use crate::spec::AvifySpec;
use crate::voxel::{VoxelPlanExport, export_voxel_plan};

/// Everything `verify` produced, for callers that render results.
#[derive(Debug)]
pub struct VerifyPipelineOutput {
    pub export: VoxelPlanExport,
    pub plan_path: PathBuf,
    pub outcome: VerifyOutcome,
    pub receipt: AvifyRunReceipt,
    pub receipt_path: PathBuf,
}

fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, AvifyError> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

/// Build the engine plan document from the spec — the declared set
/// passes verbatim (the engine computes corner maps itself).
pub fn engine_plan(spec: &AvifySpec) -> AvifyPlan {
    AvifyPlan {
        declared_set: spec.plan.declared_set.clone(),
        brain_ratio: spec.plan.brain_ratio,
        weights: spec.plan.weights.clone(),
        criteria: spec.plan.criteria.clone(),
        normalisation: spec.plan.normalisation.clone(),
        histories: spec.plan.histories.clone(),
        seeds: spec.plan.seeds.clone(),
        beam: spec.plan.beam.clone(),
    }
}

/// The canonical export prefix for a verify run inside `outdir`.
pub fn verify_prefix(outdir: &Path) -> PathBuf {
    outdir.join("openbnct-case")
}

/// Export the voxel plan + engine plan JSON for a case/assignment/spec
/// triple. Returns the export record and the written plan path.
pub fn export_pipeline(
    case_path: &Path,
    assignment_path: &Path,
    spec_path: &Path,
    prefix: &Path,
) -> Result<(VoxelPlanExport, PathBuf), AvifyError> {
    let case: TransportCase = load_json(case_path)?;
    let assignment: MaterialAssignment = load_json(assignment_path)?;
    let spec: AvifySpec = load_json(spec_path)?;
    let export = export_voxel_plan(&case, &assignment, &spec, prefix)?;
    let plan_path = PathBuf::from(format!("{}.plan.json", prefix.display()));
    std::fs::write(&plan_path, serde_json::to_vec_pretty(&engine_plan(&spec))?)?;
    Ok((export, plan_path))
}

/// Export, run the engine, bind inputs/artifacts/certificate into a
/// versioned run receipt, and load the certificate. The engine runs as
/// a bounded child process — timeout kills and reaps it.
pub fn verify_pipeline(
    case_path: &Path,
    assignment_path: &Path,
    spec_path: &Path,
    outdir: &Path,
    engine_cmd: &str,
    timeout_s: u64,
    threads: Option<u32>,
) -> Result<VerifyPipelineOutput, AvifyError> {
    std::fs::create_dir_all(outdir)?;
    let prefix = verify_prefix(outdir);
    let (export, plan_path) = export_pipeline(case_path, assignment_path, spec_path, &prefix)?;

    let argv0: Vec<String> = engine_cmd.split_whitespace().map(String::from).collect();
    if argv0.is_empty() {
        return Err(AvifyError::Invalid(
            "engine command must not be empty".into(),
        ));
    }
    let invocation = EngineInvocation { argv0, timeout_s };
    let engine_version = invocation.engine_version();
    let outcome = invocation.verify(&prefix, &plan_path, outdir, threads)?;

    let abs = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let inputs = bind_inputs(&[
        ("case", &abs(case_path)),
        ("assignment", &abs(assignment_path)),
        ("spec", &abs(spec_path)),
    ])?;
    let exported = bind_inputs(&[
        ("arrays", &abs(&export.arrays_path)),
        ("meta", &abs(&export.meta_path)),
        ("plan", &abs(&plan_path)),
    ])?;
    let certificate_bound = BoundInput {
        path: abs(&outcome.certificate_path),
        sha256: hex_sha256(&std::fs::read(&outcome.certificate_path)?),
    };
    let receipt = receipt_for_run(
        inputs,
        exported,
        EngineRecord {
            argv0: invocation.argv0.clone(),
            version: engine_version,
        },
        certificate_bound,
        outcome.elapsed.as_secs_f64(),
    );
    let receipt_path = outdir.join("avify-run.json");
    receipt.write(&receipt_path)?;
    Ok(VerifyPipelineOutput {
        export,
        plan_path,
        outcome,
        receipt,
        receipt_path,
    })
}
