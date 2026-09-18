// SPDX-License-Identifier: Apache-2.0

//! Multigroup collapse from processed pointwise HDF5 nuclear data.
//!
//! Reads OpenMC-format incident-neutron HDF5 tables (shared energy grid
//! per nuclide, per-reaction `xs` arrays tail-aligned for threshold
//! reactions, `mt`/`Q_value` attributes) and produces a declared
//! `openbnct.multigroup-data/0.1.0` artifact:
//!
//! - group `sigma_total` = collapsed elastic + summed non-elastic
//!   (capture, (n,p), (n,α), inelastic and other real reactions all
//!   counted as removal — inelastic energy return is neglected);
//! - the scatter matrix is the analytic P0 isotropic-in-CM elastic
//!   transfer kernel per nuclide — outgoing energy uniform on
//!   `[αE, E]`, α = ((A−1)/(A+1))² — integrated against the declared
//!   weighting spectrum; no thermal upscatter (free-gas / S(α,β)
//!   binding effects are not modeled and are declared as such);
//! - dose responses are mass-kerma coefficients in Gy·cm² per unit
//!   fluence: boron from ¹⁰B(n,α) with a declared branch-weighted
//!   2.34 MeV charged release, nitrogen from ¹⁴N(n,p) with declared
//!   0.626 MeV (the tables' lumped Q_value attributes are 0 for these
//!   exothermic channels), hydrogen from elastic recoil
//!   (Ē(1−α)/2 per collision over all nuclides), photon from
//!   capture-γ table Q values plus the ¹⁰B (n,α₁) 0.478 MeV γ —
//!   local-deposition approximation, no photon transport.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hdf5_pure::File;
use openbnct_transport::{
    ContentReference, MaterialDefinition, MultigroupData, MultigroupMaterial,
};
use thiserror::Error;

/// MeV → J.
const MEV_TO_J: f64 = 1.602_177e-13;
/// Avogadro constant.
const N_A: f64 = 6.022_140_76e23;
/// kg per g for the mass-kerma conversion.
const G_PER_KG: f64 = 1.0e3;
/// cm² per barn — the HDF5 tables store microscopic σ in barns.
const BARN_CM2: f64 = 1.0e-24;

#[derive(Debug, Error)]
pub enum CollapseError {
    #[error("collapse: {0}")]
    Invalid(String),
    #[error("collapse: io: {0}")]
    Io(String),
    #[error("collapse: hdf5: {0}")]
    Hdf5(String),
    #[error("collapse: {0}")]
    Model(String),
}

fn invalid(msg: impl Into<String>) -> CollapseError {
    CollapseError::Invalid(msg.into())
}

/// Declared weighting spectrum for the collapse integrals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WeightingSpectrum {
    /// Maxwellian flux (kT = 0.0253 eV) below `cut` eV, 1/E above.
    ThermalMaxwellianEpithermalFlat { cut_ev: f64 },
    /// 1/E everywhere.
    FlatLethargy,
}

impl WeightingSpectrum {
    pub fn w(&self, energy_ev: f64) -> f64 {
        match *self {
            WeightingSpectrum::ThermalMaxwellianEpithermalFlat { cut_ev } => {
                if energy_ev <= cut_ev {
                    // Maxwellian *flux* spectrum ∝ E·exp(−E/kT).
                    energy_ev * (-energy_ev / 0.0253e-0_f64.max(f64::MIN_POSITIVE)).exp()
                } else {
                    1.0 / energy_ev
                }
            }
            WeightingSpectrum::FlatLethargy => 1.0 / energy_ev,
        }
    }

    fn describe(&self) -> String {
        match *self {
            WeightingSpectrum::ThermalMaxwellianEpithermalFlat { cut_ev } => {
                format!("Maxwellian flux weighting (kT = 0.0253 eV) below {cut_ev} eV, 1/E above")
            }
            WeightingSpectrum::FlatLethargy => "1/E (flat lethargy) weighting everywhere".into(),
        }
    }
}

