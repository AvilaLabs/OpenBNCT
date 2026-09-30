// SPDX-License-Identifier: MIT

//! `beam phsp-info` and `beam phsp-bin`: IAEA phase-space beam sources.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_transport::{
    PHSP_ELECTRON, PHSP_NEUTRON, PHSP_PHOTON, PHSP_POSITRON, PHSP_PROTON, PhaseSpaceBinOptions,
    PhspReader, TransportCase, bin_phase_space, phase_space_fixed_source, scan_phsp, sha256_file,
};

type DynResult<T> = Result<T, Box<dyn std::error::Error>>;

fn particle_name(code: i8) -> &'static str {
    match code {
        PHSP_PHOTON => "photon",
        PHSP_ELECTRON => "electron",
        PHSP_POSITRON => "positron",
        PHSP_NEUTRON => "neutron",
        PHSP_PROTON => "proton",
        _ => "other",
    }
}

/// `beam phsp-info`: header keys plus a full streaming scan.
pub fn info(header: &Path) -> DynResult<()> {
    let mut reader = PhspReader::open(header)?;
    let h = reader.header.clone();
    println!("header: {}", header.display());
    println!("data: {}", reader.data_path.display());
    if let Some(title) = &h.title {
        println!("title: {title}");
    }
    println!(
        "record: {} bytes, {}-endian, stored X/Y/Z/U/V/W/weight = {:?}, {} extra floats, {} extra longs",
        h.record_length,
        if h.big_endian { "big" } else { "little" },
        h.stored,
        h.extra_floats,
        h.extra_longs
    );
    println!(
        "declared: {:?} particles, {:?} original histories; photons {:?}, electrons {:?}, \
         positrons {:?}, neutrons {:?}, protons {:?}",
        h.particles,
        h.original_histories,
        h.photons,
        h.electrons,
        h.positrons,
        h.neutrons,
        h.protons
    );
    let scan = scan_phsp(&mut reader)?;
    println!(
        "scanned: {} records, {} new histories",
        scan.records, scan.histories
    );
    for (code, s) in &scan.by_type {
        println!(
            "  {:<8} {:>12} records  weight {:.6e}  E {:.4e}..{:.4e} MeV (mean {:.4e})",
            particle_name(*code),
            s.records,
            s.weight_sum,
            s.min_energy_mev,
            s.max_energy_mev,
            s.mean_energy_mev
        );
    }
    println!(
        "position box (file frame, cm): x {:.3}..{:.3}, y {:.3}..{:.3}, z {:.3}..{:.3}",
        scan.min_position_cm[0],
        scan.max_position_cm[0],
        scan.min_position_cm[1],
        scan.max_position_cm[1],
        scan.min_position_cm[2],
        scan.max_position_cm[2]
    );
    println!("sha256(header): {}", sha256_file(header)?);
    println!("sha256(data): {}", sha256_file(&reader.data_path)?);
    println!(
        "note: only neutrons (type 4) are used by the deterministic solver and the OpenMC \
         source; photons are counted here and in the binned artifact's provenance."
    );
    Ok(())
}

pub struct BinArgs {
    pub header: PathBuf,
    pub case: PathBuf,
    pub data: PathBuf,
    pub direction: String,
    pub center_uv_cm: Option<Vec<f64>>,
    pub reference_z_cm: Option<f64>,
    pub pixel_mm: f64,
    pub dir_bins: String,
    pub theta_max_deg: Option<f64>,
    pub min_cosine: f64,
    pub id: Option<String>,
    pub output: PathBuf,
    pub case_output: Option<PathBuf>,
}

fn write_new_compact<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let mut out = io::BufWriter::with_capacity(1 << 20, &mut file);
    serde_json::to_writer(&mut out, value).map_err(io::Error::other)?;
    out.write_all(b"\n")?;
    out.flush()?;
    drop(out);
    file.sync_all()
}

