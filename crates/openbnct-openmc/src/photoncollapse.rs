// SPDX-License-Identifier: Apache-2.0

//! Coupled photon multigroup collapse: turns the processed
//! photon-atomic library (`photo_<Elem>.h5` — incoherent, coherent,
//! photoelectric, pair-production tables) plus the neutron
//! evaluations' photon-production products into a
//! `openbnct.multigroup-photon-data/0.1.0` artifact for the two-pass
//! coupled solve.
//!
//! Physics model, declared honestly:
//! - Incoherent (Compton) transfer uses the free-electron Klein–Nishina
//!   kernel — strictly downscatter, no bound-electron Doppler
//!   broadening. The tabulated incoherent XS sets each row's scale;
//!   the kernel sets its shape.
//! - Coherent (Rayleigh) scattering is elastic: it stays on the
//!   in-group diagonal and contributes a forward bias to μ̄.
//! - Pair production is treated as absorption plus a two-photon
//!   annihilation source at 511 keV — the annihilation photons are
//!   transported (they deposit where they later interact), which is
//!   the correct multigroup treatment rather than counting them as
//!   local kerma.
//! - Photoelectric absorption deposits the full incident energy
//!   locally (fluorescence/Auger escape is keV-scale and ignored).
//! - The n→γ production matrix folds each neutron evaluation's
//!   photon products (discrete lines and continuous spectra, per the
//!   OpenMC HDF5 layout) through the same weighting spectrum the
//!   neutron collapse used — required for flux consistency.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hdf5_pure::File;
use openbnct_core::ContentReference;
use openbnct_transport::{
    MULTIGROUP_PHOTON_DATA_SCHEMA, MaterialDefinition, MultigroupPhotonData, PhotonMaterial,
};

use crate::mgcollapse::{
    CollapseError, WeightingSpectrum, log_interp, nuclide_mass, read_attr_f64, read_attr_i64,
};

fn invalid(msg: impl Into<String>) -> CollapseError {
    CollapseError::Invalid(msg.into())
}

/// Electron rest energy, eV — sets the Klein–Nishina recoil scale and
/// the 511 keV annihilation line.
const MC2_EV: f64 = 510_999.0;
/// The (n,α₁γ) branch's de-excitation photon on ¹⁰B, eV — the dominant
/// prompt line in BNCT photon production, 94% branch per capture.
const B10_NA1G_PHOTON_EV: f64 = 478_000.0;
const B10_NA1G_BRANCH: f64 = 0.94;
/// eV·cm⁻¹ → Gy·cm² per unit fluence at 1 g/cm³: dose = fluence×Σ·E_dep
/// converted J→Gy via material density (applied with the material's ρ).
const EV_CM_TO_GY_CM2_DENSITY: f64 = 1.602_176_634e-16;
const N_A: f64 = 6.022_140_76e23;
const BARN_CM2: f64 = 1.0e-24;

/// Per-element photon-atomic table on its shared energy grid.
struct PhotonElement {
    energy: Vec<f64>,
    incoherent: Vec<f64>,
    coherent: Vec<f64>,
    photoelectric: Vec<f64>,
    pair_nuclear: Vec<f64>,
    pair_electron: Vec<f64>,
}

/// One neutron evaluation's photon-producing product.
struct PhotonProduct {
    /// Reaction MT (102, 4, …) — recorded for the declaration.
    #[allow(dead_code)]
    mt: i64,
    /// Reaction XS on the nuclide's shared energy grid (barns).
    xs: Vec<f64>,
    /// Photon yield vs incident neutron energy — 2-row (E, y) table.
    yield_table: Vec<(f64, f64)>,
    /// Outgoing photon spectrum per incident energy.
    spectrum: ProductSpectrum,
}

enum ProductSpectrum {
    /// `discrete_photon`: a single line at `line_ev` for every
    /// incident energy.
    DiscreteLine { line_ev: f64 },
    /// `continuous`: per-incident-breakpoint outgoing tables. Each
    /// table is `(point_masses, pdf_rows)` — the first
    /// `n_discrete_lines` entries of an HDF5 run are discrete line
    /// weights (E_out, probability); the rest a continuous pdf.
    Continuous {
        energy: Vec<f64>,
        tables: Vec<OutgoingTable>,
    },
}