/// Atomic mass (g/mol) and integer mass number for the nuclides the
/// phantom materials use. ENDF-loaded nuclides fall back to their AWR.
fn nuclide_mass(name: &str) -> Option<(f64, u32)> {
    Some(match name {
        "H1" => (1.007_825, 1),
        "H2" => (2.014_102, 2),
        "B10" => (10.012_937, 10),
        "B11" => (11.009_305, 11),
        "C12" => (12.000_000, 12),
        "C13" => (13.003_355, 13),
        "N14" => (14.003_074, 14),
        "N15" => (15.000_109, 15),
        "O16" => (15.994_915, 16),
        "O17" => (16.999_132, 17),
        "O18" => (17.999_160, 18),
        "Na23" => (22.989_769, 23),
        "Mg24" => (23.985_042, 24),
        "Mg25" => (24.985_837, 25),
        "Mg26" => (25.982_593, 26),
        "P31" => (30.973_762, 31),
        "S32" => (31.972_071, 32),
        "S33" => (32.971_459, 33),
        "S34" => (33.967_867, 34),
        "Cl35" => (34.968_853, 35),
        "Cl37" => (36.965_903, 37),
        "K39" => (38.963_707, 39),
        "K40" => (39.963_999, 40),
        "K41" => (40.961_825, 41),
        "Ca40" => (39.962_591, 40),
        "Ca42" => (41.958_618, 42),
        "Ca43" => (42.958_767, 43),
        "Ca44" => (43.955_481, 44),
        "Ca46" => (45.953_693, 46),
        "Ca48" => (47.952_523, 48),
        "Fe54" => (53.939_611, 54),
        "Fe56" => (55.934_937, 56),
        "Fe57" => (56.935_394, 57),
        "Fe58" => (57.933_276, 58),
        _ => return None,
    })
}

/// Declared charged-kerma release for ¹⁰B(n,α) — the table's lumped
/// MT107 carries no Q. Branch-weighted: 94% (n,α₁γ) at 2.31 MeV
/// charged + 6% (n,α₀) at 2.79 MeV ≈ 2.34 MeV.
const B10_NA_CHARGED_MEV: f64 = 2.34;
/// The (n,α₁γ) branch's 0.478 MeV de-excitation photon, branch-weighted
/// 0.94 × 0.478 ≈ 0.449 MeV per capture — folded into the photon
/// channel, not charged kerma.
const B10_NA_PHOTON_MEV: f64 = 0.449;
/// Declared kerma release for ¹⁴N(n,p) — lumped MT103 carries no Q.
const N14_NP_MEV: f64 = 0.626;

/// One nuclide's pointwise table on the shared 294 K grid.
struct NuclideTable {
    name: String,
    mass_g_mol: f64,
    mass_number: u32,
    /// Shared energy grid, ascending eV.
    energy: Vec<f64>,
    /// MT2 elastic on the shared grid.
    elastic: Vec<f64>,
    /// MT102 (n,γ) on the shared grid, plus its Q in MeV.
    capture: (Vec<f64>, f64),
    /// MT103 (n,p), plus its Q in MeV.
    np: (Vec<f64>, f64),
    /// MT107 (n,α), plus its Q in MeV.
    na: (Vec<f64>, f64),
    /// Every other real reaction (mt 3..=299, excluding the channels
    /// above) summed — counted as removal in σ_t.
    other: Vec<f64>,
}

fn read_attr_f64(
    attrs: &std::collections::HashMap<String, hdf5_pure::AttrValue>,
    key: &str,
) -> f64 {
    match attrs.get(key) {
        Some(hdf5_pure::AttrValue::F64(v)) => *v,
        Some(hdf5_pure::AttrValue::F32(v)) => f64::from(*v),
        Some(hdf5_pure::AttrValue::I64(v)) => *v as f64,
        _ => 0.0,
    }
}

fn read_attr_i64(
    attrs: &std::collections::HashMap<String, hdf5_pure::AttrValue>,
    key: &str,
) -> i64 {
    match attrs.get(key) {
        Some(hdf5_pure::AttrValue::I64(v)) => *v,
        Some(hdf5_pure::AttrValue::I32(v)) => i64::from(*v),
        _ => -1,
    }
}

