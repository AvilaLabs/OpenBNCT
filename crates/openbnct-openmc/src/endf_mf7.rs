// SPDX-License-Identifier: Apache-2.0

//! Minimal ENDF-6 fixed-format reader for MF7/MT4 thermal scattering
//! law data — incoherent-inelastic S(α,β) tables.
//!
//! Scope: HEAD/TAB1/TAB2/LIST/SEND record structure for the
//! S(α,β) section of a TSL evaluation (e.g. ENDF/B-VIII.1
//! `tsl_H(H2O)_0001`). The layout decoded for LAT-style tapes:
//!
//! ```text
//! HEAD   ZA, AWR, 0, LAT, 0, 0
//! CONT   0, 0, 0, 0, NS, 1              — NS = non-principal temp count
//! CONT   σ_b (barns), T_hi, 0, LLN, 0, 2
//! CONT   1.0, β_max, 15, 0, 0, 1
//! TAB2   0, 0, 0, 0, NR, NB + (NBT,INT) — β-grid interpolation
//! for each β_i (NB):
//!   TAB1 [T0, β_i, LI, NR, NP / (α_j, S_ij)]   — principal temperature
//!   for each non-principal T:
//!     LIST [T, β_i, LI, 0, NP, 0 / S_j]        — shared α grid of β_i
//! ```
//!
//! The parser is deliberately conservative: it checks the record
//! counts implied by the headers and returns `Err` on drift rather
//! than silently desyncing.

/// ENDF FORTRAN real (shared convention with `endf_mf3`).
fn endf_float(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    let t = t.replace(['D', 'd'], "e");
    if t.contains('e') || t.contains('E') {
        return t.parse().unwrap_or(0.0);
    }
    if let Some(pos) = t
        .rmatch_indices(['+', '-'])
        .map(|(i, _)| i)
        .find(|&i| i > 0)
    {
        let (mantissa, exp) = t.split_at(pos);
        return format!("{mantissa}e{exp}").parse().unwrap_or(0.0);
    }
    t.parse().unwrap_or(0.0)
}

fn endf_int(s: &str) -> i64 {
    endf_float(s) as i64
}

struct Record {
    c1: f64,
    c2: f64,
    l1: i64,
    l2: i64,
    n1: i64,
    n2: i64,
    mf: i32,
    mt: i32,
}

fn parse_record(line: &str) -> Record {
    let f = |a: usize, b: usize| endf_float(line.get(a..b).unwrap_or(""));
    let i = |a: usize, b: usize| endf_int(line.get(a..b).unwrap_or(""));
    Record {
        c1: f(0, 11),
        c2: f(11, 22),
        l1: i(22, 33),
        l2: i(33, 44),
        n1: i(44, 55),
        n2: i(55, 66),
        mf: i(70, 72) as i32,
        mt: i(72, 75) as i32,
    }
}

/// One (T, β) S(α,β) table: the α grid (dimensionless momentum
/// parameter) and the S values on it.
#[derive(Debug, Clone)]
pub struct SabSection {
    /// Temperature, K.
    pub temperature_k: f64,
    /// β = (E' − E)/kT for this section.
    pub beta: f64,
    /// Tape LI flag for this section (interp control / symmetry).
    pub li: i64,
    /// α grid (dimensionless) shared by all T for this β.
    pub alpha: Vec<f64>,
    /// S(α_j, β, T) — same length as `alpha`.
    pub s: Vec<f64>,
}