/// `beam phsp-bin`: bin the neutrons onto the case's source plane.
pub fn bin(args: BinArgs) -> DynResult<()> {
    let (rings, sectors) = args
        .dir_bins
        .split_once(['x', 'X'])
        .and_then(|(r, s)| Some((r.parse::<u32>().ok()?, s.parse::<u32>().ok()?)))
        .ok_or_else(|| {
            io::Error::other("--dir-bins must look like 8x16 (cos-theta rings x phi sectors)")
        })?;
    let case: TransportCase = serde_json::from_slice(&fs::read(&args.case)?)?;
    let data_bytes = fs::read(&args.data)?;
    let data_sha = openbnct_evidence::sha256_hex(&data_bytes);
    let data: openbnct_transport::MultigroupData = serde_json::from_slice(&data_bytes)?;
    data.validate()
        .map_err(|e| io::Error::other(format!("multigroup data: {e}")))?;
    let header_abs = fs::canonicalize(&args.header)?;
    let id = args.id.clone().unwrap_or_else(|| {
        format!(
            "openbnct.phase-space.{}",
            header_abs
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("beam")
                .to_ascii_lowercase()
                .replace(|c: char| !c.is_ascii_alphanumeric(), "-")
        )
    });
    let center = match &args.center_uv_cm {
        Some(v) if v.len() == 2 => Some([v[0], v[1]]),
        Some(_) => return Err(io::Error::other("--center-uv-cm needs u,v").into()),
        None => None,
    };
    let mut options = PhaseSpaceBinOptions::new(&id, &args.direction);
    options.center_uv_cm = center;
    options.reference_z_cm = args.reference_z_cm;
    options.pixel_cm = args.pixel_mm / 10.0;
    options.rings = rings;
    options.sectors = sectors;
    options.theta_max_rad = args.theta_max_deg.map(f64::to_radians);
    options.min_cosine = args.min_cosine;
    let table = bin_phase_space(&header_abs, &case.geometry, &data, &data_sha, &options)?;
    write_new_compact(&args.output, &table)?;
    let table_abs = fs::canonicalize(&args.output)?;
    let table_sha = sha256_file(&table_abs)?;

    let p = &table.provenance;
    println!("phase-space source: {}", table.id);
    println!("table: {} (sha256 {table_sha})", args.output.display());
    println!(
        "neutrons: {} records (weight {:.6e}); accepted {} (weight {:.6e}, {:.4} of weight)",
        p.neutron_records,
        p.neutron_weight_sum,
        p.accepted_neutron_records,
        p.accepted_weight_sum,
        p.accepted_weight_sum / p.neutron_weight_sum
    );
    println!(
        "photons: {} records (weight {:.6e}) counted, not used; other particles: {}",
        p.photon_records, p.photon_weight_sum, p.other_particle_records
    );
    let r = &p.rejected;
    println!(
        "rejected neutrons: {} total (backward {}, grazing {}, past plane {}, outside face {}, \
         outside energy {}, beyond theta_max {}, bad weight {})",
        r.total(),
        r.backward,
        r.grazing,
        r.past_plane,
        r.outside_face,
        r.outside_energy,
        r.outside_direction_range,
        r.non_finite_or_nonpositive_weight
    );
    println!(
        "plane: {:?} at {} cm, beam {}, {} x {} pixels of {} mm, {}x{} direction bins to \
         theta_max {:.3} deg, {} groups, {} table entries",
        table.selection.plane_axis,
        table.selection.plane_offset_cm,
        table.selection.beam_direction,
        table.pixels.nu,
        table.pixels.nv,
        args.pixel_mm,
        table.directions.rings,
        table.directions.sectors,
        table.directions.cos_theta_max.acos().to_degrees(),
        table.group_count(),
        table.entries.weight.len()
    );
    if let Some(out) = &args.case_output {
        let mut bound = case;
        bound.source =
            phase_space_fixed_source(&table, &table_abs.display().to_string(), &table_sha);
        bound
            .validate()
            .map_err(|e| io::Error::other(format!("bound case is invalid: {e}")))?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(out)?;
        serde_json::to_writer_pretty(&mut file, &bound)?;
        file.write_all(b"\n")?;
        println!("case: {}", out.display());
    }
    Ok(())
}