fn load_nuclide(library_dir: &Path, name: &str) -> Result<NuclideTable, CollapseError> {
    let (mass, mass_number) = nuclide_mass(name)
        .ok_or_else(|| invalid(format!("no atomic mass table entry for nuclide {name:?}")))?;
    let path: PathBuf = library_dir.join(format!("{name}.h5"));
    let file =
        File::open(&path).map_err(|e| CollapseError::Hdf5(format!("{}: {e}", path.display())))?;
    let nuc = file
        .group(&format!("/{name}"))
        .map_err(|e| CollapseError::Hdf5(format!("{name} group: {e}")))?;
    let energy: Vec<f64> = nuc
        .group("energy")
        .and_then(|g| g.dataset("294K"))
        .and_then(|d| d.read_f64())
        .map_err(|e| CollapseError::Hdf5(format!("{name} energy/294K: {e}")))?;
    let n = energy.len();
    let mut elastic = vec![0.0; n];
    let mut capture = (vec![0.0; n], 0.0);
    let mut np = (vec![0.0; n], 0.0);
    let mut na = (vec![0.0; n], 0.0);
    let mut other = vec![0.0; n];

    let reactions = nuc
        .group("reactions")
        .map_err(|e| CollapseError::Hdf5(format!("{name} reactions: {e}")))?;
    for rname in reactions
        .groups()
        .map_err(|e| CollapseError::Hdf5(format!("{name} reaction list: {e}")))?
    {
        let rg = reactions
            .group(&rname)
            .map_err(|e| CollapseError::Hdf5(format!("{name}/{rname}: {e}")))?;
        let attrs = rg
            .attrs()
            .map_err(|e| CollapseError::Hdf5(format!("{name}/{rname} attrs: {e}")))?;
        let mt = read_attr_i64(&attrs, "mt");
        if !(3..=299).contains(&mt) && mt != 2 {
            continue; // energy-release pseudo-reactions and unlabeled groups
        }
        let Ok(tg) = rg.group("294K") else { continue };
        let Ok(ds) = tg.dataset("xs") else { continue };
        let xs: Vec<f64> = ds
            .read_f64()
            .map_err(|e| CollapseError::Hdf5(format!("{name}/{rname} xs: {e}")))?;
        if xs.len() > n {
            return Err(invalid(format!(
                "{name} {rname}: xs longer than the energy grid"
            )));
        }
        // Threshold arrays are tail-aligned on the shared grid.
        let offset = n - xs.len();
        let mut full = vec![0.0; n];
        for (j, v) in xs.iter().enumerate() {
            full[offset + j] = *v;
        }
        // The table stores Q_value in eV; dose responses need MeV.
        let q = read_attr_f64(&attrs, "Q_value").abs() / 1.0e6;
        match mt {
            2 => elastic = full,
            102 => capture = (full, q),
            103 => np = (full, q),
            107 => na = (full, q),
            _ => {
                for (a, b) in other.iter_mut().zip(full.iter()) {
                    *a += *b;
                }
            }
        }
    }
    Ok(NuclideTable {
        name: name.into(),
        mass_g_mol: mass,
        mass_number,
        energy,
        elastic,
        capture,
        np,
        na,
        other,
    })
}

/// Load a nuclide from an ENDF-6 tape (raw evaluation or NJOY PENDF).
/// Every MF3 section's energy points go into a shared union grid; each
/// section is evaluated on it with its own interpolation law. Q values
/// come from the TAB1 QI field (eV → MeV).
fn load_nuclide_endf(path: &Path, name: &str) -> Result<NuclideTable, CollapseError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| CollapseError::Io(format!("{}: {e}", path.display())))?;
    let tape = crate::endf_mf3::parse_endf(&text)
        .map_err(|e| CollapseError::Invalid(format!("{}: {e}", path.display())))?;
    let (mass, mass_number) = nuclide_mass(name).unwrap_or((tape.awr, tape.awr.round() as u32));

    // Union grid of every MF3 section's points — RECONR/PENDF tapes
    // already share one; raw evaluations may not.
    let mut energy: Vec<f64> = Vec::new();
    for s in tape.sections.values() {
        energy.extend_from_slice(&s.energy_ev);
    }
    energy.sort_by(f64::total_cmp);
    energy.dedup_by(|a, b| (*a - *b).abs() < 1e-12 * b.abs().max(1e-20));
    if energy.len() < 2 {
        return Err(invalid(format!("{name}: ENDF tape has no usable MF3 grid")));
    }
    let n = energy.len();
    let eval = |mt: u32| -> (Vec<f64>, f64) {
        match tape.sections.get(&mt) {
            Some(s) => (energy.iter().map(|&e| s.at(e)).collect(), s.q_ev / 1.0e6),
            None => (vec![0.0; n], 0.0),
        }
    };
    let (elastic, _) = eval(2);
    let capture = eval(102);
    let np = eval(103);
    let na = eval(107);
    let mut other = vec![0.0; n];
    for (mt, s) in &tape.sections {
        if matches!(*mt, 2 | 102 | 103 | 107) || !(3..=299).contains(mt) {
            continue;
        }
        for (a, &e) in other.iter_mut().zip(energy.iter()) {
            *a += s.at(e);
        }
    }
    Ok(NuclideTable {
        name: name.into(),
        mass_g_mol: mass,
        mass_number,
        energy,
        elastic,
        capture,
        np,
        na,
        other,
    })
}

