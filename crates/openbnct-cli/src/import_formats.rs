// SPDX-License-Identifier: MIT

//! Handlers for `openbnct import volume` and `openbnct import rtdose`
//! (R18-10 Tier-1 input formats).

use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};

use openbnct_core::{
    EXTERNAL_DOSE_SCHEMA, ExternalDoseDocument, ExternalDoseQuantity, ExternalFractionation,
    ExternalProducer, grid_geometry_equivalent,
};
use openbnct_evidence::{sha256_file, sha256_hex};
use openbnct_nifti::{
    Interpolation, NiftiImage, read_target_geometry, read_volume, resample_to_grid,
    sniff_volume_format, voxel_center, world_to_voxel, write_nifti,
};

use crate::write_new_json;

type DynResult<T> = Result<T, Box<dyn Error>>;

fn refuse_existing(path: &Path) -> DynResult<()> {
    if path.exists() {
        return Err(io::Error::other(format!(
            "{} already exists; outputs are never overwritten",
            path.display()
        ))
        .into());
    }
    Ok(())
}

/// `import volume`: read any supported volume and write it as NIfTI.
pub(crate) fn import_volume(input: &Path, output: &Path) -> DynResult<()> {
    refuse_existing(output)?;
    let format = sniff_volume_format(input)?;
    let image =
        read_volume(input).map_err(|e| io::Error::other(format!("{}: {e}", input.display())))?;
    let converted = NiftiImage {
        description: format!("openbnct volume import from {}", format.name()),
        transform_source: "sform",
        units_declared_mm: true,
        ..image
    };
    write_nifti(&converted, output)?;
    let g = &converted.geometry;
    println!("read {} as {}", input.display(), format.name());
    println!(
        "grid: shape {:?}, spacing {:?} mm, origin {:?} mm (LPS)",
        g.shape, g.spacing_mm, g.origin_mm
    );
    println!("source sha256: {}", sha256_file(input)?);
    println!(
        "wrote {} (sha256 {})",
        output.display(),
        sha256_file(output)?
    );
    Ok(())
}

