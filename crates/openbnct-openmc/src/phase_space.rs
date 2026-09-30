// SPDX-License-Identifier: MIT

//! OpenMC source file for a phase-space beam.
//!
//! The deterministic solver consumes the BINNED table; OpenMC is fed the
//! ORIGINAL accepted particles so `project verify` stays an independent
//! check. Both use `PhaseSpaceSelection::classify`, so the same neutrons
//! start in each code. Output is an OpenMC 0.16 `source_bank` HDF5 file:
//! a compound of `r{x,y,z}` cm, `u{x,y,z}`, `E` eV, `time`, `wgt`,
//! `delayed_group`, `surf_id` and `particle` (PDG code, 2112 = neutron).
//! Weights are scaled to mean 1, the usual unit-weight convention;
//! OpenMC normalizes tallies per source weight, so the scale is immaterial.

use hdf5_pure::{AttrValue, CompoundTypeBuilder, FileBuilder, make_f64_type, make_i32_type};
use openbnct_transport::{PhaseSpaceSource, load_phase_space_source};

/// File name of the generated source inside a deck.
pub const PHASE_SPACE_OPENMC_FILE: &str = "phase-space-source.h5";

/// Bytes per record of the packed OpenMC source-bank compound.
const RECORD_BYTES: usize = 84;

/// Result of converting a table's original particles.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenMcPhaseSpaceSource {
    /// The HDF5 file.
    pub bytes: Vec<u8>,
    pub particles: u64,
    /// Sum of the original weights before scaling to mean 1.
    pub weight_sum: f64,
}

/// Read the table a `phase_space` source references and write the OpenMC
/// source file. `plane_offset_cm` is the plane the particles start on
/// (the deck's source offset, which may sit a hair inside the table's).
pub fn build_openmc_source_file(
    table_path: &str,
    table_sha256: &str,
    plane_offset_cm: f64,
) -> Result<OpenMcPhaseSpaceSource, String> {
    let loaded = load_phase_space_source(table_path, table_sha256).map_err(|e| e.to_string())?;
    convert(&loaded.source, plane_offset_cm)
}

fn convert(
    table: &PhaseSpaceSource,
    plane_offset_cm: f64,
) -> Result<OpenMcPhaseSpaceSource, String> {
    let sel = &table.selection;
    let a = sel.plane_axis.index();
    let (u, v) = sel.plane_axis.in_plane_axes();
    let expected = table.provenance.accepted_neutron_records as usize;
    let mut raw: Vec<u8> = Vec::with_capacity(expected * RECORD_BYTES);
    let mut weight_sum = 0.0_f64;
    let mut weights: Vec<f64> = Vec::with_capacity(expected);
    table
        .for_each_accepted(|p| {
            let mut position = [0.0_f64; 3];
            position[a] = plane_offset_cm;
            position[u] = p.uv_cm[0];
            position[v] = p.uv_cm[1];
            for x in position {
                raw.extend_from_slice(&x.to_le_bytes());
            }
            for x in p.direction {
                raw.extend_from_slice(&x.to_le_bytes());
            }
            raw.extend_from_slice(&p.energy_ev.to_le_bytes());
            raw.extend_from_slice(&0.0_f64.to_le_bytes()); // time
            raw.extend_from_slice(&1.0_f64.to_le_bytes()); // wgt, rewritten below
            raw.extend_from_slice(&0_i32.to_le_bytes()); // delayed_group
            raw.extend_from_slice(&0_i32.to_le_bytes()); // surf_id
            raw.extend_from_slice(&2112_i32.to_le_bytes()); // neutron
            weight_sum += p.weight;
            weights.push(p.weight);
        })
        .map_err(|e| e.to_string())?;
    let n = weights.len();
    if n == 0 || n != expected {
        return Err(format!(
            "phase-space stream produced {n} particles, the table records {expected}"
        ));
    }
    debug_assert_eq!(raw.len(), n * RECORD_BYTES);
    let mean = weight_sum / n as f64;
    for (i, w) in weights.iter().enumerate() {
        let offset = i * RECORD_BYTES + 64;
        raw[offset..offset + 8].copy_from_slice(&(w / mean).to_le_bytes());
    }
    drop(weights);

    let xyz = || -> Result<_, String> {
        CompoundTypeBuilder::with_size(24)
            .f64_field("x", 0)
            .f64_field("y", 8)
            .f64_field("z", 16)
            .build()
            .map_err(|e| e.to_string())
    };
    let dtype = CompoundTypeBuilder::with_size(RECORD_BYTES as u32)
        .field("r", 0, xyz()?)
        .field("u", 24, xyz()?)
        .field("E", 48, make_f64_type())
        .field("time", 56, make_f64_type())
        .field("wgt", 64, make_f64_type())
        .field("delayed_group", 72, make_i32_type())
        .field("surf_id", 76, make_i32_type())
        .field("particle", 80, make_i32_type())
        .build()
        .map_err(|e| e.to_string())?;
    let mut builder = FileBuilder::new();
    builder.set_attr("filetype", AttrValue::AsciiString("source".into()));
    builder.set_attr("version", AttrValue::I32Array(vec![18, 2]));
    builder
        .create_dataset("source_bank")
        .with_compound_data(dtype, raw, n as u64);
    let bytes = builder.finish().map_err(|e| e.to_string())?;
    Ok(OpenMcPhaseSpaceSource {
        bytes,
        particles: n as u64,
        weight_sum,
    })
}