/// `(point_masses, pdf_rows)` for one incident-energy breakpoint.
type OutgoingTable = (Vec<(f64, f64)>, Vec<(f64, f64)>);

/// Photon spectrum fraction of `spectrum` landing in photon group
/// `[lo, hi)` (eV, ascending) at incident neutron energy `e_n`.
fn spectrum_fraction(spectrum: &ProductSpectrum, e_n: f64, lo: f64, hi: f64) -> f64 {
    match spectrum {
        ProductSpectrum::DiscreteLine { line_ev } => {
            if *line_ev >= lo && *line_ev < hi {
                1.0
            } else {
                0.0
            }
        }
        ProductSpectrum::Continuous { energy, tables } => {
            if tables.is_empty() {
                return 0.0;
            }
            // Nearest incident breakpoint — outgoing spectra vary
            // slowly with E_n.
            let i = energy
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| (*a - e_n).abs().total_cmp(&(*b - e_n).abs()))
                .map(|(i, _)| i)
                .unwrap_or(0)
                .min(tables.len() - 1);
            let (lines, pdf) = &tables[i];
            let mut frac: f64 = lines
                .iter()
                .filter(|(e, _)| *e >= lo && *e < hi)
                .map(|(_, p)| *p)
                .sum();
            // Piecewise-linear pdf integrated over the group.
            for w in pdf.windows(2) {
                let (e0, p0) = w[0];
                let (e1, p1) = w[1];
                if e1 <= e0 || e1 <= lo || e0 >= hi {
                    continue;
                }
                let a = e0.max(lo);
                let b = e1.min(hi);
                let pa = p0 + (p1 - p0) * (a - e0) / (e1 - e0);
                let pbb = p0 + (p1 - p0) * (b - e0) / (e1 - e0);
                frac += 0.5 * (pa + pbb) * (b - a);
            }
            frac.clamp(0.0, 1.0)
        }
    }
}

/// Klein–Nishina outgoing photon energy at incident `e`, cosine `mu`:
/// E′ = E/(1+α(1−μ)), α = E/mc².
fn kn_outgoing_energy(e: f64, mu: f64) -> f64 {
    e / (1.0 + (e / MC2_EV) * (1.0 - mu))
}

/// Unnormalized Klein–Nishina dσ/dμ at incident `e`, cosine `mu`:
/// (E′/E)²·(E/E′ + E′/E − 1 + μ²) — the r_e²/2 prefactor cancels in
/// the normalized fraction.
fn kn_kernel(e: f64, mu: f64) -> f64 {
    let ep = kn_outgoing_energy(e, mu);
    let r = ep / e;
    r * r * (1.0 / r + r - (1.0 - mu * mu))
}

/// Collapse the Klein–Nishina transfer at representative incident
/// energy `e_rep` into outgoing-group fractions and the mean cosine.
/// `boundaries` descend; group `gg` spans `(b[gg+1], b[gg]]`.
fn kn_transfer(e_rep: f64, boundaries: &[f64]) -> (Vec<f64>, f64) {
    let groups = boundaries.len() - 1;
    const N_MU: usize = 96;
    let mut frac = vec![0.0; groups];
    let mut mu_w = 0.0;
    let mut w_sum = 0.0;
    for i in 0..N_MU {
        let mu = -1.0 + 2.0 * (i as f64 + 0.5) / N_MU as f64;
        let w = kn_kernel(e_rep, mu) * (2.0 / N_MU as f64);
        let ep = kn_outgoing_energy(e_rep, mu);
        let gg = (0..groups)
            .find(|&g| ep <= boundaries[g] && ep > boundaries[g + 1])
            .unwrap_or(groups - 1);
        frac[gg] += w;
        mu_w += w * mu;
        w_sum += w;
    }
    if w_sum > 0.0 {
        for f in frac.iter_mut() {
            *f /= w_sum;
        }
    }
    (frac, if w_sum > 0.0 { mu_w / w_sum } else { 0.0 })
}