/// `import rtdose`: DICOM RT Dose -> `openbnct.external-dose` bundle on the
/// case grid, plus an import record binding every input by hash.
pub(crate) fn import_rtdose(
    file: &Path,
    case: &Path,
    quantity: &str,
    fractions: u32,
    output: &Path,
) -> DynResult<()> {
    let record_path: PathBuf = output.with_extension("import-record.json");
    refuse_existing(output)?;
    refuse_existing(&record_path)?;
    if fractions == 0 {
        return Err(io::Error::other("--fractions must be at least 1").into());
    }
    let quantity = match quantity {
        "physical" => ExternalDoseQuantity::Physical,
        "rbe_weighted" => ExternalDoseQuantity::RbeWeighted,
        other => {
            return Err(io::Error::other(format!(
                "--quantity {other:?}: expected physical or rbe_weighted"
            ))
            .into());
        }
    };

    let dose = openbnct_dicom::import_rt_dose(file)?;
    if dose.dose_units != "GY" {
        return Err(io::Error::other(format!(
            "Dose Units {} cannot be imported as absolute dose; only GY is accepted",
            dose.dose_units
        ))
        .into());
    }
    let target = read_target_geometry(case)?;
    let case_json: serde_json::Value = serde_json::from_slice(&std::fs::read(case)?)?;
    let case_id = case_json
        .get("case_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| io::Error::other("case JSON has no case_id"))?
        .to_owned();
    if let (Some(a), Some(b)) = (
        case_json
            .get("frame_of_reference_uid")
            .and_then(|v| v.as_str()),
        dose.frame_of_reference_uid.as_deref(),
    ) && a != b
    {
        return Err(io::Error::other(format!(
            "RT Dose Frame of Reference UID {b} does not match the case's {a}"
        ))
        .into());
    }

    let identical = grid_geometry_equivalent(&dose.geometry, &target);
    let (values, coverage) = if identical {
        (dose.values.clone(), 1.0)
    } else {
        let image = NiftiImage {
            geometry: dose.geometry.clone(),
            values: dose.values.clone(),
            datatype: 64,
            transform_source: "sform",
            description: "rtdose".into(),
            intent_name: String::new(),
            units_declared_mm: true,
        };
        let values = resample_to_grid(&image, &target, Interpolation::Trilinear);
        let [nx, ny, nz] = target.shape.map(|v| v as usize);
        let mut inside = 0_usize;
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    if world_to_voxel(&dose.geometry, voxel_center(&target, i, j, k)).is_some() {
                        inside += 1;
                    }
                }
            }
        }
        if inside == 0 {
            return Err(
                io::Error::other("the RT Dose grid does not overlap the case grid at all").into(),
            );
        }
        (values, inside as f64 / (nx * ny * nz) as f64)
    };

    let normalization = format!(
        "DICOM RT Dose {} {}, DoseGridScaling {:e} applied, units GY{}",
        dose.dose_type
            .as_deref()
            .unwrap_or("(dose type undeclared)"),
        dose.dose_summation_type
            .as_deref()
            .unwrap_or("(summation undeclared)"),
        dose.dose_grid_scaling,
        if identical {
            ", native grid"
        } else {
            ", trilinear resample onto the case grid (0 outside the dose grid)"
        }
    );
    let document = ExternalDoseDocument {
        schema_version: EXTERNAL_DOSE_SCHEMA.into(),
        case_id: case_id.clone(),
        frame_of_reference_uid: dose.frame_of_reference_uid.clone(),
        geometry: target.clone(),
        producer: ExternalProducer {
            system: "dicom-rtdose".into(),
            version: dose
                .manufacturer
                .clone()
                .unwrap_or_else(|| "undeclared".into()),
            normalization,
        },
        quantity,
        values,
        absolute_standard_uncertainty: None,
        fractionation: ExternalFractionation::Uniform { count: fractions },
    };
    let sha256 = sha256_hex(&serde_json::to_vec_pretty(&document)?);
    let bundle = openbnct_core::import_external_dose(&document, &sha256)
        .map_err(|e| io::Error::other(format!("external-dose import: {e}")))?;
    write_new_json(output, &bundle)?;

    let record = serde_json::json!({
        "schema_version": "openbnct.rtdose-import-record/0.1.0",
        "case_id": case_id,
        "inputs": {
            "rtdose": {"file": file, "sha256": sha256_file(file)?},
            "case": {"file": case, "sha256": sha256_file(case)?},
        },
        "rtdose": {
            "sop_instance_uid": dose.sop_instance_uid,
            "study_instance_uid": dose.study_instance_uid,
            "series_instance_uid": dose.series_instance_uid,
            "frame_of_reference_uid": dose.frame_of_reference_uid,
            "dose_units": dose.dose_units,
            "dose_type": dose.dose_type,
            "dose_summation_type": dose.dose_summation_type,
            "dose_grid_scaling": dose.dose_grid_scaling,
            "dose_comment": dose.dose_comment,
            "geometry": dose.geometry,
        },
        "case_geometry": target,
        "resampled": !identical,
        "resampling": if identical { "none".to_owned() } else {
            "trilinear in world coordinates; case voxels outside the RT Dose grid are 0".to_owned()
        },
        "case_voxel_fraction_inside_dose_grid": coverage,
        "quantity": bundle.quantity,
        "fractions": fractions,
        "output": {"file": output, "sha256": sha256_file(output)?},
        "qualification": "research_only_not_clinical",
    });
    write_new_json(&record_path, &record)?;

    println!("external dose bundle at {}", output.display());
    println!(
        "grid: {} ({:.1}% of case voxels inside the RT Dose grid)",
        if identical {
            "native, no resampling"
        } else {
            "resampled onto the case grid"
        },
        coverage * 100.0
    );
    println!("record: {}", record_path.display());
    Ok(())
}