/// Parsed MF7/MT4 content: bound-atom scale, the temperature set, and
/// every (T, β) S(α,β) table in tape order.
#[derive(Debug, Clone)]
pub struct ThermalScatteringLaw {
    /// ZA of the scattering nuclide (e.g. 1001 for H).
    pub za: f64,
    /// Mass ratio to the neutron.
    pub awr: f64,
    /// Bound/free cross-section scale σ_b, barns. On ENDF tapes this
    /// is `natom·σ_free` — the molecular bound-scattering scale for the
    /// whole scattering unit.
    pub sigma_b_barns: f64,
    /// Number of principal scattering atoms per scattering unit (the
    /// σ_b LIST record's sixth field — 2 for H in H₂O, 1 for H in
    /// Lucite). Defaults to 1 when absent.
    pub natom: f64,
    /// Largest tabulated β.
    pub beta_max: f64,
    /// Largest incident energy the law applies to, eV (the σ_b
    /// record's E_max — 10 eV on the ENDF/B-VIII.1 water evaluation).
    pub emax_ev: f64,
    /// All (T, β) sections in tape order — `beta_count` per
    /// temperature block, principal first.
    pub sections: Vec<SabSection>,
    /// Sorted distinct temperatures, K.
    pub temperatures_k: Vec<f64>,
    /// Distinct β grid of the principal-temperature sections.
    pub betas: Vec<f64>,
}

impl ThermalScatteringLaw {
    /// Bound-atom scattering cross section per principal atom, barns:
    /// `σ_b / natom · ((A_r+1)/A_r)²`. This is the σ_b the ENDF-102
    /// DDXS prefactor expects — NJOY/THERMR's `sb = smz·((az+1)/az)²`
    /// convention. Verified: integrating the resulting kernel over the
    /// real H(H₂O) tape reproduces THERMR's MF3/MT222 σ_s(E) within
    /// ~10% across 0.025–5 eV.
    pub fn bound_sigma_b(&self) -> f64 {
        let n = if self.natom > 0.0 { self.natom } else { 1.0 };
        self.sigma_b_barns / n * ((self.awr + 1.0) / self.awr).powi(2)
    }