/// Weight-integrated average of a pointwise table over `[lo, hi]`.
fn group_average(energy: &[f64], table: &[f64], lo: f64, hi: f64, w: &WeightingSpectrum) -> f64 {
    let mut edges = vec![lo];
    edges.extend(energy.iter().copied().filter(|e| *e > lo && *e < hi));
    edges.push(hi);
    let mut acc = 0.0;
    let mut wacc = 0.0;
    for i in 0..edges.len() - 1 {
        let m = 0.5 * (edges[i] + edges[i + 1]);
        let wv = w.w(m);
        acc += log_interp(energy, table, m) * wv * (edges[i + 1] - edges[i]);
        wacc += wv * (edges[i + 1] - edges[i]);
    }
    if wacc > 0.0 { acc / wacc } else { 0.0 }
}

/// Load one element's photon-atomic table (`photo_<Elem>.h5`).
fn load_photon_element(dir: &Path, elem: &str) -> Result<PhotonElement, CollapseError> {
    let path = dir.join(format!("photo_{elem}.h5"));
    let file =
        File::open(&path).map_err(|e| CollapseError::Hdf5(format!("{}: {e}", path.display())))?;
    let root = file
        .group(&format!("/{elem}"))
        .map_err(|e| CollapseError::Hdf5(format!("{elem} group: {e}")))?;
    let read = |name: &str| -> Result<Vec<f64>, CollapseError> {
        root.group(name)
            .and_then(|g| g.dataset("xs"))
            .and_then(|d| d.read_f64())
            .map_err(|e| CollapseError::Hdf5(format!("{elem}/{name}: {e}")))
    };
    let energy = root
        .dataset("energy")
        .and_then(|d| d.read_f64())
        .map_err(|e| CollapseError::Hdf5(format!("{elem}/energy: {e}")))?;
    Ok(PhotonElement {
        energy,
        incoherent: read("incoherent")?,
        coherent: read("coherent")?,
        photoelectric: read("photoelectric")?,
        pair_nuclear: read("pair_production_nuclear")?,
        pair_electron: read("pair_production_electron")?,
    })
}

/// Nuclide → photon-atomic element symbol. Photon interaction is
/// electronic (atomic), so all isotopes of an element share one table.
fn element_of(nuclide: &str) -> Option<&'static str> {
    Some(match nuclide {
        "H1" | "H2" => "H",
        "B10" | "B11" => "B",
        "C12" | "C13" => "C",
        "N14" | "N15" => "N",
        "O16" | "O17" | "O18" => "O",
        _ => return None,
    })
}

/// Read a 2-row (E, y) table: flat `read_f64` plus `shape` reshape.
fn read_pair_table(ds: &hdf5_pure::Dataset) -> Vec<(f64, f64)> {
    let (Ok(v), Ok(shape)) = (ds.read_f64(), ds.shape()) else {
        return Vec::new();
    };
    if shape.len() == 2 && shape[0] == 2 {
        let n = shape[1] as usize;
        (0..n).map(|i| (v[i], v[n + i])).collect()
    } else if shape.len() == 2 && shape[1] == 2 {
        v.chunks_exact(2).map(|r| (r[0], r[1])).collect()
    } else {
        Vec::new()
    }
}