/// Integrate `f(E)` over `[lo, hi]` by trapezoid on the nuclide's own
/// grid points plus boundary samples interpolated log-log — cross
/// sections follow power-law shapes (1/v tails, thresholds) where
/// log-log interpolation is far more faithful than linear-in-E.
fn integrate_grid(energy: &[f64], f: &[f64], lo: f64, hi: f64) -> f64 {
    let interp = |e: f64| -> f64 {
        if e <= energy[0] {
            return f[0];
        }
        if e >= energy[energy.len() - 1] {
            return f[f.len() - 1];
        }
        let i = energy.partition_point(|&x| x < e);
        let (e0, e1) = (energy[i - 1], energy[i]);
        let (f0, f1) = (f[i - 1], f[i]);
        if f0 > 0.0 && f1 > 0.0 && e0 > 0.0 && e1 > 0.0 {
            let t = (e.ln() - e0.ln()) / (e1.ln() - e0.ln());
            (f0.ln() + t * (f1.ln() - f0.ln())).exp()
        } else {
            f0 + (f1 - f0) * (e - e0) / (e1 - e0)
        }
    };
    let mut pts: Vec<(f64, f64)> = vec![(lo, interp(lo))];
    for (i, &e) in energy.iter().enumerate() {
        if e > lo && e < hi {
            pts.push((e, f[i]));
        }
    }
    pts.push((hi, interp(hi)));
    pts.windows(2)
        .map(|w| 0.5 * (w[0].1 + w[1].1) * (w[1].0 - w[0].0))
        .sum()
}

/// P0 iso-CM elastic transfer probability that a neutron at energy `e`
/// on a nuclide with parameter `alpha` lands in the group `[lo, hi]`.
/// Outgoing energy is uniform on `[αe, e]`.
fn elastic_transfer_p(e: f64, alpha: f64, lo: f64, hi: f64) -> f64 {
    if e <= 0.0 {
        return 0.0;
    }
    let elo = alpha * e;
    let overlap = (e.min(hi) - elo.max(lo)).max(0.0);
    overlap / ((1.0 - alpha) * e)
}

/// P1 iso-CM elastic transfer moment: the outgoing lab-frame mean
/// cosine folded into the transfer probability — the `l = 1` Legendre
/// moment of the elastic kernel restricted to `[lo, hi]`.
///
/// For iso-CM elastic scattering, `μ_cm = (2E'/E − 1 − α)/(1 − α)` and
/// the lab cosine of the outgoing neutron is
/// `μ_lab = (A·μ_cm + 1)/√(A² + 2A·μ_cm + 1)`. The return value is
/// `∫_{lo}^{hi} μ_lab(E,E') / (E·(1−α)) dE'` over the reachable window,
/// so `p1(g→g')/p0(g→g')` is the mean lab cosine of transfers into the
/// destination group. For A = 1 this reduces analytically to the known
/// `⟨μ⟩ = 2/3` over the full outgoing range.
fn elastic_transfer_p1(e: f64, alpha: f64, mass_number: f64, lo: f64, hi: f64) -> f64 {
    if e <= 0.0 {
        return 0.0;
    }
    let elo = alpha * e;
    let lo_c = elo.max(lo);
    let hi_c = e.min(hi);
    if hi_c <= lo_c {
        return 0.0;
    }
    // Midpoint quadrature over the overlap — μ_lab is smooth in E'.
    const N: usize = 64;
    let step = (hi_c - lo_c) / N as f64;
    let a = mass_number;
    let mut acc = 0.0;
    for i in 0..N {
        let ep = lo_c + (i as f64 + 0.5) * step;
        let mu_cm = (2.0 * ep / e - 1.0 - alpha) / (1.0 - alpha);
        let den2 = a * a + 2.0 * a * mu_cm + 1.0;
        // E'→0 on A=1 is a 0/0 corner; the kinematic limit is μ_lab→0.
        let mu_lab = if den2 > 1e-30 {
            (a * mu_cm + 1.0) / den2.sqrt()
        } else {
            0.0
        };
        acc += mu_lab;
    }
    acc * step / ((1.0 - alpha) * e)
}