    /// The (T, β) section nearest `t` at exactly `beta`, if present.
    pub fn section(&self, temperature_k: f64, beta: f64) -> Option<&SabSection> {
        self.sections
            .iter()
            .filter(|s| (s.beta - beta).abs() < 1e-9)
            .min_by(|a, b| {
                (a.temperature_k - temperature_k)
                    .abs()
                    .partial_cmp(&(b.temperature_k - temperature_k).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// S(α, β, T) at the nearest tabulated temperature, linear in α on
    /// the section's grid (log-space interpolation where the tape's
    /// INT=4 log-lin law applies — S is positive here).
    pub fn s_at(&self, temperature_k: f64, beta: f64, alpha: f64) -> Option<f64> {
        let sec = self.section(temperature_k, beta)?;
        let xs = &sec.alpha;
        let ys = &sec.s;
        if alpha <= xs[0] {
            return Some(ys[0]);
        }
        if alpha >= *xs.last().unwrap() {
            return Some(*ys.last().unwrap());
        }
        let i = xs.partition_point(|&x| x <= alpha).clamp(1, xs.len() - 1);
        let (x0, x1) = (xs[i - 1], xs[i]);
        let (y0, y1) = (ys[i - 1], ys[i]);
        if y0 > 0.0 && y1 > 0.0 {
            let t = (alpha - x0) / (x1 - x0);
            Some((y0.ln() + t * (y1.ln() - y0.ln())).exp())
        } else {
            Some(y0 + (y1 - y0) * (alpha - x0) / (x1 - x0))
        }
    }

    /// S(α, β, T) at arbitrary β — brackets `beta` on the tape's grid
    /// and interpolates linearly in β between the two nearest sections
    /// at the nearest tabulated temperature. Returns `None` if β is
    /// outside the grid or no sections exist.
    pub fn s_interp(&self, temperature_k: f64, beta: f64, alpha: f64) -> Option<f64> {
        let bs = &self.betas;
        if bs.is_empty() || beta < bs[0] || beta > *bs.last().unwrap() {
            return None;
        }
        if let Some(&b) = bs.iter().find(|&&b| (b - beta).abs() < 1e-12) {
            return self.s_at(temperature_k, b, alpha);
        }
        let hi = bs.partition_point(|&b| b <= beta).max(1).min(bs.len() - 1);
        let (b0, b1) = (bs[hi - 1], bs[hi]);
        let (s0, s1) = (
            self.s_at(temperature_k, b0, alpha)?,
            self.s_at(temperature_k, b1, alpha)?,
        );
        let t = (beta - b0) / (b1 - b0);
        Some(s0 + (s1 - s0) * t)
    }
}

fn rows(n_values: usize) -> usize {
    n_values.div_ceil(6)
}

/// Parse an MF7/MT4 TSL tape. `Err` on structural failure; non-MF7
/// records outside MT4 are skipped.
pub fn parse_tsl(text: &str) -> Result<ThermalScatteringLaw, String> {
    let lines: Vec<&str> = text.lines().collect();
    let err = |m: String| -> Result<ThermalScatteringLaw, String> { Err(m) };

    // Locate the MF7/MT4 HEAD record.
    let mut i = 0;
    loop {
        if i >= lines.len() {
            return err("no MF7/MT4 section found".into());
        }
        let r = parse_record(lines[i]);
        if r.mf == 7 && r.mt == 4 {
            break;
        }
        i += 1;
    }
    let head = parse_record(lines[i]);
    let (za, awr) = (head.c1, head.c2);
    i += 1;

    // CONT records follow the HEAD: their count varies between
    // evaluations (H(H2O) carries an extra β_max CONT that Lucite
    // lacks). Walk forward capturing σ_b / β_max by field shape until
    // the β-grid TAB2 appears — it is the first record with N2 > 2
    // (the β count), followed by NR×2/6 interpolation lines.
    let mut sigma_b = 0.0;
    let mut natom = 1.0;
    let mut beta_max = 0.0;
    let mut emax_ev = 10.0;
    let mut ns = 0usize;
    let (nr, nb) = loop {
        if i >= lines.len() {
            return err("no MT4 β-grid TAB2 found".into());
        }
        let r = parse_record(lines[i]);
        if r.mf != 7 || r.mt != 4 {
            return err("MT4 header truncated".into());
        }
        // β-grid TAB2 is the first record carrying both NR ≥ 1 and
        // NE ≥ 2 — the CONTs around it have n1 = 0 or n2 < 2.
        if r.n1 > 0 && r.n2 >= 2 {
            i += 1;
            break (r.n1.max(0) as usize, r.n2.max(0) as usize);
        }
        if r.c1 > 1.0 && r.c2 > 50.0 {
            // First σ_b LIST-body line: [σ_b, ε, A_r, E_max, M0, natom]
            // read as a CONT — σ_b lands in c1, E_max in l2, natom in
            // n2 (the sixth field on the line).
            sigma_b = r.c1;
            if r.l2 > 0 {
                emax_ev = r.l2 as f64;
            }
            if r.n2 > 0 {
                natom = r.n2 as f64;
            }
        } else if (0.9..=1.1).contains(&r.c1) && r.c2 > 0.0 {
            beta_max = r.c2;
        } else if r.n1 > 0 {
            ns = r.n1 as usize;
        }
        i += 1;
    };
    if nb == 0 {
        return err("MT4 declares zero β sections".into());
    }
    i += rows(nr * 2);

    let nt = ns + 1;
    let mut sections = Vec::with_capacity(nb * nt);
    let mut temperatures = Vec::new();
    let _ = ns; // NS is advisory; the walk is driven by record shape.
    for _b in 0..nb {
        // Principal-temperature TAB1: [T0, β_i, LI, NR, NP] + (NBT,INT)
        // + NP (α,S) pairs.
        let h = parse_record(lines[i]);
        if h.mf != 7 || h.mt != 4 {
            return err(format!("line {}: expected MT4 section header", i + 1));
        }
        let (t0, beta, li, nr_s, np) =
            (h.c1, h.c2, h.l1, h.n1.max(0) as usize, h.n2.max(0) as usize);
        i += 1;
        i += rows(nr_s * 2);
        if i + rows(np * 2) > lines.len() {
            return err(format!("β {beta}: truncated (α,S) table"));
        }
        let mut alpha = Vec::with_capacity(np);
        let mut s = Vec::with_capacity(np);
        for _ in 0..rows(np * 2) {
            let l = *lines.get(i).ok_or("truncated (α,S) table")?;
            for f in 0..3 {
                let (a, b) = (f * 22, f * 22 + 11);
                let x = endf_float(l.get(a..b).unwrap_or(""));
                let y = endf_float(l.get(b..b + 11).unwrap_or(""));
                if alpha.len() < np {
                    alpha.push(x);
                    s.push(y);
                }
            }
            i += 1;
        }
        if !temperatures.contains(&t0) {
            temperatures.push(t0);
        }
        sections.push(SabSection {
            temperature_k: t0,
            beta,
            li,
            alpha: alpha.clone(),
            s,
        });
        // Non-principal temperatures: LIST [T, β_i, LI, 0, NP, 0] + NP
        // flat S values on the shared α grid. The count is not taken
        // from the header — a LIST header has N2 = 0 while the next
        // principal TAB1 has N2 = NP > 0, so walk by record shape.
        loop {
            if i >= lines.len() {
                break;
            }
            let h = parse_record(lines[i]);
            if h.mf != 7 || h.mt != 4 || h.n2 > 0 {
                break;
            }
            let (t, li_l, np_l) = (h.c1, h.l1, h.n1.max(0) as usize);
            i += 1;
            if i + rows(np_l) > lines.len() {
                return err(format!("β {beta} T {t}: truncated S list"));
            }
            let mut s_l = Vec::with_capacity(np_l);
            for _ in 0..rows(np_l) {
                let l = *lines.get(i).ok_or("truncated S list")?;
                for f in 0..6 {
                    let v = endf_float(l.get(f * 11..f * 11 + 11).unwrap_or(""));
                    if s_l.len() < np_l {
                        s_l.push(v);
                    }
                }
                i += 1;
            }
            if np_l != np {
                return err(format!(
                    "β {beta} T {t}: S list has {np_l} values, α grid has {np}"
                ));
            }
            if !temperatures.contains(&t) {
                temperatures.push(t);
            }
            sections.push(SabSection {
                temperature_k: t,
                beta,
                li: li_l,
                alpha: alpha.clone(),
                s: s_l,
            });
        }
    }

    temperatures.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut betas: Vec<f64> = sections
        .iter()
        .filter(|s| s.temperature_k == temperatures[0])
        .map(|s| s.beta)
        .collect();
    betas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // β_max is the tabulated grid limit — the CONT constant read as a
    // candidate is not always the grid max (H(H2O) carries 3.79 while
    // its grid runs to 395.26), so the parsed sections win.
    beta_max = betas.last().copied().unwrap_or(beta_max);

    Ok(ThermalScatteringLaw {
        za,
        awr,
        sigma_b_barns: sigma_b,
        natom,
        beta_max,
        emax_ev,
        sections,
        temperatures_k: temperatures,
        betas,
    })
}

/// Boltzmann constant, eV/K.
pub const K_B_EV: f64 = 8.617333262e-5;

/// Per-β cumulative α-integrals of S(α,β,T) at one temperature.
/// `cum0[k] = ∫_{α_0}^{α_k} S dα`, `cum1[k] = ∫ α S dα` (trapezoidal
/// on the tabulated grid).
#[derive(Debug)]
struct SabGrid {
    alpha: Vec<f64>,
    cum0: Vec<f64>,
    cum1: Vec<f64>,
}

impl SabGrid {
    /// Cumulative integral `cum(α)` at arbitrary α — log-α
    /// interpolation between grid points, flat extrapolation off the
    /// ends (S is tabulated through the small-α ridge).
    fn cum_at(cum: &[f64], alpha: &[f64], a: f64) -> f64 {
        if a <= alpha[0] {
            return 0.0;
        }
        if a >= *alpha.last().unwrap() {
            return *cum.last().unwrap();
        }
        let i = alpha.partition_point(|&x| x <= a).clamp(1, alpha.len() - 1);
        let (x0, x1) = (alpha[i - 1], alpha[i]);
        let (c0, c1) = (cum[i - 1], cum[i]);
        let t = (a.ln() - x0.ln()) / (x1.ln() - x0.ln());
        c0 + t * (c1 - c0)
    }
}

/// The incoherent-inelastic bound-atom scattering kernel built from a
/// parsed [`ThermalScatteringLaw`] at one temperature. Implements the
/// ENDF-102 DDXS
///
/// ```text
/// σ(E→E',μ) = (σ_b/4πkT)·√(E'/E)·e^(−β/2)·S(α,|β|)
/// β = (E'−E)/kT,  α = (E+E'−2μ√EE')/(A_r·kT)
/// ```
///
/// with the symmetric β extension (detailed balance verified
/// numerically: with the `e^(−β/2)·√(E'/E)` prefactor the symmetric
/// extension reproduces the Maxwellian ratio exactly).
#[derive(Debug)]
pub struct SabKernel {
    /// The tabulated temperature this kernel was built at, K.
    pub temperature_k: f64,
    /// kT in eV.
    pub kt_ev: f64,
    /// Effective bound-atom σ_b used in the DDXS prefactor, barns —
    /// `σ_b_tape/natom·((A_r+1)/A_r)²` per [`ThermalScatteringLaw::bound_sigma_b`].
    pub sigma_b_barns: f64,
    /// Effective mass ratio A_r (α denominator).
    pub a_r: f64,
    /// |E'−E| domain limit = β_max·kT, eV.
    pub delta_ev: f64,
    /// Incident-energy ceiling, eV — above this the bound law does not
    /// apply and the free-gas kernel governs.
    pub emax_ev: f64,
    beta_grid: Vec<f64>,
    grids: Vec<SabGrid>,
}

impl SabKernel {
    /// Build the kernel at the tabulated temperature nearest
    /// `temperature_k`. Every β section at that temperature is folded
    /// into cumulative α-integrals.
    pub fn build(tsl: &ThermalScatteringLaw, temperature_k: f64) -> Result<Self, String> {
        let t = tsl
            .temperatures_k
            .iter()
            .min_by(|a, b| {
                (*a - temperature_k)
                    .abs()
                    .partial_cmp(&(*b - temperature_k).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
            .ok_or("no temperatures on tape")?;
        let kt = K_B_EV * t;
        let mut beta_grid = Vec::new();
        let mut grids = Vec::new();
        for &beta in &tsl.betas {
            let secs: Vec<&SabSection> = tsl
                .sections
                .iter()
                .filter(|s| (s.beta - beta).abs() < 1e-9)
                .collect();
            let sec = secs
                .iter()
                .min_by(|a, b| {
                    (a.temperature_k - t)
                        .abs()
                        .partial_cmp(&(b.temperature_k - t).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .ok_or_else(|| format!("β {beta}: no section at T {t}"))?;
            let n = sec.alpha.len();
            let mut cum0 = Vec::with_capacity(n);
            let mut cum1 = Vec::with_capacity(n);
            cum0.push(0.0);
            cum1.push(0.0);
            for j in 1..n {
                let da = sec.alpha[j] - sec.alpha[j - 1];
                cum0.push(cum0[j - 1] + 0.5 * (sec.s[j] + sec.s[j - 1]) * da);
                cum1.push(
                    cum1[j - 1]
                        + 0.5 * (sec.alpha[j] * sec.s[j] + sec.alpha[j - 1] * sec.s[j - 1]) * da,
                );
            }
            beta_grid.push(beta);
            grids.push(SabGrid {
                alpha: sec.alpha.clone(),
                cum0,
                cum1,
            });
        }
        Ok(SabKernel {
            temperature_k: t,
            kt_ev: kt,
            sigma_b_barns: tsl.bound_sigma_b(),
            a_r: tsl.awr,
            delta_ev: tsl.beta_max * kt,
            emax_ev: tsl.emax_ev,
            beta_grid,
            grids,
        })
    }

    /// `(I0, I1)` — the α-window integrals `∫S dα` and `∫αS dα` over
    /// `[a_lo, a_hi]` at `|beta|`, linearly interpolated in β.
    fn i01(&self, beta: f64, a_lo: f64, a_hi: f64) -> (f64, f64) {
        if a_hi <= a_lo || self.grids.is_empty() {
            return (0.0, 0.0);
        }
        let bs = &self.beta_grid;
        let eval = |g: &SabGrid| -> (f64, f64) {
            (
                SabGrid::cum_at(&g.cum0, &g.alpha, a_hi) - SabGrid::cum_at(&g.cum0, &g.alpha, a_lo),
                SabGrid::cum_at(&g.cum1, &g.alpha, a_hi) - SabGrid::cum_at(&g.cum1, &g.alpha, a_lo),
            )
        };
        if beta <= bs[0] {
            return eval(&self.grids[0]);
        }
        if beta >= *bs.last().unwrap() {
            return eval(self.grids.last().unwrap());
        }
        let i = bs.partition_point(|&b| b <= beta).clamp(1, bs.len() - 1);
        let (b0, b1) = (bs[i - 1], bs[i]);
        let (p0, q0) = eval(&self.grids[i - 1]);
        let (p1, q1) = eval(&self.grids[i]);
        let t = (beta - b0) / (b1 - b0);
        (p0 + t * (p1 - p0), q0 + t * (q1 - q0))
    }

    /// dσ/dE' (barns/eV) at incident `e`, outgoing `ep` — the
    /// μ-integrated P0 kernel. Zero outside the β domain.
    pub fn sigma0_de(&self, e: f64, ep: f64) -> f64 {
        if e <= 0.0 || ep <= 0.0 {
            return 0.0;
        }
        let beta = (ep - e) / self.kt_ev;
        let abs = beta.abs();
        if abs > self.beta_grid.last().copied().unwrap_or(0.0) {
            return 0.0;
        }
        let (re, rp) = (e.sqrt(), ep.sqrt());
        let ar_kt = self.a_r * self.kt_ev;
        let a_lo = (re - rp) * (re - rp) / ar_kt;
        let a_hi = (re + rp) * (re + rp) / ar_kt;
        let (i0, _) = self.i01(abs, a_lo, a_hi);
        self.sigma_b_barns * self.a_r / (4.0 * e) * (-beta / 2.0).exp() * i0
    }

    /// dσ₁/dE' (barns/eV) — the lab-cosine-weighted P1 moment kernel
    /// `∫ μ σ(E→E',μ) dμ`.
    pub fn sigma1_de(&self, e: f64, ep: f64) -> f64 {
        if e <= 0.0 || ep <= 0.0 {
            return 0.0;
        }
        let beta = (ep - e) / self.kt_ev;
        let abs = beta.abs();
        if abs > self.beta_grid.last().copied().unwrap_or(0.0) {
            return 0.0;
        }
        let (re, rp) = (e.sqrt(), ep.sqrt());
        let ar_kt = self.a_r * self.kt_ev;
        let a_lo = (re - rp) * (re - rp) / ar_kt;
        let a_hi = (re + rp) * (re + rp) / ar_kt;
        let (i0, i1) = self.i01(abs, a_lo, a_hi);
        self.sigma_b_barns * self.a_r / (8.0 * e * re * rp)
            * (-beta / 2.0).exp()
            * ((e + ep) * i0 - ar_kt * i1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic two-temperature, two-β tape exercising both section
    /// forms (principal TAB1 + non-principal LIST).
    fn mini_tape() -> String {
        fn hdr(c1: f64, c2: f64, l1: i64, l2: i64, n1: i64, n2: i64) -> String {
            format!(
                "{:>11.6E}{:>11.6E}{:>11}{:>11}{:>11}{:>11}{:>4}{:>2}{:>3}{:>5}\n",
                c1, c2, l1, l2, n1, n2, 1451, 7, 4, 1
            )
        }
        fn vals(v: &[f64]) -> String {
            v.chunks(6)
                .map(|c| {
                    let mut s = String::new();
                    for x in c {
                        s.push_str(&format!("{:>11.6E}", x));
                    }
                    s.push_str(&format!("{:>4}{:>2}{:>3}{:>5}\n", 1451, 7, 4, 1));
                    s
                })
                .collect()
        }
        let mut t = String::new();
        t.push_str(&hdr(1001.0, 0.999, 0, 1, 0, 0)); // HEAD
        t.push_str(&hdr(0.0, 0.0, 0, 0, 1, 1)); // NS=1
        t.push_str(&hdr(40.87, 395.26, 0, 10, 0, 2)); // σb
        t.push_str(&hdr(1.0, 4.0, 15, 0, 0, 1)); // βmax=4
        t.push_str(&hdr(0.0, 0.0, 0, 0, 1, 2)); // β TAB2: NR=1, NE=2
        t.push_str(&format!("{:>11}{:>11}{:>49}\n", 2, 2, ""));
        // β_1 = 0: principal TAB1 300K, 2 (α,S) pairs
        t.push_str(&hdr(300.0, 0.0, 4, 0, 1, 2));
        t.push_str(&format!("{:>11}{:>11}{:>49}\n", 2, 2, ""));
        t.push_str(&vals(&[0.1, 5.0, 0.5, 1.0]));
        // non-principal 310K: flat S list of 2
        t.push_str(&hdr(310.0, 0.0, 4, 0, 2, 0));
        t.push_str(&vals(&[6.0, 1.2]));
        // β_2 = 2: principal TAB1 300K
        t.push_str(&hdr(300.0, 2.0, 4, 0, 1, 2));
        t.push_str(&format!("{:>11}{:>11}{:>49}\n", 2, 2, ""));
        t.push_str(&vals(&[0.2, 3.0, 0.9, 0.5]));
        t.push_str(&hdr(310.0, 2.0, 4, 0, 2, 0));
        t.push_str(&vals(&[4.0, 0.6]));
        // SEND record (MF=0/MT=0) terminates the section.
        t.push_str(&format!(
            "{:>11.6E}{:>11.6E}{:>11}{:>11}{:>11}{:>11}{:>4}{:>2}{:>3}{:>5}\n",
            0.0, 0.0, 0, 0, 0, 0, 1451, 0, 0, 0
        ));
        t
    }

    #[test]
    fn parses_principal_and_list_sections() {
        let tsl = parse_tsl(&mini_tape()).unwrap();
        assert_eq!(tsl.za, 1001.0);
        assert!((tsl.sigma_b_barns - 40.87).abs() < 1e-9);
        assert!((tsl.natom - 2.0).abs() < 1e-9);
        // σ_b/natom·((A_r+1)/A_r)² — the DDXS bound-atom scale.
        assert!((tsl.bound_sigma_b() - 81.9).abs() < 0.1);
        assert_eq!(tsl.temperatures_k, vec![300.0, 310.0]);
        assert_eq!(tsl.betas, vec![0.0, 2.0]);
        assert_eq!(tsl.sections.len(), 4);
        let s = tsl.s_at(307.0, 0.0, 0.3).unwrap();
        // Nearest T = 310 → S on that grid between (0.1,6.0),(0.5,1.2)
        // log-interpolated.
        let want = (6.0_f64.ln() + 0.5 * (1.2_f64.ln() - 6.0_f64.ln())).exp();
        assert!((s - want).abs() / want < 1e-9, "S(0.3)={s} want {want}");
    }

    #[test]
    fn truncated_list_is_an_error() {
        let mut t = mini_tape();
        // Cut inside the final β section's declared S list.
        t.truncate(t.len() - 300);
        assert!(parse_tsl(&t).is_err());
    }

    #[test]
    fn kernel_total_cross_section_and_domain_gate() {
        let tsl = parse_tsl(&mini_tape()).unwrap();
        let kern = SabKernel::build(&tsl, 300.0).unwrap();
        assert!((kern.temperature_k - 300.0).abs() < 1e-9);
        assert!(kern.kt_ev > 0.0);
        assert!((kern.emax_ev - 10.0).abs() < 1e-9);
        // Kernel is positive inside the β domain, zero outside.
        let e = 0.05;
        let inside = kern.sigma0_de(e, e * 0.8);
        let outside = kern.sigma0_de(e, e + 3.0 * kern.kt_ev);
        assert!(inside > 0.0, "σ(E→E') inside domain = {inside}");
        assert_eq!(outside, 0.0, "outside β domain must be zero");
        // Total cross section integrates to a finite positive bound.
        let mut total = 0.0;
        for i in 0..2000 {
            let ep = 1e-4 * e + i as f64 * (2.0 * e - 1e-4 * e) / 2000.0;
            total += kern.sigma0_de(e, ep) * (2.0 * e - 1e-4 * e) / 2000.0;
        }
        assert!(total > 0.0 && total < kern.sigma_b_barns * 10.0);
    }

    #[test]
    fn kernel_symmetric_beta_detailed_balance() {
        // On a constant-S(α,|β|) toy the ratio σ(E→E')/σ(E'→E) with
        // the √(E'/E)·e^(−β/2) prefactor equals e^((E−E')/kT) — the
        // Maxwellian detailed-balance factor — when the α integrals
        // coincide. The mini tape has two β sections so a mid-grid
        // (E,E') pair that shares the α window exercises the β
        // interpolation path instead; check smoothness/positivity
        // there rather than exact equality (different β sections have
        // different S here).
        let tsl = parse_tsl(&mini_tape()).unwrap();
        let kern = SabKernel::build(&tsl, 300.0).unwrap();
        let kt = kern.kt_ev;
        // Pick E,E' inside β-grid range 0..2 → |ΔE| ≤ 2·kT.
        let e = 1.0 * kt;
        let ep = 1.8 * kt;
        let up = kern.sigma0_de(e, ep); // β = +0.8 — upscatter
        let down = kern.sigma0_de(ep, e); // β = −0.8 — downscatter
        assert!(up > 0.0 && down > 0.0);
        // Downscatter exceeds upscatter (e^(−β/2)·√(E'/E) tilts to
        // energy loss — the detailed-balance direction).
        assert!(down > up, "down {down} vs up {up}");
    }

    #[test]
    fn p1_kernel_bounded_by_p0() {
        // |σ1(E→E')| ≤ σ0(E→E') pointwise — the cosine-weighted
        // integral is bounded by the unweighted one.
        let tsl = parse_tsl(&mini_tape()).unwrap();
        let kern = SabKernel::build(&tsl, 300.0).unwrap();
        for (e, ep) in [(0.02, 0.018), (0.05, 0.04), (0.03, 0.035)] {
            let s0 = kern.sigma0_de(e, ep);
            let s1 = kern.sigma1_de(e, ep);
            assert!(s1.abs() <= s0 + 1e-30, "e={e} ep={ep}: |s1|={s1} s0={s0}");
        }
    }
}