/// Load every photon-producing product of one neutron evaluation.
fn load_photon_products(
    nuc_group: &hdf5_pure::Group,
    name: &str,
    grid_len: usize,
) -> Result<Vec<PhotonProduct>, CollapseError> {
    let mut out = Vec::new();
    let Ok(reactions) = nuc_group.group("reactions") else {
        return Ok(out);
    };
    for rname in reactions
        .groups()
        .map_err(|e| CollapseError::Hdf5(format!("{name} reaction list: {e}")))?
    {
        let rg = reactions
            .group(&rname)
            .map_err(|e| CollapseError::Hdf5(format!("{name}/{rname}: {e}")))?;
        let mt = read_attr_i64(
            &rg.attrs()
                .map_err(|e| CollapseError::Hdf5(format!("{name}/{rname} attrs: {e}")))?,
            "mt",
        );
        for pname in rg
            .groups()
            .map_err(|e| CollapseError::Hdf5(format!("{name}/{rname} products: {e}")))?
            .into_iter()
            .filter(|p| p.starts_with("product"))
        {
            let pg = rg
                .group(&pname)
                .map_err(|e| CollapseError::Hdf5(format!("{name}/{rname}/{pname}: {e}")))?;
            let attrs = pg
                .attrs()
                .map_err(|e| CollapseError::Hdf5(format!("{name}/{rname}/{pname} attrs: {e}")))?;
            if !matches!(attrs.get("particle"), Some(hdf5_pure::AttrValue::AsciiString(s)) if s == "photon")
            {
                continue;
            }
            let xs: Vec<f64> = rg
                .group("294K")
                .and_then(|t| t.dataset("xs"))
                .and_then(|d| d.read_f64())
                .map_err(|e| CollapseError::Hdf5(format!("{name}/{rname} xs: {e}")))?;
            let offset = grid_len.saturating_sub(xs.len());
            let mut full = vec![0.0; grid_len];
            for (j, v) in xs.iter().enumerate() {
                full[offset + j] = *v;
            }
            let yield_table = pg
                .dataset("yield")
                .map(|ds| read_pair_table(&ds))
                .unwrap_or_default();

            let (Ok(dg),) = (pg.group("distribution_0"),) else {
                continue;
            };
            let Ok(eg) = dg.group("energy") else {
                continue;
            };
            let eattrs = eg
                .attrs()
                .map_err(|e| CollapseError::Hdf5(format!("{name} dist attrs: {e}")))?;
            let etype = match eattrs.get("type") {
                Some(hdf5_pure::AttrValue::AsciiString(s)) => s.clone(),
                _ => String::new(),
            };
            let spectrum = if etype == "discrete_photon" {
                ProductSpectrum::DiscreteLine {
                    line_ev: read_attr_f64(&eattrs, "energy"),
                }
            } else {
                let (Ok(ed), Ok(dist)) = (eg.dataset("energy"), eg.dataset("distribution")) else {
                    continue;
                };
                let incident: Vec<f64> = ed
                    .read_f64()
                    .map_err(|e| CollapseError::Hdf5(format!("{name} dist energy: {e}")))?;
                let dattrs = dist
                    .attrs()
                    .map_err(|e| CollapseError::Hdf5(format!("{name} dist dattrs: {e}")))?;
                let offsets: Vec<i64> = match dattrs.get("offsets") {
                    Some(hdf5_pure::AttrValue::I64Array(v)) => v.clone(),
                    _ => Vec::new(),
                };
                let n_lines: Vec<i64> = match dattrs.get("n_discrete_lines") {
                    Some(hdf5_pure::AttrValue::I64Array(v)) => v.clone(),
                    _ => Vec::new(),
                };
                let flat = dist
                    .read_f64()
                    .map_err(|e| CollapseError::Hdf5(format!("{name} dist data: {e}")))?;
                let n_cols = flat.len() / 3;
                let eout = &flat[0..n_cols];
                let pdf = &flat[n_cols..2 * n_cols];
                let mut tables = Vec::with_capacity(incident.len());
                for i in 0..incident.len() {
                    let j = offsets.get(i).copied().unwrap_or(0).max(0) as usize;
                    let end = if i + 1 < incident.len() {
                        offsets.get(i + 1).copied().unwrap_or(n_cols as i64) as usize
                    } else {
                        n_cols
                    }
                    .min(n_cols);
                    let m = n_lines.get(i).copied().unwrap_or(0).max(0) as usize;
                    let lines: Vec<(f64, f64)> =
                        (j..(j + m).min(end)).map(|k| (eout[k], pdf[k])).collect();
                    let cont: Vec<(f64, f64)> = ((j + m)..end).map(|k| (eout[k], pdf[k])).collect();
                    tables.push((lines, cont));
                }
                ProductSpectrum::Continuous {
                    energy: incident,
                    tables,
                }
            };
            out.push(PhotonProduct {
                mt,
                xs: full,
                yield_table,
                spectrum,
            });
        }
    }
    Ok(out)
}