/// Options for [`collapse_multigroup`].
pub struct CollapseOptions {
    /// Directory of `<Nuclide>.h5` incident-neutron tables.
    pub library_dir: PathBuf,
    /// Per-nuclide ENDF-6 tape paths (raw evaluations or NJOY PENDF)
    /// overriding the HDF5 lookup — for nuclides outside the processed
    /// library.
    pub endf_paths: BTreeMap<String, PathBuf>,
    /// The material definitions the artifact is bound to — every
    /// material the solve requires must appear here.
    pub materials: Vec<MaterialDefinition>,
    /// Group boundaries, eV, strictly descending (group 0 = highest).
    pub energy_boundaries_ev: Vec<f64>,
    /// Declared collapse weighting spectrum.
    pub weighting: WeightingSpectrum,
    /// Artifact id.
    pub id: String,
    /// Component-profile reference for the dose-response vectors.
    pub component_profile: Option<ContentReference>,
    /// Extra free-text appended to the collapse declaration.
    pub note: String,
}

/// Collapse the processed pointwise library into a declared
/// `MultigroupData` artifact covering every supplied material.
pub fn collapse_multigroup(opts: &CollapseOptions) -> Result<MultigroupData, CollapseError> {
    let b = &opts.energy_boundaries_ev;
    if b.len() < 2 || !b.windows(2).all(|w| w[0] > w[1]) {
        return Err(invalid("energy boundaries must be strictly descending"));
    }
    let groups = b.len() - 1;
    if opts.materials.is_empty() {
        return Err(invalid("at least one material is required"));
    }

    let mut materials = Vec::with_capacity(opts.materials.len());
    for material in &opts.materials {
        materials.push(collapse_material(opts, material, groups)?);
    }

    let endf_note = if opts.endf_paths.is_empty() {
        String::new()
    } else {
        format!(
            " Nuclides [{}] were collapsed from ENDF-6 tapes (raw or \
             NJOY PENDF) via the built-in MF3 reader: {}.",
            opts.endf_paths
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            opts.endf_paths
                .iter()
                .map(|(k, v)| format!("{k}={}", v.display()))
                .collect::<Vec<_>>()
                .join(", "),
        )
    };
    let mut declaration = format!(
        "Collapsed from the processed ENDF/B-VIII.1 294 K OpenMC HDF5 \
         tables in {} by `openbnct sn collapse` at {groups} groups. \
         sigma_total = collapsed elastic + summed non-elastic removal \
         (capture, (n,p), (n,α), inelastic, other; inelastic energy \
         return neglected). Scatter = analytic P0 isotropic-in-CM \
         elastic kernel, α = ((A−1)/(A+1))², with the P1 lab-cosine \
         transfer moments emitted alongside (scatter_p1_matrix_per_cm); \
         no thermal upscatter — \
         free-gas model, molecular binding (S(α,β)) not included. \
         Dose responses are mass-kerma coefficients (Gy·cm² per unit \
         fluence): boron = 10B(n,α) charged kerma with declared \
         2.34 MeV (branch-weighted; the table's lumped Q is absent), \
         nitrogen = 14N(n,p) with declared 0.626 MeV, hydrogen = \
         elastic recoil kerma (Ē(1−α)/2 summed over all nuclides), \
         photon = capture-γ kerma from table MT102 Q values plus the \
         10B (n,α₁) 0.478 MeV γ (0.449 MeV branch-weighted), all \
         depositing locally — no photon transport. Other \
         charged-particle channels (e.g. 17O(n,α)) remain in σt \
         removal but are not folded into a named component. \
         Weighting: {}.{}{}",
        opts.library_dir.display(),
        opts.weighting.describe(),
        endf_note,
        if opts.note.is_empty() {
            String::new()
        } else {
            format!(" {}", opts.note)
        },
    );
    if declaration.len() > 4000 {
        declaration.truncate(4000);
    }

    let data = MultigroupData {
        schema_version: openbnct_transport::MULTIGROUP_DATA_SCHEMA.into(),
        id: opts.id.clone(),
        energy_boundaries_ev: b.clone(),
        collapse_declaration: declaration,
        component_profile: opts.component_profile.clone(),
        materials,
    };
    data.validate()
        .map_err(|e| CollapseError::Model(format!("emitted data invalid: {e}")))?;
    Ok(data)
}

