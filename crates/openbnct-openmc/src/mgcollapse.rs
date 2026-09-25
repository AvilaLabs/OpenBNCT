// SPDX-License-Identifier: MIT

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
//!   weighting spectrum; when an MF7/MT4 tape is declared for a
//!   nuclide (`--tsl`), its incoherent-inelastic S(α,β) kernel
//!   replaces the free-gas kernel below the tape's E_max (thermal
//!   upscatter included); otherwise free-gas / no-upscatter applies
//!   and is declared as such;
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
/// eV → MeV. Energy grids and group boundaries are in eV, while every
/// dose-response term below is formed in barn·MeV before `MEV_TO_J`.
const EV_TO_MEV: f64 = 1.0e-6;
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

pub(crate) fn invalid(msg: impl Into<String>) -> CollapseError {
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
pub(crate) fn nuclide_mass(name: &str) -> Option<(f64, u32)> {
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

pub(crate) fn read_attr_f64(
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

pub(crate) fn read_attr_i64(
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

/// Log-log table evaluation at `e` (log-E/log-σ when both sides are
/// positive, else linear), clamped at the grid ends — the convention
/// shared with [`integrate_grid`]. Cross sections follow power-law
/// shapes (1/v tails, thresholds) where log-log interpolation is far
/// more faithful than linear-in-E.
pub(crate) fn log_interp(energy: &[f64], f: &[f64], e: f64) -> f64 {
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
}

/// Integrate `f(E)` over `[lo, hi]` by trapezoid on the nuclide's own
/// grid points plus boundary samples from [`log_interp`].
pub(crate) fn integrate_grid(energy: &[f64], f: &[f64], lo: f64, hi: f64) -> f64 {
    let interp = |e: f64| log_interp(energy, f, e);
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
    elastic_transfer_pl(e, alpha, mass_number, 1, lo, hi)
}

/// Legendre polynomial P_l(x) via the three-term recurrence (local
/// copy — the collapse cannot reach into the transport crate).
fn legendre_p(l: u32, x: f64) -> f64 {
    match l {
        0 => 1.0,
        1 => x,
        _ => {
            let (mut p0, mut p1) = (1.0, x);
            for n in 2..=l {
                let nf = n as f64;
                let p = ((2.0 * nf - 1.0) * x * p1 - (nf - 1.0) * p0) / nf;
                p0 = p1;
                p1 = p;
            }
            p1
        }
    }
}

/// l-th Legendre moment of the iso-CM elastic transfer restricted to
/// `[lo, hi]` — the generalization of [`elastic_transfer_p1`]: the
/// outgoing lab cosine raised through P_l instead of μ_lab itself.
/// ∫_{lo}^{hi} P_l(μ_lab(E,E'))/(E(1−α)) dE′ over the reachable
/// window; `pl(g→g')/p0(g→g')` is the l-th kernel moment of transfers
/// into the destination group.
fn elastic_transfer_pl(e: f64, alpha: f64, mass_number: f64, l: u32, lo: f64, hi: f64) -> f64 {
    if e <= 0.0 {
        return 0.0;
    }
    let elo = alpha * e;
    let lo_c = elo.max(lo);
    let hi_c = e.min(hi);
    if hi_c <= lo_c {
        return 0.0;
    }
    // Midpoint quadrature over the overlap — P_l(μ_lab) is smooth in
    // E′ for l ≤ 5 at this resolution.
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
        acc += legendre_p(l, mu_lab);
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
    /// Per-nuclide MF7/MT4 thermal-scattering-law tape paths. For a
    /// listed nuclide the incoherent-inelastic bound-atom kernel
    /// replaces the free-gas elastic kernel below the tape's E_max,
    /// with free-gas retained for the residual far-downscatter and all
    /// higher energies.
    pub tsl_paths: BTreeMap<String, PathBuf>,
    /// Material temperature for the S(α,β) evaluation — the nearest
    /// tabulated temperature on each tape is used.
    pub tsl_temperature_k: f64,
    /// Bondarenko heterogeneous-dilution self-shielding. When set, each
    /// nuclide's collapse weight becomes
    /// `w(E)·σ₀_n(E)/(σ_t,n(E)+σ₀_n(E))` with the per-atom background
    /// `σ₀_n(E) = Σ_{m≠n} (n_m/n_n)·σ_t,m(E)` — the other nuclides in
    /// the same material supply the dilution automatically (no free
    /// parameter). Resonance dips in σ_t,n then suppress that
    /// nuclide's effective weighting locally — the correct first-order
    /// self-shielding treatment. Recorded in the declaration.
    pub self_shielding: bool,
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

    // Parse every declared TSL tape once — materials share nuclides.
    let mut tsl_kernels: BTreeMap<String, crate::endf_mf7::SabKernel> = BTreeMap::new();
    for (name, path) in &opts.tsl_paths {
        let text = std::fs::read_to_string(path)
            .map_err(|e| CollapseError::Model(format!("TSL tape {}: {e}", path.display())))?;
        let tsl = crate::endf_mf7::parse_tsl(&text)
            .map_err(|e| CollapseError::Model(format!("TSL tape {}: {e}", path.display())))?;
        let kern = crate::endf_mf7::SabKernel::build(&tsl, opts.tsl_temperature_k)
            .map_err(|e| CollapseError::Model(format!("TSL tape {}: {e}", path.display())))?;
        tsl_kernels.insert(name.clone(), kern);
    }

    let mut materials = Vec::with_capacity(opts.materials.len());
    for material in &opts.materials {
        materials.push(collapse_material(opts, &tsl_kernels, material, groups)?);
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
    let tsl_note = if tsl_kernels.is_empty() {
        "no thermal upscatter — free-gas model, molecular binding \
         (S(α,β)) not included."
            .to_string()
    } else {
        format!(
            "bound-atom incoherent-inelastic S(α,β) transfer applied \
             for [{}] at {} K (nearest tabulated T per tape; ENDF-102 \
             DDXS (σ_b/4πkT)·√(E'/E)·e^(−β/2)·S(α,|β|) integrated over \
             E'∈[E±β_max·kT] and μ, symmetric-β extension verified \
             against detailed balance; free-gas retained for the \
             |ΔE|>β_max·kT residual and E>E_max; thermal upscatter \
             included via the bound kernel). Tapes: {}.",
            tsl_kernels.keys().cloned().collect::<Vec<_>>().join(", "),
            opts.tsl_temperature_k,
            opts.tsl_paths
                .iter()
                .map(|(k, v)| format!("{k}={}", v.display()))
                .collect::<Vec<_>>()
                .join(", "),
        )
    };
    let shield_note = if opts.self_shielding {
        " Bondarenko heterogeneous-dilution self-shielding applied: \
         each nuclide's weighting ×σ₀_n(E)/(σ_t,n(E)+σ₀_n(E)) with \
         σ₀_n = Σ_{m≠n}(n_m/n_n)σ_t,m from the material's own \
         composition."
    } else {
        ""
    };
    let mut declaration = format!(
        "Collapsed from the processed ENDF/B-VIII.1 294 K OpenMC HDF5 \
         tables in {} by `openbnct sn collapse` at {groups} groups. \
         sigma_total = collapsed elastic + summed non-elastic removal \
         (capture, (n,p), (n,α), inelastic, other; inelastic energy \
         return neglected). Scatter = analytic P0 isotropic-in-CM \
         elastic kernel, α = ((A−1)/(A+1))², with the P1 lab-cosine \
         transfer moments emitted alongside (scatter_p1_matrix_per_cm); \
         {tsl_note} \
         Dose responses are mass-kerma coefficients (Gy·cm² per unit \
         fluence): boron = 10B(n,α) charged kerma with declared \
         2.34 MeV (branch-weighted; the table's lumped Q is absent), \
         nitrogen = 14N(n,p) with declared 0.626 MeV, hydrogen = \
         elastic recoil kerma (Ē(1−α)/2 summed over all nuclides — \
         bound-atom nuclides use the TSL kernel's ∫(E−E')σ dE' \
         instead), photon = capture-γ kerma from table MT102 Q values \
         plus the 10B (n,α₁) 0.478 MeV γ (0.449 MeV branch-weighted), \
         all depositing locally — no photon transport. Other \
         charged-particle channels (e.g. 17O(n,α)) remain in σt \
         removal but are not folded into a named component. \
         Weighting: {}.{}{}{}",
        opts.library_dir.display(),
        opts.weighting.describe(),
        endf_note,
        shield_note,
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

/// Bound-atom/free-gas transfer from incident energy `e` into
/// `[lo, hi]` for a TSL-treated nuclide: `(P0 σ, P1 σ, recoil-kerma σ)`
/// in barns. The incoherent-inelastic kernel covers
/// `|E'−E| ≤ β_max·kT`; free-gas covers the residual downscatter
/// window (or the full window above E_max).
fn tsl_group_transfer(
    kern: &crate::endf_mf7::SabKernel,
    e: f64,
    sigma_free: f64,
    alpha: f64,
    mass: f64,
    lo: f64,
    hi: f64,
) -> (f64, f64, f64) {
    let mut s0 = 0.0;
    let mut s1 = 0.0;
    let mut k = 0.0;
    if e <= kern.emax_ev {
        let tlo = lo.max(e - kern.delta_ev).max(0.0);
        let thi = hi.min(e + kern.delta_ev);
        if thi > tlo {
            const NEP: usize = 48;
            let st = (thi - tlo) / NEP as f64;
            for m in 0..NEP {
                let ep = tlo + (m as f64 + 0.5) * st;
                let d0 = kern.sigma0_de(e, ep) * st;
                s0 += d0;
                s1 += kern.sigma1_de(e, ep) * st;
                // Recoil kerma counts deposited energy only — an
                // upscattered neutron takes energy FROM the bath and
                // deposits nothing.
                k += (e - ep).max(0.0) * d0;
            }
        }
    }
    // Free-gas remainder: E' ∈ [αe, e−Δ] inside the group — or the
    // full [αe, e] window when the bound law does not apply.
    let fhi = hi.min(if e <= kern.emax_ev {
        e - kern.delta_ev
    } else {
        e
    });
    let flo = lo.max(alpha * e);
    if fhi > flo {
        let p0 = elastic_transfer_p(e, alpha, flo, fhi);
        let p1 = elastic_transfer_p1(e, alpha, mass, flo, fhi);
        s0 += sigma_free * p0;
        s1 += sigma_free * p1;
        k += sigma_free * p0 * (e - 0.5 * (flo + fhi));
    }
    (s0, s1, k)
}

/// Collapse one material: group σt, the P0 elastic transfer matrix,
/// and the four mass-kerma dose-response vectors.
fn collapse_material(
    opts: &CollapseOptions,
    tsl_kernels: &BTreeMap<String, crate::endf_mf7::SabKernel>,
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
    // l = 2..=5 Legendre transfer moments — the free-gas iso-CM kernel
    // supplies them analytically for every nuclide (for TSL-treated
    // nuclides the bound-atom kernel currently exports only s0/s1/kj,
    // so l ≥ 2 carries the free-gas moment — a declared approximation
    // whose error sits at thermal energies where anisotropy is weak).
    let mut transfer_pl: [Vec<f64>; 4] = [
        vec![0.0; groups * groups],
        vec![0.0; groups * groups],
        vec![0.0; groups * groups],
        vec![0.0; groups * groups],
    ];
    let mut dose_boron = vec![0.0; groups];
    let mut dose_nitrogen = vec![0.0; groups];
    let mut dose_hydrogen = vec![0.0; groups];
    let mut dose_photon = vec![0.0; groups];
    // Transport-correction accumulators: μ̄_g = Σ_n n_n σ̄s_n,g
    // (2/3A_n) / Σ_n n_n σ̄s_n,g — the scatter-weighted mean lab cosine
    // for iso-CM elastic.
    let mut mu_num = vec![0.0; groups];
    let mut mu_den = vec![0.0; groups];

    // Bondarenko pre-pass: per-nuclide total XS (barns) on its own
    // grid — the dilution background for every other nuclide.
    let total_xs: Vec<Vec<f64>> = tables
        .iter()
        .map(|t| {
            (0..t.energy.len())
                .map(|i| t.elastic[i] + t.capture.0[i] + t.np.0[i] + t.na.0[i] + t.other[i])
                .collect()
        })
        .collect();
    // σ₀_n(E) in macroscopic /cm: Σ_{m≠n} n_m·σ_t,m(E). The shield
    // factor σ₀/(σ_t,n·n_n + σ₀) is dimensionless and recovers the
    // per-atom Bondarenko weight automatically.
    let sigma0_macro = |skip: usize, e: f64| -> f64 {
        tables
            .iter()
            .enumerate()
            .zip(&densities)
            .filter(|((m, _), _)| *m != skip)
            .map(|((m, t), &n)| n * log_interp(&t.energy, &total_xs[m], e))
            .sum()
    };

    for (n_idx, (table, &n_density)) in tables.iter().zip(&densities).enumerate() {
        let alpha = ((table.mass_number as f64 - 1.0) / (table.mass_number as f64 + 1.0)).powi(2);
        let e = &table.energy;
        // Bondarenko shield factor at e: σ₀/(σ_t,n·n_n + σ₀) — 1.0 when
        // self-shielding is off or the nuclide has no dilution partner
        // (a single-nuclide material's shielding is a spatial transport
        // effect, not a collapse correction — zeroing its weight would
        // produce a meaningless empty group integral). Applied to every
        // weighting integral below (grid weights and the TSL explicit
        // quadrature alike).
        let shield_factor = |e: f64| -> f64 {
            if !opts.self_shielding {
                return 1.0;
            }
            let s0 = sigma0_macro(n_idx, e);
            if s0 <= 0.0 {
                return 1.0;
            }
            let st = log_interp(&table.energy, &total_xs[n_idx], e) * n_density;
            if s0 + st > 0.0 { s0 / (s0 + st) } else { 1.0 }
        };
        let weight: Vec<f64> = e
            .iter()
            .map(|&x| opts.weighting.w(x) * shield_factor(x))
            .collect();
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

            let sigma_a = collapse(&sw(&absorption));

            // Elastic transfer out of g: T[g][g'] = N ∫σs·W·P(E→g') / ∫W.
            // P varies within g, so integrate the product numerically.
            let mut row = vec![0.0; groups];
            let mut row_p1 = vec![0.0; groups];
            let mut row_pl = [
                vec![0.0; groups],
                vec![0.0; groups],
                vec![0.0; groups],
                vec![0.0; groups],
            ];
            let a = table.mass_number as f64;
            let tsl = tsl_kernels.get(&table.name);
            // σ_s and the recoil-kerma σ·⟨E−E'⟩ depend on which kernel
            // drives the transfer.
            let sigma_s;
            let recoil_sigma;
            if let Some(kern) = tsl {
                // Bound-atom S(α,β): explicit E-quadrature — the kernel
                // is not separable as σ(E)·P(E).
                const NE: usize = 24;
                let mut wsum = 0.0;
                let mut s_acc = 0.0;
                let mut k_acc = 0.0;
                for j in 0..NE {
                    let e_j = lo + (j as f64 + 0.5) * (hi - lo) / NE as f64;
                    let w_j = opts.weighting.w(e_j) * shield_factor(e_j);
                    let sig_f = log_interp(e, &table.elastic, e_j);
                    let mut s0tot = 0.0;
                    for gp in 0..groups {
                        let (s0, s1, kj) =
                            tsl_group_transfer(kern, e_j, sig_f, alpha, a, b[gp + 1], b[gp]);
                        row[gp] += w_j * s0;
                        row_p1[gp] += w_j * s1;
                        s0tot += s0;
                        k_acc += w_j * kj;
                    }
                    s_acc += w_j * s0tot;
                    wsum += w_j;
                }
                // The j-accumulation carries the weighting integral —
                // divide it out so the row matches σ_s (and the
                // row-sum ≤ σ_t invariant holds).
                for val in row.iter_mut() {
                    *val /= wsum;
                }
                for val in row_p1.iter_mut() {
                    *val /= wsum;
                }
                sigma_s = s_acc / wsum;
                // k carries σ·⟨E−E'⟩ in barn·eV — the kerma conversion
                // below is per MeV.
                recoil_sigma = k_acc / wsum * EV_TO_MEV;
            } else {
                sigma_s = collapse(&sw(&table.elastic));
                let elastic_weighted = sw(&table.elastic);
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
                // Iso-CM elastic mean recoil: Ē·(1−α)/2.
                // Ē comes from the eV grid; the kerma conversion is per MeV.
                let ebar_ev = collapse(
                    &e.iter()
                        .zip(&table.elastic)
                        .zip(&weight)
                        .map(|((x, s), w)| x * s * w)
                        .collect::<Vec<_>>(),
                ) / sigma_s.max(f64::MIN_POSITIVE);
                recoil_sigma = sigma_s * ebar_ev * EV_TO_MEV * (1.0 - alpha) / 2.0;
            }
            // Higher Legendre moments (l = 2..=5) from the free-gas
            // iso-CM kernel for every nuclide; for TSL-treated nuclides
            // the free-gas row is scaled by σ_s,bound/σ_s,free so the
            // moment magnitude tracks the bound-atom scatter strength
            // (declared approximation — the TSL kernel exports no
            // l ≥ 2 moments).
            let sigma_s_free = collapse(&sw(&table.elastic));
            let scale_pl = if tsl.is_some() && sigma_s_free > 0.0 {
                sigma_s / sigma_s_free
            } else {
                1.0
            };
            let elastic_weighted_pl = sw(&table.elastic);
            for gp in 0..groups {
                for (li, row_l) in row_pl.iter_mut().enumerate() {
                    let l = li as u32 + 2;
                    let integrand: Vec<f64> = e
                        .iter()
                        .enumerate()
                        .map(|(i, &x)| {
                            elastic_weighted_pl[i]
                                * elastic_transfer_pl(x, alpha, a, l, b[gp + 1], b[gp])
                        })
                        .collect();
                    row_l[gp] = collapse(&integrand) * scale_pl;
                }
            }
            for (gp, val) in row.iter().enumerate() {
                transfer[g * groups + gp] += n_density * val;
            }
            for (gp, val) in row_p1.iter().enumerate() {
                transfer_p1[g * groups + gp] += n_density * val;
            }
            for (li, mat) in transfer_pl.iter_mut().enumerate() {
                for (gp, val) in row_pl[li].iter().enumerate() {
                    // Realizability bound: |P_l moment| ≤ P0 for any
                    // positive kernel (|P_l(μ)| ≤ 1). The free-gas
                    // estimate can exceed the bound-atom row on TSL
                    // upscatter pairs — clamp to the final P0 entry.
                    let bound = row[gp].max(0.0);
                    mat[g * groups + gp] += n_density * val.clamp(-bound, bound);
                }
            }
            sigma_t[g] += n_density * (sigma_s + sigma_a);
            // Scatter-weighted mean lab cosine: analytic 2/(3A) for
            // free-gas, the TSL P1/P0 ratio when bound-atom applies.
            if tsl.is_some() {
                mu_num[g] += n_density * row_p1.iter().sum::<f64>();
                mu_den[g] += n_density * row.iter().sum::<f64>();
            } else {
                let mu = 2.0 / (3.0 * table.mass_number as f64);
                mu_num[g] += n_density * sigma_s * mu;
                mu_den[g] += n_density * sigma_s;
            }

            // Dose responses (mass-kerma per unit fluence, Gy·cm²).
            let conv = MEV_TO_J * G_PER_KG / rho;
            // "hydrogen" — elastic recoil kerma summed over all
            // nuclides (overwhelmingly H). For the free-gas path this
            // is σ_s·Ē·(1−α)/2; for a bound-atom nuclide the TSL
            // kernel's ∫(E−E')σ dE' integral is used directly.
            dose_hydrogen[g] += n_density * recoil_sigma * conv;
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
        scatter_legendre_moments_per_cm: Some(transfer_pl.into()),
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

    /// A constant-S toy law: S(α,β) = 1 on α ∈ [0.01, 8], β ∈ {0, 8}
    /// at a single temperature — enough structure for the kernel's
    /// α-window integrals and the β extension to engage.
    fn toy_tsl() -> crate::endf_mf7::ThermalScatteringLaw {
        let alpha: Vec<f64> = (0..40).map(|i| 0.01 * (1.25_f64).powi(i)).collect();
        let mk = |beta: f64| crate::endf_mf7::SabSection {
            temperature_k: 300.0,
            beta,
            li: 0,
            alpha: alpha.clone(),
            s: vec![1.0; alpha.len()],
        };
        crate::endf_mf7::ThermalScatteringLaw {
            za: 1001.0,
            awr: 1.0,
            sigma_b_barns: 40.0,
            natom: 2.0,
            beta_max: 8.0,
            emax_ev: 10.0,
            sections: vec![mk(0.0), mk(8.0)],
            temperatures_k: vec![300.0],
            betas: vec![0.0, 8.0],
        }
    }

    #[test]
    fn tsl_transfer_upscatters_and_free_gases_above_emax() {
        let kern = crate::endf_mf7::SabKernel::build(&toy_tsl(), 300.0).unwrap();
        let b = test_boundaries(20.0, 1.0e-4, 40);
        let e = 0.05; // well below E_max
        // Find the group containing e and the group directly above it.
        let g_src = (0..b.len() - 1)
            .find(|&g| e <= b[g] && e > b[g + 1])
            .unwrap();
        let g_up = g_src - 1;
        // Thermal upscatter: a destination above E gets weight — under
        // free-gas this transfer is identically zero.
        let (s0_up, _, _) = tsl_group_transfer(&kern, e, 20.0, 0.0, 1.0, b[g_up + 1], b[g_up]);
        let free_up = elastic_transfer_p(e, 0.0, b[g_up + 1], b[g_up]);
        assert_eq!(free_up, 0.0, "free-gas upscatter must be zero");
        assert!(s0_up > 0.0, "bound-atom upscatter must be positive");
        // In-group + downscatter also positive.
        let (s0_src, s1_src, k_src) =
            tsl_group_transfer(&kern, e, 20.0, 0.0, 1.0, b[g_src + 1], b[g_src]);
        assert!(s0_src > 0.0 && k_src != 0.0);
        assert!(s1_src.abs() <= s0_src + 1e-30);
        // Above E_max the kernel reverts to pure free-gas: no upscatter
        // and the row equals σ_free·P.
        let e_hi = 12.0; // inside group [11, 14.8]; group 0 sits above it
        let g_hi_src = (0..b.len() - 1)
            .find(|&g| e_hi <= b[g] && e_hi > b[g + 1])
            .unwrap();
        let (s0_hi, _, _) =
            tsl_group_transfer(&kern, e_hi, 20.0, 0.0, 1.0, b[g_hi_src + 1], b[g_hi_src]);
        let free_hi = 20.0 * elastic_transfer_p(e_hi, 0.0, b[g_hi_src + 1], b[g_hi_src]);
        assert!((s0_hi - free_hi).abs() / free_hi < 1e-9);
        // Upscatter above E_max stays zero (group 0 = [14.8, 20] is
        // wholly above the source energy).
        let (s0_up_hi, _, _) = tsl_group_transfer(&kern, e_hi, 20.0, 0.0, 1.0, b[1], b[0]);
        assert_eq!(s0_up_hi, 0.0);
    }

    #[test]
    fn tsl_transfer_rows_sum_to_total_cross_section() {
        // Over a group structure covering the whole β domain, the
        // transfer row must recover σ_s,total(E) = ∫σ(E→E')dE' plus
        // the free-gas remainder — i.e. it partitions the kernel's
        // own total, not the free-atom value.
        let kern = crate::endf_mf7::SabKernel::build(&toy_tsl(), 300.0).unwrap();
        let b = test_boundaries(30.0, 1.0e-4, 60);
        let e = 0.4;
        let row: f64 = (0..b.len() - 1)
            .map(|g| tsl_group_transfer(&kern, e, 20.0, 0.0, 1.0, b[g + 1], b[g]).0)
            .sum();
        // Direct quadrature of the same kernel over its domain.
        let (tlo, thi) = (0.0_f64, e + kern.delta_ev);
        let mut direct = 0.0;
        let n = 4000;
        for i in 0..n {
            let ep = tlo + (i as f64 + 0.5) * (thi - tlo) / n as f64;
            direct += kern.sigma0_de(e, ep) * (thi - tlo) / n as f64;
        }
        assert!(
            (row - direct).abs() / direct < 2e-3,
            "group-sum {row} vs direct integral {direct}"
        );
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

    /// One ENDF-6 record line: six 11-column fields, MAT, MF, MT, sequence.
    fn endf_line(fields: [&str; 6], mat: u32, mf: u32, mt: u32, seq: u32) -> String {
        let mut line = String::new();
        for field in fields {
            line.push_str(&format!("{field:>11}"));
        }
        line.push_str(&format!("{mat:>4}{mf:>2}{mt:>3}{seq:>5}"));
        line
    }

    /// ENDF-6 real in 11 columns with an explicit exponent (the reader
    /// accepts the `E` form).
    fn endf_real(v: f64) -> String {
        format!("{v:.5E}")
    }

    #[test]
    fn hydrogen_recoil_kerma_is_in_gray_cm2() {
        // Pure ¹H with a constant 20 b elastic cross section, collapsed
        // into one group [1.0, 1.1] MeV under 1/E weighting. Iso-CM
        // elastic on A = 1 deposits Ē/2 per collision, so the kerma
        // factor is independent of density:
        //   K = (N_A·10⁻²⁴/M_H)·σ·(Ē/2)·(J/MeV)·(g/kg)
        // with Ē = ΔE/ln(1.1) under 1/E weighting. Before the eV→MeV fix
        // this came out 10⁶ too large.
        let dir = tempfile::tempdir().unwrap();
        let (mat, mt) = (125, 2);
        let mut lines = vec![endf_line(
            ["1.00100E+3", "9.99167E-1", "0", "0", "0", "0"],
            mat,
            3,
            mt,
            1,
        )];
        let grid: Vec<f64> = (0..)
            .map(|i| 1.0e-5 * 1.005_f64.powi(i))
            .take_while(|e| *e <= 2.0e7)
            .collect();
        let np = grid.len();
        lines.push(endf_line(
            ["0.00000E+0", "0.00000E+0", "0", "0", "1", &np.to_string()],
            mat,
            3,
            mt,
            2,
        ));
        lines.push(endf_line(
            [&np.to_string(), "2", "", "", "", ""],
            mat,
            3,
            mt,
            3,
        ));
        for (row, chunk) in grid.chunks(3).enumerate() {
            let mut fields: Vec<String> = Vec::new();
            for e in chunk {
                fields.push(endf_real(*e));
                fields.push(endf_real(20.0));
            }
            fields.resize(6, String::new());
            let f: [&str; 6] = std::array::from_fn(|i| fields[i].as_str());
            lines.push(endf_line(f, mat, 3, mt, 4 + row as u32));
        }
        let tape = dir.path().join("H1.endf");
        std::fs::write(&tape, lines.join("\n") + "\n").unwrap();

        let material = MaterialDefinition {
            schema_version: "openbnct.material/0.1.0".into(),
            id: "test.pure-hydrogen".into(),
            density_g_cm3: 0.5,
            temperature_k: 294.0,
            nuclides: vec![openbnct_transport::NuclideMassFraction {
                name: "H1".into(),
                mass_fraction: 1.0,
            }],
            neutron_thermal_treatment: openbnct_transport::NeutronThermalTreatment::FreeGas,
            boron_microdistribution: None,
        };
        let opts = CollapseOptions {
            library_dir: dir.path().to_path_buf(),
            endf_paths: [("H1".to_string(), tape)].into_iter().collect(),
            materials: vec![material],
            energy_boundaries_ev: vec![1.1e6, 1.0e6],
            weighting: WeightingSpectrum::FlatLethargy,
            tsl_paths: BTreeMap::new(),
            tsl_temperature_k: 294.0,
            self_shielding: false,
            id: "test.hydrogen-kerma".into(),
            component_profile: None,
            note: String::new(),
        };
        let data = collapse_multigroup(&opts).unwrap();
        let got = data.materials[0].dose_response_gy_cm2["hydrogen"][0];
        let e_bar_mev = 0.1 / 1.1_f64.ln();
        let want = N_A * 1.0e-24 / 1.007_825 * 20.0 * (e_bar_mev / 2.0) * MEV_TO_J * 1.0e3;
        assert!(
            (got - want).abs() / want < 1.0e-3,
            "hydrogen kerma {got:e} Gy·cm², expected {want:e}"
        );
        // Sanity anchor: ~1.0e-9 Gy·cm² for pure hydrogen near 1 MeV.
        assert!((0.9e-9..1.1e-9).contains(&got), "got {got:e}");
    }

    /// Bondarenko heterogeneous-dilution self-shielding: a narrow
    /// resonance spike in the resonant nuclide's σ_t must collapse
    /// *down* when the dilution weight σ₀/(σ_t+σ₀) is applied —
    /// the spike's local flux depression suppresses its contribution.
    /// B10 carries flat 1 b capture plus a 10⁴ b spike over
    /// [9.9e4, 1.01e5] eV (~10% of the group's lethargy); O16 is a
    /// flat 5 b elastic diluent. Unshielded σ̄_t ≈ 10³ b for the
    /// resonant component; shielded ≈ its ~1.5 b off-resonance value —
    /// a ~700× separation the material σ_t must show.
    #[test]
    fn bondarenko_shielding_suppresses_narrow_resonance() {
        let dir = tempfile::tempdir().unwrap();
        let mat = 125;
        // Shared base grid, plus spike-boundary points for B10's
        // capture section so the resonance edges are explicit.
        let grid: Vec<f64> = (0..)
            .map(|i| 1.0e-5 * 1.005_f64.powi(i))
            .take_while(|e| *e <= 2.0e7)
            .collect();
        let write_tape = |name: &str, sections: &[(u32, &[(f64, f64)])]| {
            let mut lines = Vec::new();
            for (sidx, (mt, points)) in sections.iter().enumerate() {
                let np = points.len();
                lines.push(endf_line(
                    ["1.00100E+3", "9.99167E-1", "0", "0", "0", "0"],
                    mat,
                    3,
                    *mt,
                    (1 + sidx * 1000) as u32,
                ));
                lines.push(endf_line(
                    ["0.00000E+0", "0.00000E+0", "0", "0", "1", &np.to_string()],
                    mat,
                    3,
                    *mt,
                    2,
                ));
                lines.push(endf_line(
                    [&np.to_string(), "2", "", "", "", ""],
                    mat,
                    3,
                    *mt,
                    3,
                ));
                for (seq, chunk) in (4 + sidx * 1000..).zip(points.chunks(3)) {
                    let mut fields: Vec<String> = Vec::new();
                    for (e, s) in chunk {
                        fields.push(endf_real(*e));
                        fields.push(endf_real(*s));
                    }
                    fields.resize(6, String::new());
                    let f: [&str; 6] = std::array::from_fn(|i| fields[i].as_str());
                    lines.push(endf_line(f, mat, 3, *mt, seq as u32));
                }
            }
            let tape = dir.path().join(format!("{name}.endf"));
            std::fs::write(&tape, lines.join("\n") + "\n").unwrap();
            tape
        };
        // B10: flat 0.5 b elastic + 1 b capture with a 10⁴ b resonance
        // spike on [9.9e4, 1.01e5].
        let b10_elastic: Vec<(f64, f64)> = grid.iter().map(|&e| (e, 0.5)).collect();
        let mut b10_energies: Vec<f64> = grid.iter().copied().chain([9.9e4, 1.01e5]).collect();
        b10_energies.sort_by(f64::total_cmp);
        b10_energies.dedup();
        let b10_capture: Vec<(f64, f64)> = b10_energies
            .into_iter()
            .map(|e| {
                (
                    e,
                    if (9.9e4..=1.01e5).contains(&e) {
                        1.0e4
                    } else {
                        1.0
                    },
                )
            })
            .collect();
        // O16: flat 5 b elastic, nothing else.
        let o16_elastic: Vec<(f64, f64)> = grid.iter().map(|&e| (e, 5.0)).collect();
        let b10 = write_tape("B10", &[(2, &b10_elastic), (102, &b10_capture)]);
        let o16 = write_tape("O16", &[(2, &o16_elastic)]);
        let material = MaterialDefinition {
            schema_version: "openbnct.material/0.1.0".into(),
            id: "test.b10-in-o16".into(),
            density_g_cm3: 1.0,
            temperature_k: 294.0,
            nuclides: vec![
                openbnct_transport::NuclideMassFraction {
                    name: "B10".into(),
                    mass_fraction: 0.5,
                },
                openbnct_transport::NuclideMassFraction {
                    name: "O16".into(),
                    mass_fraction: 0.5,
                },
            ],
            neutron_thermal_treatment: openbnct_transport::NeutronThermalTreatment::FreeGas,
            boron_microdistribution: None,
        };
        let opts = |shield: bool| CollapseOptions {
            library_dir: dir.path().to_path_buf(),
            endf_paths: [
                ("B10".to_string(), b10.clone()),
                ("O16".to_string(), o16.clone()),
            ]
            .into_iter()
            .collect(),
            materials: vec![material.clone()],
            energy_boundaries_ev: vec![1.1e5, 9.0e4],
            weighting: WeightingSpectrum::FlatLethargy,
            tsl_paths: BTreeMap::new(),
            tsl_temperature_k: 294.0,
            self_shielding: shield,
            id: "test.shielding".into(),
            component_profile: None,
            note: String::new(),
        };
        let unshielded = collapse_multigroup(&opts(false)).unwrap();
        let shielded = collapse_multigroup(&opts(true)).unwrap();
        let st_un = unshielded.materials[0].sigma_total_per_cm[0];
        let st_sh = shielded.materials[0].sigma_total_per_cm[0];
        // Unshielded: n_B·σ̄_B ≈ 0.03·~10³ ≈ 30 /cm (spike dominates);
        // shielded ≈ 0.03·~1.5 + 0.019·5 ≈ 0.14 /cm — off-resonance.
        assert!(
            st_un > 5.0,
            "unshielded σ_t {st_un} should be resonance-dominated"
        );
        assert!(
            st_sh < st_un / 50.0,
            "shielded σ_t {st_sh} not suppressed vs unshielded {st_un}"
        );
        assert!(
            (0.02..0.5).contains(&st_sh),
            "shielded σ_t {st_sh} far from the ~0.14 off-resonance limit"
        );
    }
}