/// Options for [`collapse_photon`].
pub struct PhotonCollapseOptions {
    /// Directory of `photo_<Elem>.h5` photon-atomic tables.
    pub photon_library_dir: PathBuf,
    /// Directory of `<Nuclide>.h5` neutron tables (for production).
    pub neutron_library_dir: PathBuf,
    /// The material definitions the artifact is bound to.
    pub materials: Vec<MaterialDefinition>,
    /// Photon group edges, eV, strictly descending.
    pub photon_boundaries_ev: Vec<f64>,
    /// Neutron group edges, eV, strictly descending — must match the
    /// `MultigroupData` the neutron flux is solved against.
    pub neutron_boundaries_ev: Vec<f64>,
    /// Weighting spectrum shared with the neutron collapse (required
    /// for production-matrix consistency).
    pub weighting: WeightingSpectrum,
    /// Artifact id.
    pub id: String,
    /// Component-profile reference for the kerma response.
    pub component_profile: Option<ContentReference>,
    /// Extra free-text appended to the collapse declaration.
    pub note: String,
}

/// Collapse the coupled photon tables for every material.
pub fn collapse_photon(
    opts: &PhotonCollapseOptions,
) -> Result<MultigroupPhotonData, CollapseError> {
    let pb = &opts.photon_boundaries_ev;
    let nb = &opts.neutron_boundaries_ev;
    if pb.len() < 2 || !pb.windows(2).all(|w| w[0] > w[1]) {
        return Err(invalid("photon boundaries must be strictly descending"));
    }
    if nb.len() < 2 || !nb.windows(2).all(|w| w[0] > w[1]) {
        return Err(invalid("neutron boundaries must be strictly descending"));
    }
    if opts.materials.is_empty() {
        return Err(invalid("at least one material is required"));
    }
    let g_gamma = pb.len() - 1;
    let g_n = nb.len() - 1;
    let annih_group = (0..g_gamma)
        .find(|&g| MC2_EV <= pb[g] && MC2_EV > pb[g + 1])
        .unwrap_or(g_gamma - 1);
    let b10_line_group = (0..g_gamma)
        .find(|&g| B10_NA1G_PHOTON_EV <= pb[g] && B10_NA1G_PHOTON_EV > pb[g + 1])
        .unwrap_or(g_gamma - 1);

    let mut elements: BTreeMap<String, PhotonElement> = BTreeMap::new();
    let mut products: BTreeMap<String, (Vec<f64>, Vec<PhotonProduct>)> = BTreeMap::new();

    let mut materials = Vec::with_capacity(opts.materials.len());
    for material in &opts.materials {
        material
            .validate()
            .map_err(|e| CollapseError::Model(format!("material invalid: {e}")))?;
        let rho = material.density_g_cm3;
        let mut sigma_t = vec![0.0; g_gamma];
        let mut scatter = vec![0.0; g_gamma * g_gamma];
        let mut scatter_p1 = vec![0.0; g_gamma * g_gamma];
        let mut production = vec![0.0; g_n * g_gamma];
        let mut dose = vec![0.0; g_gamma];

        for nuc in &material.nuclides {
            let (mass_g_mol, _a) = nuclide_mass(&nuc.name)
                .ok_or_else(|| invalid(format!("no mass table entry for {:?}", nuc.name)))?;
            let dens = rho * nuc.mass_fraction / mass_g_mol * N_A * BARN_CM2;
            let elem = element_of(&nuc.name)
                .ok_or_else(|| invalid(format!("no photon-atomic element for {:?}", nuc.name)))?;
            let pe = match elements.get(elem) {
                Some(e) => e,
                None => {
                    let e = load_photon_element(&opts.photon_library_dir, elem)?;
                    elements.entry(elem.to_string()).or_insert(e)
                }
            };

            // --- Photon interaction collapse ---
            for gg in 0..g_gamma {
                let (lo, hi) = (pb[gg + 1], pb[gg]);
                let s_pe = group_average(&pe.energy, &pe.photoelectric, lo, hi, &opts.weighting);
                let s_inc = group_average(&pe.energy, &pe.incoherent, lo, hi, &opts.weighting);
                let s_coh = group_average(&pe.energy, &pe.coherent, lo, hi, &opts.weighting);
                let s_pair = group_average(&pe.energy, &pe.pair_nuclear, lo, hi, &opts.weighting)
                    + group_average(&pe.energy, &pe.pair_electron, lo, hi, &opts.weighting);
                sigma_t[gg] += dens * (s_pe + s_inc + s_coh + s_pair);
                // Incoherent KN transfer at the group's log-mean energy.
                let e_rep = (lo * hi).sqrt();
                let (frac, mu_kn) = kn_transfer(e_rep, pb);
                for (gp, f) in frac.iter().enumerate() {
                    scatter[gg * g_gamma + gp] += dens * s_inc * f;
                    scatter_p1[gg * g_gamma + gp] += dens * s_inc * f * mu_kn;
                }
                // Coherent: elastic → diagonal, maximally forward.
                scatter[gg * g_gamma + gg] += dens * s_coh;
                scatter_p1[gg * g_gamma + gg] += dens * s_coh;
                // Pair production → 2 annihilation photons at 511 keV
                // (isotropic — no P1 contribution).
                scatter[gg * g_gamma + annih_group] += dens * 2.0 * s_pair;
                // Kerma response: photoelectric full-E + pair (E−2mc²)
                // + incoherent recoil (E−E′); coherent ~0.
                let dep_inc: f64 = frac
                    .iter()
                    .enumerate()
                    .map(|(gp, f)| {
                        let ep_rep = (pb[gp] * pb[gp + 1]).sqrt();
                        f * (e_rep - ep_rep).max(0.0)
                    })
                    .sum();
                let dep_ev =
                    s_pe * e_rep + s_pair * (e_rep - 2.0 * MC2_EV).max(0.0) + s_inc * dep_inc;
                dose[gg] += dens * dep_ev * EV_CM_TO_GY_CM2_DENSITY / rho;
            }

            // --- n→γ production collapse ---
            let (nuc_energy, prods) = match products.get(&nuc.name) {
                Some(v) => v,
                None => {
                    let path = opts.neutron_library_dir.join(format!("{}.h5", nuc.name));
                    let file = File::open(&path)
                        .map_err(|e| CollapseError::Hdf5(format!("{}: {e}", path.display())))?;
                    let ng = file
                        .group(&format!("/{}", nuc.name))
                        .map_err(|e| CollapseError::Hdf5(format!("{} group: {e}", nuc.name)))?;
                    let energy: Vec<f64> = ng
                        .group("energy")
                        .and_then(|g| g.dataset("294K"))
                        .and_then(|d| d.read_f64())
                        .map_err(|e| {
                            CollapseError::Hdf5(format!("{} energy/294K: {e}", nuc.name))
                        })?;
                    let prods = load_photon_products(&ng, &nuc.name, energy.len())?;
                    products.entry(nuc.name.clone()).or_insert((energy, prods))
                }
            };
            for prod in prods {
                let ye: Vec<f64> = prod.yield_table.iter().map(|p| p.0).collect();
                let yy: Vec<f64> = prod.yield_table.iter().map(|p| p.1).collect();
                for gn in 0..g_n {
                    let (lo, hi) = (nb[gn + 1], nb[gn]);
                    let mut edges = vec![lo];
                    edges.extend(nuc_energy.iter().copied().filter(|e| *e > lo && *e < hi));
                    edges.extend(ye.iter().copied().filter(|e| *e > lo && *e < hi));
                    edges.sort_by(|a, b| a.total_cmp(b));
                    edges.dedup();
                    edges.push(hi);
                    for gg in 0..g_gamma {
                        let (plo, phi) = (pb[gg + 1], pb[gg]);
                        let mut acc = 0.0;
                        let mut wacc = 0.0;
                        for i in 0..edges.len() - 1 {
                            let m = 0.5 * (edges[i] + edges[i + 1]);
                            let s = log_interp(nuc_energy, &prod.xs, m);
                            let y = if ye.is_empty() {
                                1.0
                            } else {
                                log_interp(&ye, &yy, m)
                            };
                            let f = spectrum_fraction(&prod.spectrum, m, plo, phi);
                            let wv = opts.weighting.w(m);
                            acc += s * y * f * wv * (edges[i + 1] - edges[i]);
                            wacc += wv * (edges[i + 1] - edges[i]);
                        }
                        if wacc > 0.0 {
                            production[gn * g_gamma + gg] += dens * acc / wacc;
                        }
                    }
                }
            }
            // Declared ¹⁰B(n,α₁γ) 0.478 MeV line — the dominant prompt
            // photon in BNCT; the evaluation's photon products may not
            // carry it explicitly.
            if nuc.name == "B10" {
                let path = opts.neutron_library_dir.join("B10.h5");
                if let Ok(file) = File::open(&path)
                    && let Ok(ng) = file.group("/B10")
                    && let (Ok(reactions), Ok(ed)) = (
                        ng.group("reactions"),
                        ng.group("energy").and_then(|g| g.dataset("294K")),
                    )
                    && let (Ok(r107), Ok(ne)) = (reactions.group("reaction_107"), ed.read_f64())
                    && let Ok(xs_ds) = r107.group("294K").and_then(|t| t.dataset("xs"))
                    && let Ok(xs) = xs_ds.read_f64()
                {
                    let offset = ne.len().saturating_sub(xs.len());
                    let mut full = vec![0.0; ne.len()];
                    for (j, v) in xs.iter().enumerate() {
                        full[offset + j] = *v;
                    }
                    for gn in 0..g_n {
                        let (lo, hi) = (nb[gn + 1], nb[gn]);
                        let mut acc = 0.0;
                        let mut wacc = 0.0;
                        let mut edges = vec![lo];
                        edges.extend(ne.iter().copied().filter(|e| *e > lo && *e < hi));
                        edges.push(hi);
                        for i in 0..edges.len() - 1 {
                            let m = 0.5 * (edges[i] + edges[i + 1]);
                            let s = log_interp(&ne, &full, m);
                            let wv = opts.weighting.w(m);
                            acc += s * B10_NA1G_BRANCH * wv * (edges[i + 1] - edges[i]);
                            wacc += wv * (edges[i + 1] - edges[i]);
                        }
                        if wacc > 0.0 {
                            production[gn * g_gamma + b10_line_group] += dens * acc / wacc;
                        }
                    }
                }
            }
        }

        // transport_mu_bar = P1-weighted mean cosine per group.
        let mut mu_bar = vec![0.0; g_gamma];
        for gg in 0..g_gamma {
            let row: f64 = (0..g_gamma).map(|gp| scatter[gg * g_gamma + gp]).sum();
            let row_p1: f64 = (0..g_gamma).map(|gp| scatter_p1[gg * g_gamma + gp]).sum();
            mu_bar[gg] = if row > 0.0 { row_p1 / row } else { 0.0 };
        }

        materials.push(PhotonMaterial {
            material_id: material.id.clone(),
            sigma_total_per_cm: sigma_t,
            scatter_matrix_per_cm: scatter,
            scatter_p1_matrix_per_cm: Some(scatter_p1),
            transport_mu_bar: mu_bar,
            production_matrix_per_cm: production,
            dose_response_gy_cm2: dose,
        });
    }

    let declaration = format!(
        "coupled photon multigroup collapse ({g_gamma} photon groups, {g_n} neutron groups): \
         photoelectric+incoherent(Klein–Nishina)+coherent+pair XS from photo-atomic HDF5; \
         n→γ production from neutron evaluations' photon products ({}); {}. \
         Pair production sources 2×511 keV annihilation photons (transported). \
         Research-use data; not a clinical library.",
        match opts.weighting {
            WeightingSpectrum::ThermalMaxwellianEpithermalFlat { .. } => {
                "Maxwellian+1/E weighting"
            }
            WeightingSpectrum::FlatLethargy => "1/E weighting",
        },
        opts.note
    );

    let data = MultigroupPhotonData {
        schema_version: MULTIGROUP_PHOTON_DATA_SCHEMA.into(),
        id: opts.id.clone(),
        energy_boundaries_ev: pb.clone(),
        neutron_energy_boundaries_ev: nb.clone(),
        collapse_declaration: declaration,
        component_profile: opts.component_profile.clone(),
        materials,
    };
    data.validate()
        .map_err(|e| invalid(format!("collapsed photon data invalid: {e}")))?;
    Ok(data)
}