/// Collapse one material: group σt, the P0 elastic transfer matrix,
/// and the four mass-kerma dose-response vectors.
fn collapse_material(
    opts: &CollapseOptions,
    material: &MaterialDefinition,
    groups: usize,
) -> Result<MultigroupMaterial, CollapseError> {
    material
        .validate()
        .map_err(|e| CollapseError::Model(format!("material invalid: {e}")))?;
    let b = &opts.energy_boundaries_ev;
    let rho = material.density_g_cm3;

    // Per-nuclide tables and number densities.
    let mut tables = Vec::new();
    let mut densities = Vec::new();
    for nuc in &material.nuclides {
        let table = match opts.endf_paths.get(&nuc.name) {
            Some(path) => load_nuclide_endf(path, &nuc.name)?,
            None => load_nuclide(&opts.library_dir, &nuc.name)?,
        };
        // atoms/cm³ · cm²/barn → the effective per-barn macroscopic
        // multiplier applied when the table σ's are summed.
        let n = rho * nuc.mass_fraction / table.mass_g_mol * N_A * BARN_CM2;
        tables.push(table);
        densities.push(n);
    }

    let mut sigma_t = vec![0.0; groups];
    let mut transfer = vec![0.0; groups * groups];
    let mut transfer_p1 = vec![0.0; groups * groups];
    let mut dose_boron = vec![0.0; groups];
    let mut dose_nitrogen = vec![0.0; groups];
    let mut dose_hydrogen = vec![0.0; groups];
    let mut dose_photon = vec![0.0; groups];
    // Transport-correction accumulators: μ̄_g = Σ_n n_n σ̄s_n,g
    // (2/3A_n) / Σ_n n_n σ̄s_n,g — the scatter-weighted mean lab cosine
    // for iso-CM elastic.
    let mut mu_num = vec![0.0; groups];
    let mut mu_den = vec![0.0; groups];

    for (table, &n_density) in tables.iter().zip(&densities) {
        let alpha = ((table.mass_number as f64 - 1.0) / (table.mass_number as f64 + 1.0)).powi(2);
        let e = &table.energy;
        let weight: Vec<f64> = e.iter().map(|&x| opts.weighting.w(x)).collect();
        let sw = |xs: &[f64]| -> Vec<f64> { xs.iter().zip(&weight).map(|(s, w)| s * w).collect() };
        let absorption: Vec<f64> = (0..e.len())
            .map(|i| table.capture.0[i] + table.np.0[i] + table.na.0[i] + table.other[i])
            .collect();

        for g in 0..groups {
            let hi = b[g];
            let lo = b[g + 1];
            let w_norm = integrate_grid(e, &weight, lo, hi);
            if w_norm <= 0.0 {
                return Err(invalid(format!("group {g}: zero weighting integral")));
            }
            let collapse = |f: &[f64]| integrate_grid(e, f, lo, hi) / w_norm;

            let sigma_s = collapse(&sw(&table.elastic));
            let sigma_a = collapse(&sw(&absorption));

            // Elastic transfer out of g: T[g][g'] = N ∫σs·W·P(E→g') / ∫W.
            // P varies within g, so integrate the product numerically.
            let elastic_weighted = sw(&table.elastic);
            let mut row = vec![0.0; groups];
            let mut row_p1 = vec![0.0; groups];
            let a = table.mass_number as f64;
            for gp in 0..groups {
                let integrand: Vec<f64> = e
                    .iter()
                    .enumerate()
                    .map(|(i, &x)| {
                        elastic_weighted[i] * elastic_transfer_p(x, alpha, b[gp + 1], b[gp])
                    })
                    .collect();
                row[gp] = collapse(&integrand);
                let integrand_p1: Vec<f64> = e
                    .iter()
                    .enumerate()
                    .map(|(i, &x)| {
                        elastic_weighted[i] * elastic_transfer_p1(x, alpha, a, b[gp + 1], b[gp])
                    })
                    .collect();
                row_p1[gp] = collapse(&integrand_p1);
            }
            for (gp, val) in row.iter().enumerate() {
                transfer[g * groups + gp] += n_density * val;
            }
            for (gp, val) in row_p1.iter().enumerate() {
                transfer_p1[g * groups + gp] += n_density * val;
            }
            sigma_t[g] += n_density * (sigma_s + sigma_a);
            // Iso-CM elastic: mean lab cosine is exactly 2/(3A).
            let mu = 2.0 / (3.0 * table.mass_number as f64);
            mu_num[g] += n_density * sigma_s * mu;
            mu_den[g] += n_density * sigma_s;

            // Dose responses (mass-kerma per unit fluence, Gy·cm²).
            let conv = MEV_TO_J * G_PER_KG / rho;
            // "hydrogen" — elastic recoil kerma summed over all nuclides
            // (overwhelmingly H); mean recoil Ē·(1−α)/2 per collision.
            let recoil_frac = (1.0 - alpha) / 2.0;
            let ebar = collapse(
                &e.iter()
                    .zip(&table.elastic)
                    .zip(&weight)
                    .map(|((x, s), w)| x * s * w)
                    .collect::<Vec<_>>(),
            ) / sigma_s.max(f64::MIN_POSITIVE);
            dose_hydrogen[g] += n_density * sigma_s * ebar * recoil_frac * conv;
            // "photon" — capture-γ cascade kerma from every nuclide's
            // MT102 (table Q), plus the ¹⁰B (n,α₁) 0.478 MeV γ.
            dose_photon[g] += n_density * collapse(&sw(&table.capture.0)) * table.capture.1 * conv;
            // Named charged channels: strict per-nuclide semantics with
            // declared kerma energies — the lumped MT103/MT107 Q attrs
            // are 0 for the exothermic reactions that matter.
            if table.name == "B10" {
                let sna = collapse(&sw(&table.na.0));
                dose_boron[g] += n_density * sna * B10_NA_CHARGED_MEV * conv;
                dose_photon[g] += n_density * sna * B10_NA_PHOTON_MEV * conv;
            }
            if table.name == "N14" {
                dose_nitrogen[g] += n_density * collapse(&sw(&table.np.0)) * N14_NP_MEV * conv;
            }
            // Other charged-particle channels (e.g. ¹⁷O(n,α)) remain in
            // σt removal but are not folded into a named component —
            // declared in the collapse declaration.
        }
    }

    let mut dose_response_gy_cm2: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    dose_response_gy_cm2.insert("boron".into(), dose_boron);
    dose_response_gy_cm2.insert("nitrogen".into(), dose_nitrogen);
    dose_response_gy_cm2.insert("hydrogen".into(), dose_hydrogen);
    dose_response_gy_cm2.insert("photon".into(), dose_photon);

    Ok(MultigroupMaterial {
        material_id: material.id.clone(),
        sigma_total_per_cm: sigma_t,
        scatter_matrix_per_cm: transfer,
        scatter_p1_matrix_per_cm: Some(transfer_p1),
        dose_response_gy_cm2,
        transport_mu_bar: Some(
            mu_num
                .iter()
                .zip(&mu_den)
                .map(|(&num, &den)| if den > 0.0 { num / den } else { 0.0 })
                .collect(),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Log-spaced descending boundaries spanning `groups` decades of
    /// equal lethargy width between `top` and `bottom` eV.
    fn test_boundaries(top: f64, bottom: f64, groups: usize) -> Vec<f64> {
        let step = (top / bottom).ln() / groups as f64;
        (0..=groups)
            .map(|i| top * (-step * i as f64).exp())
            .collect()
    }

    #[test]
    fn elastic_transfer_is_a_partition_of_unity() {
        // Summing P(E→g') over all destination groups must equal the
        // fraction of the kinematic range [αE, E] the group structure
        // covers — 1 inside the boundaries, less for ranges extending
        // below the lowest edge (outgoing energy under the structure
        // floor is genuinely lost).
        let b = test_boundaries(1.0e7, 1.0e-5, 40);
        let (floor, top) = (*b.last().unwrap(), b[0]);
        for &(a, label) in &[(0.0_f64, "H1"), (0.918_f64, "C12")] {
            for &e in &[3.0e-4_f64, 0.4, 250.0, 5.0e4, 1.2e6] {
                let sum: f64 = (0..b.len() - 1)
                    .map(|g| elastic_transfer_p(e, a, b[g + 1], b[g]))
                    .sum();
                let covered = (e.min(top) - (a * e).max(floor)).max(0.0) / ((1.0 - a) * e);
                assert!(
                    (sum - covered).abs() < 1e-9,
                    "{label} at {e} eV: transfer row sums to {sum}, expected {covered}"
                );
            }
        }
    }

    #[test]
    fn elastic_transfer_stays_in_source_group_for_heavy_targets() {
        // α→1 (very heavy nuclide): all outgoing energy lands back in
        // the source group — no downscatter.
        let b = test_boundaries(1.0e7, 1.0e-5, 40);
        let e = 1.0e4; // inside group with bounds ~[6.8e3, 1.5e4]
        let g_src = (0..b.len() - 1)
            .find(|&g| e <= b[g] && e > b[g + 1])
            .unwrap();
        let alpha = ((207.0_f64 - 1.0) / (207.0 + 1.0)).powi(2); // Pb-like
        for g in 0..b.len() - 1 {
            let p = elastic_transfer_p(e, alpha, b[g + 1], b[g]);
            if g == g_src {
                assert!(p > 0.99, "diagonal transfer {p} should be ~1");
            } else {
                assert!(p < 0.01, "off-diagonal transfer {p} should be ~0");
            }
        }
    }

    #[test]
    fn elastic_transfer_p1_recovers_analytic_mean_cosines() {
        // Full-range P1/P0 ratio must equal the iso-CM elastic mean lab
        // cosine 2/(3A) exactly — the kinematic identity for the kernel.
        // H: μ̄ = 2/3; A=12: μ̄ = 1/18; Pb-like: ~3e-3.
        for (a, expected) in [
            (1.0_f64, 2.0 / 3.0),
            (12.0, 2.0 / 36.0),
            (207.0, 2.0 / 621.0),
        ] {
            let alpha = ((a - 1.0) / (a + 1.0)).powi(2);
            let e = 1.0e6;
            let (lo, hi) = (alpha * e, e);
            let p0 = elastic_transfer_p(e, alpha, lo, hi);
            let p1 = elastic_transfer_p1(e, alpha, a, lo, hi);
            assert!(
                (p1 / p0 - expected).abs() / expected < 2e-3,
                "A={a}: p1/p0 = {} expected {expected}",
                p1 / p0
            );
        }
    }

    #[test]
    fn elastic_transfer_p1_hydrogen_subrange() {
        // A = 1: μ_lab = √(E'/E); P1 over [lo,hi] is
        // ∫√(E'/E)/E dE' = (2/3)·(hi^{3/2} − lo^{3/2})/E^{3/2}.
        let (e, lo, hi) = (1.0e6, 2.0e5, 5.0e5);
        let p1 = elastic_transfer_p1(e, 0.0, 1.0, lo, hi);
        let want = (2.0 / 3.0) * (hi.powf(1.5) - lo.powf(1.5)) / e.powf(1.5);
        assert!(
            (p1 - want).abs() / want < 1e-3,
            "H subrange p1 = {p1} expected {want}"
        );
    }

    #[test]
    fn integrate_grid_recovers_log_integral_and_constant_collapse() {
        let e: Vec<f64> = (0..2000).map(|i| 1.0e-2 * (1.01_f64).powi(i)).collect();
        // f(E) = 1/E exactly on the grid: ∫_a^b 1/E dE = ln(b/a).
        let inv_e: Vec<f64> = e.iter().map(|x| 1.0 / x).collect();
        let (lo, hi) = (0.1, 10.0);
        let got = integrate_grid(&e, &inv_e, lo, hi);
        let want = (hi / lo).ln();
        assert!(
            (got - want).abs() / want < 1e-3,
            "∫1/E over [{lo},{hi}] = {got}, expected {want}"
        );
        // Constant σ under 1/E weighting collapses to σ.
        let w: Vec<f64> = inv_e.clone();
        let sig: Vec<f64> = vec![3.7; e.len()];
        let num = integrate_grid(
            &e,
            &sig.iter().zip(&w).map(|(s, w)| s * w).collect::<Vec<_>>(),
            lo,
            hi,
        );
        let den = integrate_grid(&e, &w, lo, hi);
        assert!((num / den - 3.7).abs() < 1e-9);
    }

    #[test]
    fn weighting_switches_at_the_declared_cut() {
        let w = WeightingSpectrum::ThermalMaxwellianEpithermalFlat { cut_ev: 0.5 };
        // Maxwellian flux ∝ E·exp(−E/kT) peaks at kT = 0.0253 eV:
        // rising below it, falling above it.
        assert!(w.w(0.01) > w.w(0.001));
        assert!(w.w(0.025) > w.w(0.1));
        // Above the cut: 1/E.
        assert!((w.w(1.0) - 1.0).abs() < 1e-12);
        assert!((w.w(1.0e6) - 1.0e-6).abs() < 1e-18);
    }
}
